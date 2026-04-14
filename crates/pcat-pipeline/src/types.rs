use ndarray::Array3;
use std::sync::Arc;

/// CT volume loaded from DICOM, stored in Rust memory.
/// `data` is wrapped in `Arc` so commands can share it without
/// cloning ~300MB on every render call.
pub struct LoadedVolume {
    pub data: Arc<Array3<f32>>,  // (Z, Y, X) HU values -- shared, not cloned
    pub spacing: [f64; 3],       // [sz, sy, sx] mm
    pub origin: [f64; 3],        // [oz, oy, ox] mm
    pub direction: [f64; 9],     // row-major 3x3
    pub window_center: f64,
    pub window_width: f64,
    pub patient_name: String,
    pub study_description: String,
}
