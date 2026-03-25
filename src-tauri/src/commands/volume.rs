use std::sync::Mutex;

use ndarray::s;

use crate::state::AppState;

/// Extracts a single 2D slice from the loaded volume as raw i16 little-endian bytes.
///
/// - `axis = "axial"`    -> slice along Z: `data.slice(s![z, .., ..])`
/// - `axis = "coronal"`  -> slice along Y: `data.slice(s![.., y, ..])`
/// - `axis = "sagittal"` -> slice along X: `data.slice(s![.., .., x])`
#[tauri::command]
pub async fn get_slice(
    axis: String,
    idx: usize,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<Vec<u8>, String> {
    // Lock, extract slice data, drop lock promptly.
    let i16_values = {
        let guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        let volume = guard
            .volume
            .as_ref()
            .ok_or_else(|| "no volume loaded".to_string())?;

        let shape = volume.data.shape();
        let (nz, ny, nx) = (shape[0], shape[1], shape[2]);

        let slice_2d = match axis.as_str() {
            "axial" => {
                if idx >= nz {
                    return Err(format!("axial index {idx} out of range (0..{nz})"));
                }
                volume.data.slice(s![idx, .., ..]).to_owned()
            }
            "coronal" => {
                if idx >= ny {
                    return Err(format!("coronal index {idx} out of range (0..{ny})"));
                }
                volume.data.slice(s![.., idx, ..]).to_owned()
            }
            "sagittal" => {
                if idx >= nx {
                    return Err(format!("sagittal index {idx} out of range (0..{nx})"));
                }
                volume.data.slice(s![.., .., idx]).to_owned()
            }
            other => {
                return Err(format!(
                    "unknown axis \"{other}\": expected \"axial\", \"coronal\", or \"sagittal\""
                ));
            }
        };

        // Convert f32 -> i16 (round + clamp to i16 range)
        slice_2d
            .iter()
            .map(|&v| {
                let rounded = v.round();
                let clamped = rounded.clamp(i16::MIN as f32, i16::MAX as f32);
                clamped as i16
            })
            .collect::<Vec<i16>>()
    };
    // Guard is dropped here.

    // Convert i16 slice to raw little-endian bytes via bytemuck.
    let bytes: &[u8] = bytemuck::cast_slice(&i16_values);
    Ok(bytes.to_vec())
}
