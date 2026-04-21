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

/// Convert a patient-frame position (ZYX order, mm) into continuous voxel
/// indices (vz, vy, vx) using the volume's `origin`, `spacing`, and
/// `direction` (DICOM IOP).
///
/// `direction` is row-major 3×3 with rows `[iop_row, iop_col, slice_normal]` —
/// each row is a unit vector in patient XYZ. Because it is orthonormal, the
/// inverse is the transpose, which collapses to the three dot products below.
///
/// For axial CT with IOP `[1,0,0,0,1,0]` (the common case) this produces
/// bit-identical results to the previous component-wise `(s − o) / spacing`
/// because the matrix-vector product picks each component unchanged.
#[inline]
pub fn patient_to_voxel(
    sample_zyx: [f64; 3],
    origin: [f64; 3],
    inv_spacing: [f64; 3],
    direction: &[f64; 9],
) -> [f64; 3] {
    // Displacement in patient XYZ order (LPS), reversed from ZYX storage.
    let dx = sample_zyx[2] - origin[2];
    let dy = sample_zyx[1] - origin[1];
    let dz = sample_zyx[0] - origin[0];

    // Each row of `direction` is a unit IOP basis vector in patient XYZ.
    // Dot(row, [dx, dy, dz]) gives the displacement component along that
    // basis: row 0 → voxel-X axis, row 1 → voxel-Y axis, row 2 → voxel-Z axis.
    let vx_mm = direction[0] * dx + direction[1] * dy + direction[2] * dz;
    let vy_mm = direction[3] * dx + direction[4] * dy + direction[5] * dz;
    let vz_mm = direction[6] * dx + direction[7] * dy + direction[8] * dz;

    [vz_mm * inv_spacing[0], vy_mm * inv_spacing[1], vx_mm * inv_spacing[2]]
}

/// Identity direction matrix (axial CT default).  Useful for callers that do
/// not yet thread `direction` and want explicit-identity behavior.
pub const IDENTITY_DIRECTION: [f64; 9] = [
    1.0, 0.0, 0.0,
    0.0, 1.0, 0.0,
    0.0, 0.0, 1.0,
];
