use std::sync::Mutex;

use base64::Engine;

use crate::pipeline::cpr::{self, CprFrame};
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct CprCommandResult {
    /// Base64-encoded f32 little-endian bytes of the CPR image
    pub image_base64: String,
    /// [height, width] of the image
    pub shape: [usize; 2],
    /// Arc-length positions in mm for each column
    pub arclengths: Vec<f64>,
}

#[derive(serde::Serialize)]
pub struct CrossSectionCommandResult {
    /// Base64-encoded f32 little-endian bytes of the cross-section image
    pub image_base64: String,
    /// Size of the square image
    pub pixels: usize,
    /// Arc-length position in mm
    pub arc_mm: f64,
}

/// Build and cache the CPR frame from a centerline.
/// Called once when the centerline changes.
///
/// - `centerline_mm`: Dense centerline points in [z, y, x] mm.
/// - `pixels_wide`: Number of arc-length samples (output columns).
#[tauri::command]
pub async fn build_cpr_frame(
    centerline_mm: Vec<[f64; 3]>,
    pixels_wide: usize,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<(), String> {
    if centerline_mm.len() < 2 {
        return Err("centerline must have at least 2 points".into());
    }
    if pixels_wide < 2 {
        return Err("pixels_wide must be at least 2".into());
    }

    // Build the frame on a blocking thread (spline fitting + Bishop frame)
    let frame = tokio::task::spawn_blocking(move || {
        CprFrame::from_centerline(&centerline_mm, pixels_wide)
    })
    .await
    .map_err(|e| format!("build_cpr_frame task failed: {e}"))?;

    // Store in app state
    let mut guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
    guard.cpr_frame = Some(frame);

    Ok(())
}

/// Render a CPR image using the cached frame. Fast — just rotates + samples.
///
/// - `rotation_deg`: Rotational CPR viewing angle in degrees.
/// - `width_mm`: Half-width of lateral axis in mm.
/// - `pixels_high`: Output height (lateral axis / rows).
/// - `slab_mm`: MIP slab thickness (0.0 for single-plane interactive mode).
#[tauri::command]
pub async fn render_cpr_image(
    rotation_deg: f64,
    width_mm: f64,
    pixels_high: usize,
    slab_mm: f64,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<CprCommandResult, String> {
    if pixels_high < 2 {
        return Err("pixels_high must be at least 2".into());
    }

    // Extract what we need under the lock, then release it
    let (volume_data, spacing, origin, frame) = {
        let guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        let vol = guard
            .volume
            .as_ref()
            .ok_or_else(|| "no volume loaded".to_string())?;
        let frame_ref = guard
            .cpr_frame
            .as_ref()
            .ok_or_else(|| "no CPR frame built — call build_cpr_frame first".to_string())?;

        // We need to clone frame data for the blocking thread.
        // Clone the volume data (shared) and copy frame fields.
        let positions = frame_ref.positions.clone();
        let tangents = frame_ref.tangents.clone();
        let normals = frame_ref.normals.clone();
        let binormals = frame_ref.binormals.clone();
        let arclengths = frame_ref.arclengths.clone();

        (
            vol.data.clone(),
            vol.spacing,
            vol.origin,
            CprFrame {
                positions,
                tangents,
                normals,
                binormals,
                arclengths,
            },
        )
    };

    let result = tokio::task::spawn_blocking(move || {
        frame.render_cpr(
            &volume_data,
            spacing,
            origin,
            rotation_deg,
            width_mm,
            pixels_high,
            slab_mm,
        )
    })
    .await
    .map_err(|e| format!("render_cpr_image task failed: {e}"))?;

    let bytes: &[u8] = bytemuck::cast_slice(&result.image);
    let image_base64 = base64::engine::general_purpose::STANDARD.encode(bytes);

    Ok(CprCommandResult {
        image_base64,
        shape: [result.pixels_high, result.pixels_wide],
        arclengths: result.arclengths,
    })
}

/// Render multiple cross-sections using the cached frame.
///
/// - `position_fractions`: Array of fractional positions along the centerline [0.0, 1.0].
/// - `rotation_deg`: Rotational CPR angle in degrees.
/// - `width_mm`: Half-width of each cross-section in mm.
/// - `pixels`: Output square image size.
#[tauri::command]
pub async fn render_cross_sections(
    position_fractions: Vec<f64>,
    rotation_deg: f64,
    width_mm: f64,
    pixels: usize,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<Vec<CrossSectionCommandResult>, String> {
    if pixels < 2 {
        return Err("output size must be at least 2".into());
    }
    for &frac in &position_fractions {
        if !(0.0..=1.0).contains(&frac) {
            return Err(format!(
                "position_fraction must be in [0, 1], got {frac}"
            ));
        }
    }

    // Extract from state
    let (volume_data, spacing, origin, frame) = {
        let guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        let vol = guard
            .volume
            .as_ref()
            .ok_or_else(|| "no volume loaded".to_string())?;
        let frame_ref = guard
            .cpr_frame
            .as_ref()
            .ok_or_else(|| "no CPR frame built — call build_cpr_frame first".to_string())?;

        (
            vol.data.clone(),
            vol.spacing,
            vol.origin,
            CprFrame {
                positions: frame_ref.positions.clone(),
                tangents: frame_ref.tangents.clone(),
                normals: frame_ref.normals.clone(),
                binormals: frame_ref.binormals.clone(),
                arclengths: frame_ref.arclengths.clone(),
            },
        )
    };

    let results = tokio::task::spawn_blocking(move || {
        frame.render_cross_sections(
            &volume_data,
            spacing,
            origin,
            &position_fractions,
            rotation_deg,
            width_mm,
            pixels,
        )
    })
    .await
    .map_err(|e| format!("render_cross_sections task failed: {e}"))?;

    // Encode each result
    Ok(results
        .into_iter()
        .map(|r| {
            let bytes: &[u8] = bytemuck::cast_slice(&r.image);
            let image_base64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            CrossSectionCommandResult {
                image_base64,
                pixels: r.pixels,
                arc_mm: r.arc_mm,
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Legacy commands — kept for backward compatibility but delegate to new API
// ---------------------------------------------------------------------------

/// Legacy: Compute a CPR image in one call (builds frame + renders).
#[tauri::command]
pub async fn compute_cpr_image(
    centerline_mm: Vec<[f64; 3]>,
    rotation_deg: f64,
    width_mm: f64,
    slab_mm: f64,
    pixels_wide: usize,
    pixels_high: usize,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<CprCommandResult, String> {
    if centerline_mm.len() < 2 {
        return Err("centerline must have at least 2 points".into());
    }
    if pixels_wide < 2 || pixels_high < 2 {
        return Err("output dimensions must be at least 2".into());
    }

    let (volume_data, spacing, origin) = {
        let guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        let vol = guard
            .volume
            .as_ref()
            .ok_or_else(|| "no volume loaded".to_string())?;
        (vol.data.clone(), vol.spacing, vol.origin)
    };

    let result = tokio::task::spawn_blocking(move || {
        cpr::compute_cpr(
            &volume_data,
            &centerline_mm,
            spacing,
            origin,
            width_mm,
            slab_mm,
            pixels_wide,
            pixels_high,
            rotation_deg,
        )
    })
    .await
    .map_err(|e| format!("CPR task failed: {e}"))?;

    let bytes: &[u8] = bytemuck::cast_slice(&result.image);
    let image_base64 = base64::engine::general_purpose::STANDARD.encode(bytes);

    Ok(CprCommandResult {
        image_base64,
        shape: [result.pixels_high, result.pixels_wide],
        arclengths: result.arclengths,
    })
}

/// Legacy: Compute a single cross-section image.
#[tauri::command]
pub async fn compute_cross_section_image(
    centerline_mm: Vec<[f64; 3]>,
    position_fraction: f64,
    rotation_deg: f64,
    width_mm: f64,
    pixels: usize,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<CrossSectionCommandResult, String> {
    if centerline_mm.len() < 2 {
        return Err("centerline must have at least 2 points".into());
    }
    if pixels < 2 {
        return Err("output size must be at least 2".into());
    }
    if !(0.0..=1.0).contains(&position_fraction) {
        return Err(format!(
            "position_fraction must be in [0, 1], got {position_fraction}"
        ));
    }

    let (volume_data, spacing, origin) = {
        let guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        let vol = guard
            .volume
            .as_ref()
            .ok_or_else(|| "no volume loaded".to_string())?;
        (vol.data.clone(), vol.spacing, vol.origin)
    };

    let result = tokio::task::spawn_blocking(move || {
        cpr::compute_cross_section(
            &volume_data,
            &centerline_mm,
            spacing,
            origin,
            position_fraction,
            rotation_deg,
            width_mm,
            pixels,
        )
    })
    .await
    .map_err(|e| format!("cross-section task failed: {e}"))?;

    let bytes: &[u8] = bytemuck::cast_slice(&result.image);
    let image_base64 = base64::engine::general_purpose::STANDARD.encode(bytes);

    Ok(CrossSectionCommandResult {
        image_base64,
        pixels: result.pixels,
        arc_mm: result.arc_mm,
    })
}

/// Legacy: Batch-compute multiple cross-sections.
#[tauri::command]
pub async fn compute_cross_sections_batch(
    centerline_mm: Vec<[f64; 3]>,
    position_fractions: Vec<f64>,
    rotation_deg: f64,
    width_mm: f64,
    pixels: usize,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<Vec<CrossSectionCommandResult>, String> {
    if centerline_mm.len() < 2 {
        return Err("centerline must have at least 2 points".into());
    }
    if pixels < 2 {
        return Err("output size must be at least 2".into());
    }
    for &frac in &position_fractions {
        if !(0.0..=1.0).contains(&frac) {
            return Err(format!(
                "position_fraction must be in [0, 1], got {frac}"
            ));
        }
    }

    let (volume_data, spacing, origin) = {
        let guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        let vol = guard
            .volume
            .as_ref()
            .ok_or_else(|| "no volume loaded".to_string())?;
        (vol.data.clone(), vol.spacing, vol.origin)
    };

    let results = tokio::task::spawn_blocking(move || {
        cpr::compute_cross_sections_batch(
            &volume_data,
            &centerline_mm,
            spacing,
            origin,
            &position_fractions,
            rotation_deg,
            width_mm,
            pixels,
        )
    })
    .await
    .map_err(|e| format!("batch cross-section task failed: {e}"))?;

    Ok(results
        .into_iter()
        .map(|r| {
            let bytes: &[u8] = bytemuck::cast_slice(&r.image);
            let image_base64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            CrossSectionCommandResult {
                image_base64,
                pixels: r.pixels,
                arc_mm: r.arc_mm,
            }
        })
        .collect())
}
