use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use pcat_pipeline::annotation::AnnotationTarget;
use pcat_pipeline::cpr::CprFrame;
use pcat_pipeline::dicom_loader::DualEnergyVolume;
use pcat_pipeline::mmd::MmdResult;
pub use pcat_pipeline::types::LoadedVolume;

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
    pub radial_profile: Option<pcat_pipeline::stats::RadialProfile>,
    pub angular_asymmetry: Option<pcat_pipeline::stats::AngularAsymmetry>,
}

/// Application state managed by Tauri.
pub struct AppState {
    pub volume: Option<LoadedVolume>,
    pub dual_energy: Option<DualEnergyVolume>,
    pub cpr_frame: Option<Arc<CprFrame>>,
    pub analysis_results: Option<AnalysisResults>,
    /// Annotation targets (cross-section images + vessel walls + init boundaries).
    pub annotation_targets: Option<Vec<AnnotationTarget>>,
    /// Current snake contours per target index.
    pub snake_contours: HashMap<usize, Vec<[f64; 2]>>,
    /// Whether each target's contour is finalized.
    pub finalized: HashMap<usize, bool>,
    /// Most recent MMD decomposition result.
    pub mmd_result: Option<MmdResult>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            volume: None,
            dual_energy: None,
            cpr_frame: None,
            analysis_results: None,
            annotation_targets: None,
            snake_contours: HashMap::new(),
            finalized: HashMap::new(),
            mmd_result: None,
        }
    }
}
