//! Multi-Material Decomposition (MMD) for photon-counting CT mono-energetic data.
//!
//! Decomposes each voxel into volume fractions of water, lipid, and iodine
//! using noise-variance-weighted least squares with simplex projection.
//!
//! Uses a **reduced 2-variable formulation** in HU space:
//!   HU_voxel(E) = x_lipid * HU_lipid(E) + x_iodine * HU_iodine(E)
//!   x_water = 1 - x_lipid - x_iodine
//!
//! This avoids the singularity of the 3-material formulation in HU space
//! (water = 0 HU at all energies → degenerate column in A₀).

use nalgebra::{Matrix2, Matrix2x4, Matrix4x2, Vector2, Vector4};
use ndarray::Array3;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Number of energy levels.
const N_ENERGIES: usize = 4;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// MMD configuration.
///
/// The user provides HU values for lipid and iodine reference materials
/// at each mono-energy level (measured from ROIs in their own images).
/// Water is implicit (HU_water = 0 at all energies).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MmdConfig {
    /// HU of pure lipid (fat) reference at each energy [70, 100, 140, 150 keV].
    /// Measure from a subcutaneous fat ROI in each mono-energetic image.
    /// Typical values: [-95, -85, -78, -75].
    pub lipid_hu: [f64; N_ENERGIES],

    /// HU of iodine reference at each energy [70, 100, 140, 150 keV].
    /// Measure from contrast-enhanced vessel lumen ROI in each mono image.
    /// Typical values: [300, 150, 60, 50] (depends on contrast concentration).
    pub iodine_hu: [f64; N_ENERGIES],

    /// Per-energy noise variance (HU²), estimated from a uniform ROI.
    /// Used as diagonal of the weight matrix V.
    pub noise_variances: [f64; N_ENERGIES],

    /// HU upper bound for pre-filtering (skip voxels above this at 70 keV).
    /// Default: 500.0 (excludes bone/calcification).
    #[serde(default = "default_hu_upper")]
    pub hu_upper: f64,

    /// HU lower bound for pre-filtering (skip voxels below this at 70 keV).
    /// Default: -500.0 (excludes air/lung).
    #[serde(default = "default_hu_lower")]
    pub hu_lower: f64,
}

fn default_hu_upper() -> f64 {
    500.0
}
fn default_hu_lower() -> f64 {
    -500.0
}

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

/// Output of multi-material decomposition: per-voxel volume fraction maps.
pub struct MmdResult {
    /// Water volume fraction (0..1).
    pub water: Array3<f32>,
    /// Lipid volume fraction (0..1).
    pub lipid: Array3<f32>,
    /// Iodine volume fraction (0..1).
    pub iodine: Array3<f32>,
    /// L2 fitting residual per voxel: ‖Ax - HU‖₂.
    pub residual: Array3<f32>,
}

// ---------------------------------------------------------------------------
// Precomputed WLS matrix (reduced 2-variable formulation)
// ---------------------------------------------------------------------------

/// Precomputed WLS projector for the reduced system:
///   HU(E) = x_lipid * HU_lipid(E) + x_iodine * HU_iodine(E)
///
/// A is 4×2, P = (AᵀV⁻¹A)⁻¹AᵀV⁻¹ is 2×4.
struct WlsProjector {
    /// 2×4 projection matrix: maps HU vector → [x_lipid, x_iodine].
    p: Matrix2x4<f64>,
    /// 4×2 composition matrix (for residual computation).
    a: Matrix4x2<f64>,
}

impl WlsProjector {
    fn new(config: &MmdConfig) -> Self {
        // Build 4×2 composition matrix A.
        // Column 0: lipid HU at each energy.
        // Column 1: iodine HU at each energy.
        let a = Matrix4x2::new(
            config.lipid_hu[0], config.iodine_hu[0],
            config.lipid_hu[1], config.iodine_hu[1],
            config.lipid_hu[2], config.iodine_hu[2],
            config.lipid_hu[3], config.iodine_hu[3],
        );

        // Build V⁻¹ (inverse noise variances).
        let mut v_inv = [0.0_f64; N_ENERGIES];
        for i in 0..N_ENERGIES {
            let var = config.noise_variances[i];
            v_inv[i] = if var > 1e-12 { 1.0 / var } else { 1.0 };
        }

        // Compute AᵀV⁻¹ (2×4) by scaling columns of Aᵀ.
        let at = a.transpose(); // 2×4
        let mut at_vinv = at;
        for col in 0..N_ENERGIES {
            for row in 0..2 {
                at_vinv[(row, col)] *= v_inv[col];
            }
        }

        // Compute (AᵀV⁻¹A) — a 2×2 matrix.
        let ata: Matrix2<f64> = at_vinv * a;

        // Invert the 2×2 matrix.
        let ata_inv = ata
            .try_inverse()
            .expect("Matrix AᵀV⁻¹A is singular — check lipid_hu and iodine_hu");

        // P = (AᵀV⁻¹A)⁻¹ · AᵀV⁻¹  (2×4)
        let p: Matrix2x4<f64> = ata_inv * at_vinv;

        Self { p, a }
    }

    /// Solve for [x_lipid, x_iodine] given a 4×1 HU measurement vector.
    #[inline]
    fn solve(&self, hu: &Vector4<f64>) -> Vector2<f64> {
        self.p * hu
    }

    /// Compute fitting residual ‖Ax - HU‖₂.
    #[inline]
    fn residual(&self, x: &Vector2<f64>, hu: &Vector4<f64>) -> f64 {
        (self.a * x - hu).norm()
    }
}

// ---------------------------------------------------------------------------
// Simplex projection (Duchi et al., 2008)
// ---------------------------------------------------------------------------

/// Project a 3-vector onto the probability simplex {x : Σxᵢ = 1, xᵢ ≥ 0}.
#[inline]
fn project_simplex(y: &mut [f64; 3]) {
    let mut sorted = *y;
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap());

    let mut cumsum = 0.0;
    let mut rho = 0;
    for j in 0..3 {
        cumsum += sorted[j];
        if sorted[j] + (1.0 - cumsum) / (j as f64 + 1.0) > 0.0 {
            rho = j;
        }
    }

    let tau = (sorted[..=rho].iter().sum::<f64>() - 1.0) / (rho as f64 + 1.0);
    for v in y.iter_mut() {
        *v = (*v - tau).max(0.0);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Perform multi-material decomposition on 4 mono-energetic CT volumes (in HU).
///
/// Uses the reduced formulation:
///   HU(E) = x_lipid * HU_lipid(E) + x_iodine * HU_iodine(E)
///   x_water = 1 - x_lipid - x_iodine
///
/// This works directly in HU space — no unit conversion needed.
pub fn decompose(
    volumes: [&Array3<f32>; N_ENERGIES],
    config: &MmdConfig,
    progress: impl Fn(f64) + Send + Sync,
) -> MmdResult {
    let shape = volumes[0].raw_dim();
    let (nz, ny, nx) = (shape[0], shape[1], shape[2]);
    let n_pixels = nz * ny * nx;

    let proj = WlsProjector::new(config);

    let mut water_flat = vec![0.0_f32; n_pixels];
    let mut lipid_flat = vec![0.0_f32; n_pixels];
    let mut iodine_flat = vec![0.0_f32; n_pixels];
    let mut residual_flat = vec![0.0_f32; n_pixels];

    let v0 = volumes[0].as_slice().expect("contiguous array");
    let v1 = volumes[1].as_slice().expect("contiguous array");
    let v2 = volumes[2].as_slice().expect("contiguous array");
    let v3 = volumes[3].as_slice().expect("contiguous array");

    let hu_upper = config.hu_upper as f32;
    let hu_lower = config.hu_lower as f32;

    let chunk_size = (n_pixels / 100).max(1024);
    let n_chunks = (n_pixels + chunk_size - 1) / chunk_size;
    let chunks_done = std::sync::atomic::AtomicUsize::new(0);

    water_flat
        .par_chunks_mut(chunk_size)
        .zip(lipid_flat.par_chunks_mut(chunk_size))
        .zip(iodine_flat.par_chunks_mut(chunk_size))
        .zip(residual_flat.par_chunks_mut(chunk_size))
        .enumerate()
        .for_each(|(chunk_idx, (((w_chunk, l_chunk), i_chunk), r_chunk))| {
            let start = chunk_idx * chunk_size;
            for local in 0..w_chunk.len() {
                let idx = start + local;

                // Pre-filter on 70 keV image.
                let m0 = v0[idx];
                if m0 > hu_upper || m0 < hu_lower {
                    continue;
                }

                // Gather HU measurement vector.
                let hu = Vector4::new(
                    m0 as f64,
                    v1[idx] as f64,
                    v2[idx] as f64,
                    v3[idx] as f64,
                );

                // Solve reduced 2-variable system for [x_lipid, x_iodine].
                let x2 = proj.solve(&hu);
                let x_lipid = x2[0];
                let x_iodine = x2[1];
                let x_water = 1.0 - x_lipid - x_iodine;

                // Simplex projection to enforce Σ=1, all ≥ 0.
                let mut x = [x_water, x_lipid, x_iodine];
                project_simplex(&mut x);

                w_chunk[local] = x[0] as f32;
                l_chunk[local] = x[1] as f32;
                i_chunk[local] = x[2] as f32;

                // Residual: how well [x_lipid, x_iodine] fits the HU data.
                let x2_proj = Vector2::new(x[1] as f64, x[2] as f64);
                r_chunk[local] = proj.residual(&x2_proj, &hu) as f32;
            }

            let done = chunks_done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            progress(done as f64 / n_chunks as f64);
        });

    let water = Array3::from_shape_vec((nz, ny, nx), water_flat).unwrap();
    let lipid = Array3::from_shape_vec((nz, ny, nx), lipid_flat).unwrap();
    let iodine = Array3::from_shape_vec((nz, ny, nx), iodine_flat).unwrap();
    let residual = Array3::from_shape_vec((nz, ny, nx), residual_flat).unwrap();

    MmdResult { water, lipid, iodine, residual }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array3;

    fn test_config() -> MmdConfig {
        MmdConfig {
            // Typical HU values for fat and iodine-enhanced tissue.
            lipid_hu: [-95.0, -85.0, -78.0, -75.0],
            iodine_hu: [300.0, 150.0, 60.0, 50.0],
            noise_variances: [100.0, 100.0, 100.0, 100.0],
            hu_upper: 500.0,
            hu_lower: -500.0,
        }
    }

    #[test]
    fn test_pure_water() {
        let config = test_config();
        let shape = (1, 1, 1);
        // Water = 0 HU at all energies.
        let v0 = Array3::from_elem(shape, 0.0_f32);
        let v1 = Array3::from_elem(shape, 0.0_f32);
        let v2 = Array3::from_elem(shape, 0.0_f32);
        let v3 = Array3::from_elem(shape, 0.0_f32);

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});

        let w = result.water[[0, 0, 0]];
        let l = result.lipid[[0, 0, 0]];
        let i = result.iodine[[0, 0, 0]];

        assert!((w - 1.0).abs() < 0.01, "water should be ~1.0, got {w}");
        assert!(l.abs() < 0.01, "lipid should be ~0.0, got {l}");
        assert!(i.abs() < 0.01, "iodine should be ~0.0, got {i}");
    }

    #[test]
    fn test_pure_lipid() {
        let config = test_config();
        let shape = (1, 1, 1);
        // Pure fat: HU = lipid_hu at each energy.
        let v0 = Array3::from_elem(shape, -95.0_f32);
        let v1 = Array3::from_elem(shape, -85.0_f32);
        let v2 = Array3::from_elem(shape, -78.0_f32);
        let v3 = Array3::from_elem(shape, -75.0_f32);

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});

        let w = result.water[[0, 0, 0]];
        let l = result.lipid[[0, 0, 0]];
        let i = result.iodine[[0, 0, 0]];

        assert!(w.abs() < 0.01, "water should be ~0, got {w}");
        assert!((l - 1.0).abs() < 0.01, "lipid should be ~1.0, got {l}");
        assert!(i.abs() < 0.01, "iodine should be ~0, got {i}");
    }

    #[test]
    fn test_pure_iodine() {
        let config = test_config();
        let shape = (1, 1, 1);
        // Pure iodine reference: HU = iodine_hu at each energy.
        let v0 = Array3::from_elem(shape, 300.0_f32);
        let v1 = Array3::from_elem(shape, 150.0_f32);
        let v2 = Array3::from_elem(shape, 60.0_f32);
        let v3 = Array3::from_elem(shape, 50.0_f32);

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});

        let w = result.water[[0, 0, 0]];
        let l = result.lipid[[0, 0, 0]];
        let i = result.iodine[[0, 0, 0]];

        assert!(w.abs() < 0.01, "water should be ~0, got {w}");
        assert!(l.abs() < 0.01, "lipid should be ~0, got {l}");
        assert!((i - 1.0).abs() < 0.01, "iodine should be ~1.0, got {i}");
    }

    #[test]
    fn test_mixed_water_lipid() {
        let config = test_config();
        let shape = (1, 1, 1);
        // 50% water + 50% lipid: HU = 0.5*0 + 0.5*lipid_hu
        let v0 = Array3::from_elem(shape, -47.5_f32);
        let v1 = Array3::from_elem(shape, -42.5_f32);
        let v2 = Array3::from_elem(shape, -39.0_f32);
        let v3 = Array3::from_elem(shape, -37.5_f32);

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});

        let w = result.water[[0, 0, 0]];
        let l = result.lipid[[0, 0, 0]];
        let i = result.iodine[[0, 0, 0]];

        assert!((w - 0.5).abs() < 0.05, "water should be ~0.5, got {w}");
        assert!((l - 0.5).abs() < 0.05, "lipid should be ~0.5, got {l}");
        assert!(i.abs() < 0.05, "iodine should be ~0, got {i}");
    }

    #[test]
    fn test_mixed_water_iodine() {
        let config = test_config();
        let shape = (1, 1, 1);
        // 80% water + 20% iodine: HU = 0.2*iodine_hu
        let v0 = Array3::from_elem(shape, 60.0_f32);    // 0.2*300
        let v1 = Array3::from_elem(shape, 30.0_f32);    // 0.2*150
        let v2 = Array3::from_elem(shape, 12.0_f32);    // 0.2*60
        let v3 = Array3::from_elem(shape, 10.0_f32);    // 0.2*50

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});

        let w = result.water[[0, 0, 0]];
        let l = result.lipid[[0, 0, 0]];
        let i = result.iodine[[0, 0, 0]];

        assert!((w - 0.8).abs() < 0.05, "water should be ~0.8, got {w}");
        assert!(l.abs() < 0.05, "lipid should be ~0, got {l}");
        assert!((i - 0.2).abs() < 0.05, "iodine should be ~0.2, got {i}");
    }

    #[test]
    fn test_simplex_projection() {
        let mut x = [0.3, 0.5, 0.2];
        project_simplex(&mut x);
        assert!((x[0] + x[1] + x[2] - 1.0).abs() < 1e-10);

        let mut x2 = [1.5, -0.3, -0.2];
        project_simplex(&mut x2);
        assert!(x2.iter().all(|&v| v >= 0.0));
        assert!((x2[0] + x2[1] + x2[2] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_prefilter_skips_out_of_range() {
        let config = test_config();
        let shape = (1, 1, 2);
        let v0 = Array3::from_shape_vec(shape, vec![0.0, 1000.0]).unwrap();
        let v1 = Array3::from_shape_vec(shape, vec![0.0, 1000.0]).unwrap();
        let v2 = Array3::from_shape_vec(shape, vec![0.0, 1000.0]).unwrap();
        let v3 = Array3::from_shape_vec(shape, vec![0.0, 1000.0]).unwrap();

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});

        assert!(result.water[[0, 0, 0]] > 0.9, "in-range pixel should decompose");
        assert_eq!(result.water[[0, 0, 1]], 0.0, "out-of-range should be skipped");
    }
}
