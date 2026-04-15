use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ndarray::Array2;
use serde::Serialize;
use tauri::Emitter;

use pcat_pipeline::active_contour::{
    compute_gradient_field, evolve_snake as pipeline_evolve_snake, init_circular_contour,
    insert_control_point, SnakeParams,
};
use pcat_pipeline::annotation::{self, AnnotationBatchParams, AnnotationTarget};
use pcat_pipeline::cpr::CprFrame;
use pcat_pipeline::mmd::{self, MaterialLibrary, MmdResult, PwsqsParams};
use pcat_pipeline::radial_angular::{self, CrossSectionSurface, RadialAngularParams};
use pcat_pipeline::roi;

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Result types returned to the frontend
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct SnakeResult {
    pub points: Vec<[f64; 2]>,
    pub iterations: usize,
    pub max_displacement: f64,
    pub converged: bool,
}

#[derive(Serialize)]
pub struct MmdSummary {
    pub method: String,
    pub iterations: usize,
    pub converged: bool,
    pub n_voxels: usize,
    pub mean_water_frac: f64,
    pub mean_lipid_frac: f64,
    pub mean_iodine_frac: f64,
    pub mean_calcium_frac: f64,
}

// ---------------------------------------------------------------------------
// Progress event payload
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct AnnotationProgress {
    stage: String,
    progress: f64,
}

// ---------------------------------------------------------------------------
// Helper: clone CprFrame for use on blocking thread
// ---------------------------------------------------------------------------

fn clone_frame(frame: &CprFrame) -> CprFrame {
    CprFrame {
        positions: frame.positions.clone(),
        tangents: frame.tangents.clone(),
        normals: frame.normals.clone(),
        binormals: frame.binormals.clone(),
        arclengths: frame.arclengths.clone(),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Generate annotation targets for the proximal RCA (or any vessel segment).
///
/// Returns cross-section images, auto-detected vessel walls, and initial snake
/// boundaries for each of N evenly-spaced cross-sections along the centerline.
#[tauri::command]
pub async fn generate_annotation_targets(
    centerline_mm: Vec<[f64; 3]>,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<Vec<AnnotationTarget>, String> {
    if centerline_mm.len() < 2 {
        return Err("centerline must have at least 2 points".into());
    }

    // Extract volume data and CprFrame under lock, then release.
    let (volume_data, spacing, origin, frame) = {
        let guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        let vol = guard
            .volume
            .as_ref()
            .ok_or_else(|| "no volume loaded".to_string())?;
        let frame = guard
            .cpr_frame
            .as_ref()
            .ok_or_else(|| "no CPR frame built — call build_cpr_frame first".to_string())?;
        (vol.data.clone(), vol.spacing, vol.origin, Arc::clone(frame))
    };

    let frame_clone = clone_frame(&frame);

    // Run heavy computation on a blocking thread.
    let targets = tokio::task::spawn_blocking(move || {
        let params = AnnotationBatchParams::default();
        annotation::generate_annotation_batch(
            &frame_clone,
            &volume_data,
            spacing,
            origin,
            &params,
        )
    })
    .await
    .map_err(|e| format!("generate_annotation_targets task failed: {e}"))?;

    // Store targets in state.
    {
        let mut guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        guard.annotation_targets = Some(targets.clone());
        // Reset contours and finalization state for fresh annotation session.
        guard.snake_contours.clear();
        guard.finalized.clear();
    }

    Ok(targets)
}

/// Initialize the snake contour on a specific cross-section.
///
/// If `init_radius_mm` is provided, creates a circular contour at that radius;
/// otherwise uses the target's pre-computed `init_boundary`.
///
/// Returns the initial contour points as `[x, y]` in pixel coordinates.
#[tauri::command]
pub async fn init_snake(
    target_index: usize,
    init_radius_mm: Option<f64>,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<Vec<[f64; 2]>, String> {
    let mut guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;

    let targets = guard
        .annotation_targets
        .as_ref()
        .ok_or_else(|| "no annotation targets generated".to_string())?;

    let target = targets
        .get(target_index)
        .ok_or_else(|| format!("target_index {target_index} out of range (0..{})", targets.len()))?;

    let contour = if let Some(radius_mm) = init_radius_mm {
        // Create a circular contour at the requested radius.
        let mm_per_pixel = 2.0 * target.width_mm / target.pixels as f64;
        let center_px = target.pixels as f64 / 2.0;
        let radius_px = radius_mm / mm_per_pixel;
        let n_points = target.init_boundary.len(); // match point count
        init_circular_contour(center_px, center_px, radius_px, n_points)
    } else {
        // Use the target's pre-computed init_boundary.
        target.init_boundary.clone()
    };

    guard.snake_contours.insert(target_index, contour.clone());
    guard.finalized.insert(target_index, false);

    Ok(contour)
}

/// Evolve the active contour on a cross-section for `n_iterations` steps.
///
/// Returns the updated contour points and convergence information.
#[tauri::command]
pub async fn evolve_snake(
    target_index: usize,
    n_iterations: usize,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<SnakeResult, String> {
    // Extract what we need under the lock, then release for the heavy computation.
    let (mut contour, image_vec, pixels) = {
        let guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;

        let targets = guard
            .annotation_targets
            .as_ref()
            .ok_or_else(|| "no annotation targets generated".to_string())?;

        let target = targets
            .get(target_index)
            .ok_or_else(|| format!("target_index {target_index} out of range"))?;

        let contour = guard
            .snake_contours
            .get(&target_index)
            .ok_or_else(|| format!("no snake initialized for target {target_index} — call init_snake first"))?
            .clone();

        (contour, target.image.clone(), target.pixels)
    };

    // Run evolution on a blocking thread.
    let result = tokio::task::spawn_blocking(move || {
        // Convert flat Vec<f32> to Array2<f32>.
        let image = Array2::from_shape_vec((pixels, pixels), image_vec)
            .map_err(|e| format!("image reshape failed: {e}"))?;

        let (grad_x, grad_y) = compute_gradient_field(&image);

        let params = SnakeParams::default();
        let info = pipeline_evolve_snake(
            &mut contour,
            &grad_x,
            &grad_y,
            &image,
            &params,
            n_iterations,
        );

        Ok::<_, String>(SnakeResult {
            points: contour,
            iterations: info.iterations,
            max_displacement: info.max_displacement,
            converged: info.converged,
        })
    })
    .await
    .map_err(|e| format!("evolve_snake task failed: {e}"))??;

    // Store the updated contour back in state.
    {
        let mut guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        guard
            .snake_contours
            .insert(target_index, result.points.clone());
    }

    Ok(result)
}

/// Replace the snake contour points for a cross-section (e.g. after user drag).
///
/// Also marks the contour as not finalized.
#[tauri::command]
pub async fn update_snake_points(
    target_index: usize,
    points: Vec<[f64; 2]>,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;

    // Validate that annotation targets exist and index is in range.
    let targets = guard
        .annotation_targets
        .as_ref()
        .ok_or_else(|| "no annotation targets generated".to_string())?;

    if target_index >= targets.len() {
        return Err(format!(
            "target_index {target_index} out of range (0..{})",
            targets.len()
        ));
    }

    guard.snake_contours.insert(target_index, points);
    guard.finalized.insert(target_index, false);

    Ok(())
}

/// Add a control point to the snake contour at the given position.
///
/// The point is inserted at the closest edge of the existing contour.
/// Returns the index of the inserted point.
#[tauri::command]
pub async fn add_snake_point(
    target_index: usize,
    position: [f64; 2],
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<usize, String> {
    let mut guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;

    let contour = guard
        .snake_contours
        .get_mut(&target_index)
        .ok_or_else(|| {
            format!("no snake initialized for target {target_index} — call init_snake first")
        })?;

    let idx = insert_control_point(contour, position);

    // Mark as not finalized since the contour changed.
    guard.finalized.insert(target_index, false);

    Ok(idx)
}

/// Mark a cross-section's contour as finalized.
#[tauri::command]
pub async fn finalize_contour(
    target_index: usize,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;

    // Validate the target exists and has a contour.
    let targets = guard
        .annotation_targets
        .as_ref()
        .ok_or_else(|| "no annotation targets generated".to_string())?;

    if target_index >= targets.len() {
        return Err(format!(
            "target_index {target_index} out of range (0..{})",
            targets.len()
        ));
    }

    if !guard.snake_contours.contains_key(&target_index) {
        return Err(format!(
            "no snake contour for target {target_index} — call init_snake first"
        ));
    }

    guard.finalized.insert(target_index, true);

    Ok(())
}

/// Build a 3D ROI mask from all finalized contours and run multi-material
/// decomposition on the masked region.
///
/// Returns summary statistics (mean fractions over the mask).
#[tauri::command]
pub async fn run_mmd_on_roi(
    method: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<MmdSummary, String> {
    // Validate method.
    if method != "direct" && method != "pwsqs" {
        return Err(format!(
            "unknown method '{method}': expected 'direct' or 'pwsqs'"
        ));
    }

    // Extract everything we need from state under the lock.
    let (
        targets,
        finalized_contours,
        finalized_indices,
        frame,
        dual_energy,
        volume_shape,
        spacing,
        origin,
    ) = {
        let guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;

        let targets = guard
            .annotation_targets
            .as_ref()
            .ok_or_else(|| "no annotation targets generated".to_string())?;

        // Collect all finalized contours, sorted by target index.
        let mut finalized_pairs: Vec<(usize, Vec<[f64; 2]>)> = Vec::new();
        for (&idx, &is_final) in &guard.finalized {
            if is_final {
                if let Some(contour) = guard.snake_contours.get(&idx) {
                    finalized_pairs.push((idx, contour.clone()));
                }
            }
        }

        if finalized_pairs.is_empty() {
            return Err("no finalized contours — finalize at least one cross-section".into());
        }

        // Sort by target index (ascending) as required by build_3d_roi_mask.
        finalized_pairs.sort_by_key(|(idx, _)| *idx);

        let finalized_contours: Vec<Vec<[f64; 2]>> =
            finalized_pairs.iter().map(|(_, c)| c.clone()).collect();

        // Map target indices to frame column indices.
        let finalized_indices: Vec<usize> = finalized_pairs
            .iter()
            .map(|(idx, _)| targets[*idx].frame_index)
            .collect();

        let frame = guard
            .cpr_frame
            .as_ref()
            .ok_or_else(|| "no CPR frame built".to_string())?;

        let de = guard
            .dual_energy
            .as_ref()
            .ok_or_else(|| "no dual-energy volume loaded".to_string())?;

        let vol = guard
            .volume
            .as_ref()
            .ok_or_else(|| "no volume loaded".to_string())?;

        let volume_shape = [
            vol.data.shape()[0],
            vol.data.shape()[1],
            vol.data.shape()[2],
        ];

        // Get cross-section params from the first target.
        let cs_width_mm = targets[0].width_mm;
        let cs_pixels = targets[0].pixels;

        (
            (cs_width_mm, cs_pixels),
            finalized_contours,
            finalized_indices,
            clone_frame(frame),
            (Arc::clone(&de.low), Arc::clone(&de.high), de.low_energy_kev, de.high_energy_kev),
            volume_shape,
            vol.spacing,
            vol.origin,
        )
    };

    let (cs_width_mm, cs_pixels) = targets;
    let (low_energy, high_energy, low_kev, high_kev) = dual_energy;
    let method_clone = method.clone();
    let app_clone = app.clone();

    // Run on a blocking thread.
    let (mmd_result, summary) = tokio::task::spawn_blocking(move || {
        let _ = app_clone.emit(
            "annotation-progress",
            AnnotationProgress {
                stage: "building_roi_mask".into(),
                progress: 0.1,
            },
        );

        // Build 3D ROI mask.
        let mask = roi::build_3d_roi_mask(
            &finalized_contours,
            &frame,
            &finalized_indices,
            volume_shape,
            spacing,
            origin,
            cs_width_mm,
            cs_pixels,
        );

        let n_voxels = mask.iter().filter(|&&v| v).count();
        if n_voxels == 0 {
            return Err("ROI mask is empty — no voxels inside the finalized contours".to_string());
        }

        let _ = app_clone.emit(
            "annotation-progress",
            AnnotationProgress {
                stage: "decomposing".into(),
                progress: 0.3,
            },
        );

        // Run material decomposition.
        let materials = MaterialLibrary::new(low_kev, high_kev);

        let result = match method_clone.as_str() {
            "direct" => mmd::decompose_volume_direct(&low_energy, &high_energy, &mask, &materials),
            "pwsqs" => {
                let params = PwsqsParams::default();
                let app_for_cb = app_clone.clone();
                let max_iter = params.max_iter;
                let cb = move |iter: usize, _delta: f64| {
                    let progress = 0.3 + 0.6 * (iter as f64 / max_iter as f64);
                    let _ = app_for_cb.emit(
                        "annotation-progress",
                        AnnotationProgress {
                            stage: format!("pwsqs_iter_{iter}"),
                            progress,
                        },
                    );
                };
                mmd::pwsqs_solve(&low_energy, &high_energy, &mask, &materials, &params, Some(&cb))
            }
            _ => unreachable!(),
        };

        let _ = app_clone.emit(
            "annotation-progress",
            AnnotationProgress {
                stage: "computing_stats".into(),
                progress: 0.95,
            },
        );

        // Compute mean fractions over the mask.
        let mask_slice = result.mask.as_slice().unwrap();
        let wf_slice = result.water_frac.as_slice().unwrap();
        let lf_slice = result.lipid_frac.as_slice().unwrap();
        let if_slice = result.iodine_frac.as_slice().unwrap();
        let cf_slice = result.calcium_frac.as_slice().unwrap();

        let mut sum_w = 0.0_f64;
        let mut sum_l = 0.0_f64;
        let mut sum_i = 0.0_f64;
        let mut sum_c = 0.0_f64;

        for idx in 0..mask_slice.len() {
            if mask_slice[idx] {
                sum_w += wf_slice[idx] as f64;
                sum_l += lf_slice[idx] as f64;
                sum_i += if_slice[idx] as f64;
                sum_c += cf_slice[idx] as f64;
            }
        }

        let n = n_voxels as f64;
        let summary = MmdSummary {
            method: method_clone,
            iterations: result.iterations,
            converged: result.converged,
            n_voxels,
            mean_water_frac: sum_w / n,
            mean_lipid_frac: sum_l / n,
            mean_iodine_frac: sum_i / n,
            mean_calcium_frac: sum_c / n,
        };

        Ok((result, summary))
    })
    .await
    .map_err(|e| format!("run_mmd_on_roi task failed: {e}"))??;

    // Store the MmdResult in state.
    {
        let mut guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        guard.mmd_result = Some(mmd_result);
    }

    let _ = app.emit(
        "annotation-progress",
        AnnotationProgress {
            stage: "done".into(),
            progress: 1.0,
        },
    );

    Ok(summary)
}

// ---------------------------------------------------------------------------
// Helper: select material array from MmdResult
// ---------------------------------------------------------------------------

fn select_material_array<'a>(
    mmd_result: &'a MmdResult,
    material: &str,
    unit: &str,
) -> Result<&'a ndarray::Array3<f32>, String> {
    match (material, unit) {
        ("water", "fraction") => Ok(&mmd_result.water_frac),
        ("water", "mass") => Ok(&mmd_result.water_mass),
        ("lipid", "fraction") => Ok(&mmd_result.lipid_frac),
        ("lipid", "mass") => Ok(&mmd_result.lipid_mass),
        ("iodine", "fraction") => Ok(&mmd_result.iodine_frac),
        ("iodine", "mass") => Ok(&mmd_result.iodine_mass),
        ("calcium", "fraction") => Ok(&mmd_result.calcium_frac),
        ("calcium", "mass") => Ok(&mmd_result.calcium_mass),
        ("density", _) => Ok(&mmd_result.total_density),
        _ => Err(format!("unknown material/unit: {material}/{unit}")),
    }
}

// ---------------------------------------------------------------------------
// Surface sampling command
// ---------------------------------------------------------------------------

/// Sample radial-angular surface data from the MMD result.
/// Returns surface data for all finalized cross-sections.
#[tauri::command]
pub async fn sample_surfaces(
    material: String,
    unit: String,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<Vec<CrossSectionSurface>, String> {
    // Extract everything under the lock, then release.
    let (material_map, frame, targets, finalized_contours, spacing, origin) = {
        let guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;

        let mmd_result = guard
            .mmd_result
            .as_ref()
            .ok_or_else(|| "no MMD result — run decomposition first".to_string())?;

        let material_map = select_material_array(mmd_result, &material, &unit)?.clone();

        let targets = guard
            .annotation_targets
            .as_ref()
            .ok_or_else(|| "no annotation targets generated".to_string())?
            .clone();

        // Collect finalized contours.
        let mut finalized_contours: HashMap<usize, Vec<[f64; 2]>> = HashMap::new();
        for (&idx, &is_final) in &guard.finalized {
            if is_final {
                if let Some(contour) = guard.snake_contours.get(&idx) {
                    finalized_contours.insert(idx, contour.clone());
                }
            }
        }

        if finalized_contours.is_empty() {
            return Err("no finalized contours — finalize at least one cross-section".into());
        }

        let frame = guard
            .cpr_frame
            .as_ref()
            .ok_or_else(|| "no CPR frame built".to_string())?;
        let frame = clone_frame(frame);

        let vol = guard
            .volume
            .as_ref()
            .ok_or_else(|| "no volume loaded".to_string())?;

        (
            material_map,
            frame,
            targets,
            finalized_contours,
            vol.spacing,
            vol.origin,
        )
    };

    // Run sampling on a blocking thread.
    let surfaces = tokio::task::spawn_blocking(move || {
        let params = RadialAngularParams::default();
        radial_angular::sample_radial_angular(
            &material_map,
            &frame,
            &targets,
            &finalized_contours,
            spacing,
            origin,
            &params,
        )
    })
    .await
    .map_err(|e| format!("sample_surfaces task failed: {e}"))?;

    Ok(surfaces)
}

// ---------------------------------------------------------------------------
// MMD overlay for a single cross-section
// ---------------------------------------------------------------------------

/// Get the current MMD result for a specific material as a flat array for overlay rendering.
///
/// Extracts the material values for the cross-section slice at the given target,
/// for rendering as a colormap overlay in the editor.
#[tauri::command]
pub async fn get_mmd_overlay(
    target_index: usize,
    material: String,
    unit: String,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<Vec<f32>, String> {
    let guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;

    let mmd_result = guard
        .mmd_result
        .as_ref()
        .ok_or_else(|| "no MMD result — run decomposition first".to_string())?;

    let material_map = select_material_array(mmd_result, &material, &unit)?;

    let targets = guard
        .annotation_targets
        .as_ref()
        .ok_or_else(|| "no annotation targets generated".to_string())?;

    let target = targets
        .get(target_index)
        .ok_or_else(|| format!("target_index {target_index} out of range (0..{})", targets.len()))?;

    let frame = guard
        .cpr_frame
        .as_ref()
        .ok_or_else(|| "no CPR frame built".to_string())?;

    let vol = guard
        .volume
        .as_ref()
        .ok_or_else(|| "no volume loaded".to_string())?;

    let spacing = vol.spacing;
    let origin = vol.origin;

    let frame_idx = target.frame_index;
    if frame_idx >= frame.n_cols() {
        return Err(format!("frame_index {frame_idx} out of range"));
    }

    let pos_mm = frame.positions[frame_idx];
    let normal = frame.normals[frame_idx];
    let binormal = frame.binormals[frame_idx];

    let pixels = target.pixels;
    let width_mm = target.width_mm;
    let inv_spacing = [1.0 / spacing[0], 1.0 / spacing[1], 1.0 / spacing[2]];

    let mut overlay = vec![f32::NAN; pixels * pixels];

    for row in 0..pixels {
        for col in 0..pixels {
            // Convert pixel (row, col) to offset in mm from center.
            // Row direction: row=0 -> +normal, row=pixels-1 -> -normal
            let offset_n = width_mm * (1.0 - 2.0 * row as f64 / (pixels as f64 - 1.0));
            // Col direction: col=0 -> +binormal, col=pixels-1 -> -binormal
            let offset_b = width_mm * (1.0 - 2.0 * col as f64 / (pixels as f64 - 1.0));

            let wz = pos_mm[0] + offset_n * normal[0] + offset_b * binormal[0];
            let wy = pos_mm[1] + offset_n * normal[1] + offset_b * binormal[1];
            let wx = pos_mm[2] + offset_n * normal[2] + offset_b * binormal[2];

            // Convert world coords to voxel indices.
            let vz = (wz - origin[0]) * inv_spacing[0];
            let vy = (wy - origin[1]) * inv_spacing[1];
            let vx = (wx - origin[2]) * inv_spacing[2];

            let val = pcat_pipeline::interp::trilinear(material_map, vz, vy, vx);
            overlay[row * pixels + col] = val;
        }
    }

    Ok(overlay)
}
