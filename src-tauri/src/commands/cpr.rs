use std::sync::Mutex;

use base64::Engine;

use crate::pipeline::cpr;
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

/// Compute a Curved Planar Reformation image from the loaded volume.
///
/// - `centerline_mm`: Dense centerline points in [z, y, x] mm.
/// - `rotation_deg`: Rotational CPR viewing angle in degrees.
/// - `width_mm`: Half-width of lateral axis in mm (default ~25.0).
/// - `slab_mm`: MIP slab thickness in mm (default ~3.0).
/// - `pixels_wide`: Output width (arc-length axis / columns).
/// - `pixels_high`: Output height (lateral axis / rows).
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

    // Extract what we need from state under the lock, then drop it.
    let (volume_data, spacing, origin) = {
        let guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        let vol = guard
            .volume
            .as_ref()
            .ok_or_else(|| "no volume loaded".to_string())?;
        (vol.data.clone(), vol.spacing, vol.origin)
    };

    // Run the heavy computation on a blocking thread
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

    // Encode image as base64 f32 LE bytes
    let bytes: &[u8] = bytemuck::cast_slice(&result.image);
    let image_base64 = base64::engine::general_purpose::STANDARD.encode(bytes);

    Ok(CprCommandResult {
        image_base64,
        shape: [result.pixels_high, result.pixels_wide],
        arclengths: result.arclengths,
    })
}

/// Compute a cross-sectional image perpendicular to the centerline.
///
/// - `centerline_mm`: Dense centerline points in [z, y, x] mm.
/// - `position_fraction`: Fractional position along the centerline [0.0, 1.0].
/// - `rotation_deg`: Rotational CPR angle in degrees.
/// - `width_mm`: Half-width of the cross-section in mm.
/// - `pixels`: Output square image size.
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

    // Extract from state
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

/// Batch-compute multiple cross-sections in a single IPC call, sharing the
/// centerline resampling and Bishop frame computation.
///
/// - `centerline_mm`: Dense centerline points in [z, y, x] mm.
/// - `position_fractions`: Array of fractional positions along the centerline [0.0, 1.0].
/// - `rotation_deg`: Rotational CPR angle in degrees.
/// - `width_mm`: Half-width of each cross-section in mm.
/// - `pixels`: Output square image size.
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

    // Extract from state
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
