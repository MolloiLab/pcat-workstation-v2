use ndarray::Array3;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::pipeline::cpr::CprFrame;

/// CT volume loaded from DICOM, stored in Rust memory.
/// `data` is wrapped in `Arc` so commands can share it without
/// cloning ~300MB on every render call.
pub struct LoadedVolume {
    pub data: Arc<Array3<f32>>,  // (Z, Y, X) HU values — shared, not cloned
    pub spacing: [f64; 3],       // [sz, sy, sx] mm
    pub origin: [f64; 3],        // [oz, oy, ox] mm
    pub direction: [f64; 9],     // row-major 3x3
    pub window_center: f64,
    pub window_width: f64,
    pub patient_name: String,
    pub study_description: String,
}

/// A set of co-registered mono-energetic CT volumes.
pub struct MonoVolumes {
    /// (keV, volume) pairs, sorted by keV.
    pub volumes: Vec<(f64, Arc<Array3<f32>>)>,
    pub spacing: [f64; 3],
    pub origin: [f64; 3],
    pub direction: [f64; 9],
}

/// Stored MMD decomposition result.
pub struct StoredMmdResult {
    pub water: Arc<Array3<f32>>,
    pub lipid: Arc<Array3<f32>>,
    pub iodine: Arc<Array3<f32>>,
    pub residual: Arc<Array3<f32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Vessel {
    LAD,
    LCx,
    RCA,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResults {
    pub vessels: HashMap<Vessel, VesselResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VesselResult {
    pub fai_mean_hu: f64,
    pub fai_risk: String,
    pub fat_fraction: f64,
    pub n_voi_voxels: usize,
    pub n_fat_voxels: usize,
    pub hu_std: f64,
    pub hu_median: f64,
    pub histogram_bins: Vec<f64>,
    pub histogram_counts: Vec<usize>,
    pub radial_profile: Option<crate::pipeline::stats::RadialProfile>,
    pub angular_asymmetry: Option<crate::pipeline::stats::AngularAsymmetry>,
}

/// Application state managed by Tauri.
pub struct AppState {
    pub volume: Option<LoadedVolume>,
    pub cpr_frame: Option<Arc<CprFrame>>,
    pub analysis_results: Option<AnalysisResults>,
    pub mono_volumes: Option<MonoVolumes>,
    pub mmd_result: Option<StoredMmdResult>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            volume: None,
            cpr_frame: None,
            analysis_results: None,
            mono_volumes: None,
            mmd_result: None,
        }
    }
}
