use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

use crate::pipeline::dicom_loader::load_dicom_directory;
use crate::pipeline::mmd::{self, MmdConfig};
use crate::state::{AppState, MonoVolumes, StoredMmdResult};

// ---------------------------------------------------------------------------
// Types returned to the frontend
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct MonoVolumeInfo {
    pub energies: Vec<f64>,
    pub shape: [usize; 3],
    pub spacing: [f64; 3],
}

#[derive(Debug, Serialize)]
pub struct MmdSummary {
    pub shape: [usize; 3],
    pub elapsed_ms: u64,
    /// Per-material mean volume fraction across all non-filtered voxels.
    pub mean_water: f64,
    pub mean_lipid: f64,
    pub mean_iodine: f64,
    pub mean_residual: f64,
}

#[derive(Debug, Deserialize)]
pub struct MmdRunConfig {
    pub basis_lacs: [[f64; 3]; 4],
    pub noise_variances: [f64; 4],
    #[serde(default = "default_hu_upper")]
    pub hu_upper: f64,
    #[serde(default = "default_hu_lower")]
    pub hu_lower: f64,
}

fn default_hu_upper() -> f64 { 150.0 }
fn default_hu_lower() -> f64 { -500.0 }

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Load mono-energetic DICOM volumes from multiple directories.
///
/// `paths`: map of energy label → directory path, e.g. {"70": "/path/to/70keV", ...}
///
/// All volumes must have the same dimensions. They are stored in AppState.
#[tauri::command]
pub async fn load_mono_volumes(
    paths: HashMap<String, String>,
    state: State<'_, Mutex<AppState>>,
) -> Result<MonoVolumeInfo, String> {
    if paths.is_empty() {
        return Err("No paths provided".to_string());
    }

    // Parse energy labels and sort.
    let mut entries: Vec<(f64, String)> = paths
        .into_iter()
        .map(|(label, path)| {
            let kev: f64 = label
                .parse()
                .map_err(|_| format!("Invalid energy label: {label}"))?;
            Ok((kev, path))
        })
        .collect::<Result<Vec<_>, String>>()?;
    entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    // Load each volume.
    let mut volumes: Vec<(f64, Arc<ndarray::Array3<f32>>)> = Vec::new();
    let mut ref_shape: Option<[usize; 3]> = None;
    let mut spacing = [1.0; 3];
    let mut origin = [0.0; 3];
    let mut direction = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

    for (kev, path) in &entries {
        let loaded = load_dicom_directory(std::path::Path::new(path))
            .map_err(|e| format!("Failed to load {kev} keV from {path}: {e}"))?;

        let shape = loaded.data.raw_dim();
        let this_shape = [shape[0], shape[1], shape[2]];

        if let Some(ref_s) = ref_shape {
            if this_shape != ref_s {
                return Err(format!(
                    "Shape mismatch: {kev} keV is {:?}, expected {:?}",
                    this_shape, ref_s
                ));
            }
        } else {
            ref_shape = Some(this_shape);
            spacing = loaded.spacing;
            origin = loaded.origin;
            direction = loaded.direction;
        }

        volumes.push((*kev, loaded.data));
    }

    let shape = ref_shape.unwrap();
    let energies: Vec<f64> = volumes.iter().map(|(k, _)| *k).collect();

    // Store in app state.
    let mono = MonoVolumes {
        volumes,
        spacing,
        origin,
        direction,
    };

    {
        let mut st = state.lock().map_err(|e| e.to_string())?;
        st.mono_volumes = Some(mono);
        st.mmd_result = None; // clear previous result
    }

    Ok(MonoVolumeInfo {
        energies,
        shape,
        spacing,
    })
}

/// Run multi-material decomposition on loaded mono-energetic volumes.
#[tauri::command]
pub async fn run_mmd(
    config: MmdRunConfig,
    app: tauri::AppHandle,
    state: State<'_, Mutex<AppState>>,
) -> Result<MmdSummary, String> {
    // Extract volume references from state.
    let volume_arcs: Vec<Arc<ndarray::Array3<f32>>>;
    {
        let st = state.lock().map_err(|e| e.to_string())?;
        let mono = st
            .mono_volumes
            .as_ref()
            .ok_or("No mono-energetic volumes loaded")?;

        if mono.volumes.len() < 4 {
            return Err(format!(
                "Need 4 mono-energetic volumes, got {}",
                mono.volumes.len()
            ));
        }

        volume_arcs = mono.volumes.iter().map(|(_, v)| Arc::clone(v)).collect();
    }

    let mmd_config = MmdConfig {
        basis_lacs: config.basis_lacs,
        noise_variances: config.noise_variances,
        hu_upper: config.hu_upper,
        hu_lower: config.hu_lower,
    };

    // Run decomposition (CPU-bound, uses Rayon internally).
    let start = std::time::Instant::now();

    let result = tokio::task::spawn_blocking(move || {
        let refs: [&ndarray::Array3<f32>; 4] = [
            &volume_arcs[0],
            &volume_arcs[1],
            &volume_arcs[2],
            &volume_arcs[3],
        ];

        mmd::decompose(refs, &mmd_config, |frac| {
            let _ = app.emit("mmd-progress", frac);
        })
    })
    .await
    .map_err(|e| format!("MMD task failed: {e}"))?;

    let elapsed_ms = start.elapsed().as_millis() as u64;

    // Compute summary statistics.
    let shape = result.water.raw_dim();
    let n = (shape[0] * shape[1] * shape[2]) as f64;
    let mean_water = result.water.iter().map(|&v| v as f64).sum::<f64>() / n;
    let mean_lipid = result.lipid.iter().map(|&v| v as f64).sum::<f64>() / n;
    let mean_iodine = result.iodine.iter().map(|&v| v as f64).sum::<f64>() / n;
    let mean_residual = result.residual.iter().map(|&v| v as f64).sum::<f64>() / n;

    let summary = MmdSummary {
        shape: [shape[0], shape[1], shape[2]],
        elapsed_ms,
        mean_water,
        mean_lipid,
        mean_iodine,
        mean_residual,
    };

    // Store result in app state.
    {
        let mut st = state.lock().map_err(|e| e.to_string())?;
        st.mmd_result = Some(StoredMmdResult {
            water: Arc::new(result.water),
            lipid: Arc::new(result.lipid),
            iodine: Arc::new(result.iodine),
            residual: Arc::new(result.residual),
        });
    }

    Ok(summary)
}

/// Get a 2D slice from one of the MMD material fraction maps.
///
/// `material`: "water", "lipid", "iodine", or "residual"
/// `axis`: "axial", "coronal", or "sagittal"
/// `idx`: slice index along the given axis
///
/// Returns raw f32 bytes (little-endian), row-major.
#[tauri::command]
pub async fn get_mmd_slice(
    material: String,
    axis: String,
    idx: usize,
    state: State<'_, Mutex<AppState>>,
) -> Result<Vec<u8>, String> {
    let st = state.lock().map_err(|e| e.to_string())?;
    let mmd = st
        .mmd_result
        .as_ref()
        .ok_or("No MMD result available")?;

    let volume = match material.as_str() {
        "water" => &mmd.water,
        "lipid" => &mmd.lipid,
        "iodine" => &mmd.iodine,
        "residual" => &mmd.residual,
        _ => return Err(format!("Unknown material: {material}")),
    };

    let shape = volume.raw_dim();
    let (nz, ny, nx) = (shape[0], shape[1], shape[2]);

    let slice_data: Vec<f32> = match axis.as_str() {
        "axial" => {
            if idx >= nz {
                return Err(format!("Axial index {idx} out of range (max {})", nz - 1));
            }
            volume.slice(ndarray::s![idx, .., ..]).iter().copied().collect()
        }
        "coronal" => {
            if idx >= ny {
                return Err(format!("Coronal index {idx} out of range (max {})", ny - 1));
            }
            volume.slice(ndarray::s![.., idx, ..]).iter().copied().collect()
        }
        "sagittal" => {
            if idx >= nx {
                return Err(format!("Sagittal index {idx} out of range (max {})", nx - 1));
            }
            volume.slice(ndarray::s![.., .., idx]).iter().copied().collect()
        }
        _ => return Err(format!("Unknown axis: {axis}")),
    };

    // Convert to bytes.
    let bytes: Vec<u8> = slice_data
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();

    Ok(bytes)
}
