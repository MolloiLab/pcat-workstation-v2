//! Multi-Material Decomposition (MMD) for photon-counting CT mono-energetic data.
//!
//! Decomposes each voxel into volume fractions of water, lipid, and iodine
//! using noise-variance-weighted least squares with simplex projection.
//!
//! References:
//!   - Niu et al., Med Phys 2014 (iterative image-domain decomposition for DECT)
//!   - Xue et al., Med Phys 2017 (statistical image-domain MMD for DECT)
//!   - Xue et al., IEEE TMI 2021 (MMD for SECT with material sparsity)

use nalgebra::{Matrix3, Matrix3x4, Matrix4x3, Vector3, Vector4};
use ndarray::Array3;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Number of basis materials.
const N_MATERIALS: usize = 3;
/// Number of energy levels.
const N_ENERGIES: usize = 4;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// MMD configuration: basis material LACs and noise variances.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MmdConfig {
    /// LACs of basis materials at each energy level.
    /// Outer: 4 energies (70, 100, 140, 150 keV).
    /// Inner: 3 materials (water, lipid, iodine).
    pub basis_lacs: [[f64; N_MATERIALS]; N_ENERGIES],

    /// Per-energy noise variance, estimated from a uniform ROI in each
    /// mono-energetic image.  Used as diagonal of the weight matrix V.
    pub noise_variances: [f64; N_ENERGIES],

    /// HU upper bound for pre-filtering (skip voxels above this).
    /// Default: 150.0 (excludes bone/calcification).
    #[serde(default = "default_hu_upper")]
    pub hu_upper: f64,

    /// HU lower bound for pre-filtering (skip voxels below this).
    /// Default: -500.0 (excludes air/lung).
    #[serde(default = "default_hu_lower")]
    pub hu_lower: f64,
}

fn default_hu_upper() -> f64 {
    150.0
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
    /// L2 fitting residual per voxel: ‖A₀x - μ‖₂.
    pub residual: Array3<f32>,
}

// ---------------------------------------------------------------------------
// Precomputed WLS matrix
// ---------------------------------------------------------------------------

/// Precomputed weighted least-squares projection matrix P = (AᵀV⁻¹A)⁻¹ AᵀV⁻¹.
/// This is a 3×4 matrix that maps a 4×1 measurement vector to a 3×1 volume
/// fraction vector in a single matrix-vector multiply.
struct WlsProjector {
    /// 3×4 projection matrix.
    p: Matrix3x4<f64>,
    /// 4×3 composition matrix (for residual computation).
    a: Matrix4x3<f64>,
}

impl WlsProjector {
    fn new(config: &MmdConfig) -> Self {
        // Build 4×3 composition matrix A₀.
        let a = Matrix4x3::from_rows(&[
            nalgebra::RowVector3::new(
                config.basis_lacs[0][0],
                config.basis_lacs[0][1],
                config.basis_lacs[0][2],
            ),
            nalgebra::RowVector3::new(
                config.basis_lacs[1][0],
                config.basis_lacs[1][1],
                config.basis_lacs[1][2],
            ),
            nalgebra::RowVector3::new(
                config.basis_lacs[2][0],
                config.basis_lacs[2][1],
                config.basis_lacs[2][2],
            ),
            nalgebra::RowVector3::new(
                config.basis_lacs[3][0],
                config.basis_lacs[3][1],
                config.basis_lacs[3][2],
            ),
        ]);

        // Build diagonal weight matrix V⁻¹ (inverse noise variances).
        // We don't form the full 4×4 matrix; instead we scale rows of A.
        let mut v_inv = [0.0_f64; N_ENERGIES];
        for i in 0..N_ENERGIES {
            let var = config.noise_variances[i];
            v_inv[i] = if var > 1e-12 { 1.0 / var } else { 1.0 };
        }

        // Compute AᵀV⁻¹ (3×4) by scaling columns of Aᵀ.
        let at = a.transpose(); // 3×4
        let mut at_vinv = at;
        for col in 0..N_ENERGIES {
            for row in 0..N_MATERIALS {
                at_vinv[(row, col)] *= v_inv[col];
            }
        }

        // Compute (AᵀV⁻¹A) — a 3×3 matrix.
        let ata: Matrix3<f64> = at_vinv * a;

        // Invert the 3×3 matrix.
        let ata_inv = ata
            .try_inverse()
            .expect("Composition matrix A₀ᵀV⁻¹A₀ is singular — check basis LACs");

        // P = (AᵀV⁻¹A)⁻¹ · AᵀV⁻¹  (3×4)
        let p: Matrix3x4<f64> = ata_inv * at_vinv;

        Self { p, a }
    }

    /// Solve for volume fractions given a 4×1 measurement vector.
    #[inline]
    fn solve(&self, mu: &Vector4<f64>) -> Vector3<f64> {
        self.p * mu
    }

    /// Compute fitting residual ‖A₀x - μ‖₂.
    #[inline]
    fn residual(&self, x: &Vector3<f64>, mu: &Vector4<f64>) -> f64 {
        (self.a * x - mu).norm()
    }
}

// ---------------------------------------------------------------------------
// Simplex projection (Duchi et al., 2008)
// ---------------------------------------------------------------------------

/// Project a 3-vector onto the probability simplex {x : Σxᵢ = 1, xᵢ ≥ 0}.
///
/// Duchi, Shalev-Shwartz, Singer, Chandra (ICML 2008).
#[inline]
fn project_simplex(y: &mut [f64; 3]) {
    // Sort descending.
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

/// Perform multi-material decomposition on 4 mono-energetic CT volumes.
///
/// `volumes`: mono-energetic images at 70, 100, 140, 150 keV (same shape).
/// `config`:  basis material LACs and noise variances.
/// `progress`: callback invoked with fraction complete (0.0 → 1.0).
///
/// Returns volume fraction maps for water, lipid, iodine plus a residual map.
pub fn decompose(
    volumes: [&Array3<f32>; N_ENERGIES],
    config: &MmdConfig,
    progress: impl Fn(f64) + Send + Sync,
) -> MmdResult {
    let shape = volumes[0].raw_dim();
    let (nz, ny, nx) = (shape[0], shape[1], shape[2]);
    let n_pixels = nz * ny * nx;

    // Precompute the WLS projection matrix (one-time, microseconds).
    let proj = WlsProjector::new(config);

    // Allocate output arrays (flat, then reshape).
    let mut water_flat = vec![0.0_f32; n_pixels];
    let mut lipid_flat = vec![0.0_f32; n_pixels];
    let mut iodine_flat = vec![0.0_f32; n_pixels];
    let mut residual_flat = vec![0.0_f32; n_pixels];

    // Get raw slices for parallel access.
    let v0 = volumes[0].as_slice().expect("contiguous array");
    let v1 = volumes[1].as_slice().expect("contiguous array");
    let v2 = volumes[2].as_slice().expect("contiguous array");
    let v3 = volumes[3].as_slice().expect("contiguous array");

    let hu_upper = config.hu_upper as f32;
    let hu_lower = config.hu_lower as f32;

    // Process in chunks for progress reporting.
    let chunk_size = (n_pixels / 100).max(1024);
    let n_chunks = (n_pixels + chunk_size - 1) / chunk_size;
    let chunks_done = std::sync::atomic::AtomicUsize::new(0);

    // Parallel decomposition over chunks of pixels.
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

                // Pre-filter: skip voxels outside soft-tissue range.
                let m0 = v0[idx];
                if m0 > hu_upper || m0 < hu_lower {
                    continue; // outputs stay at 0.0
                }

                // Gather measurement vector.
                let mu = Vector4::new(
                    m0 as f64,
                    v1[idx] as f64,
                    v2[idx] as f64,
                    v3[idx] as f64,
                );

                // WLS solve.
                let x_raw = proj.solve(&mu);
                let mut x = [x_raw[0], x_raw[1], x_raw[2]];

                // Simplex projection.
                project_simplex(&mut x);

                let x_vec = Vector3::new(x[0], x[1], x[2]);

                w_chunk[local] = x[0] as f32;
                l_chunk[local] = x[1] as f32;
                i_chunk[local] = x[2] as f32;
                r_chunk[local] = proj.residual(&x_vec, &mu) as f32;
            }

            // Report progress.
            let done = chunks_done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            progress(done as f64 / n_chunks as f64);
        });

    // Reshape flat vecs into Array3.
    let water = Array3::from_shape_vec((nz, ny, nx), water_flat).unwrap();
    let lipid = Array3::from_shape_vec((nz, ny, nx), lipid_flat).unwrap();
    let iodine = Array3::from_shape_vec((nz, ny, nx), iodine_flat).unwrap();
    let residual = Array3::from_shape_vec((nz, ny, nx), residual_flat).unwrap();

    MmdResult {
        water,
        lipid,
        iodine,
        residual,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array3;

    /// Realistic LAC values in mm⁻¹ (from NIST XCOM, approximate).
    /// Using LAC (not HU) because HU is defined relative to water (water=0),
    /// which would make the first column of A₀ all zeros → singular.
    fn test_config() -> MmdConfig {
        MmdConfig {
            // LAC values (mm⁻¹) for water, lipid, iodine at each energy.
            // Water LAC varies with energy (photoelectric + Compton).
            // Iodine has K-edge at 33.2 keV → much higher at 70 keV.
            basis_lacs: [
                [0.0193, 0.0171, 0.0800],  // 70 keV
                [0.0171, 0.0159, 0.0250],  // 100 keV
                [0.0155, 0.0148, 0.0130],  // 140 keV
                [0.0152, 0.0146, 0.0120],  // 150 keV
            ],
            noise_variances: [1e-8, 1e-8, 1e-8, 1e-8],
            hu_upper: 1.0,    // LAC values, not HU
            hu_lower: -1.0,
        }
    }

    #[test]
    fn test_pure_water() {
        let config = test_config();
        let shape = (1, 1, 1);
        // Water LAC at each energy:
        let v0 = Array3::from_elem(shape, 0.0193_f32);
        let v1 = Array3::from_elem(shape, 0.0171_f32);
        let v2 = Array3::from_elem(shape, 0.0155_f32);
        let v3 = Array3::from_elem(shape, 0.0152_f32);

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});

        let w = result.water[[0, 0, 0]];
        let l = result.lipid[[0, 0, 0]];
        let i = result.iodine[[0, 0, 0]];

        assert!((w - 1.0).abs() < 0.01, "water should be ~1.0, got {w}");
        assert!(l.abs() < 0.01, "lipid should be ~0.0, got {l}");
        assert!(i.abs() < 0.01, "iodine should be ~0.0, got {i}");
        assert!((w + l + i - 1.0).abs() < 1e-6, "fractions should sum to 1.0");
    }

    #[test]
    fn test_pure_lipid() {
        let config = test_config();
        let shape = (1, 1, 1);
        let v0 = Array3::from_elem(shape, 0.0171_f32);
        let v1 = Array3::from_elem(shape, 0.0159_f32);
        let v2 = Array3::from_elem(shape, 0.0148_f32);
        let v3 = Array3::from_elem(shape, 0.0146_f32);

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});

        let w = result.water[[0, 0, 0]];
        let l = result.lipid[[0, 0, 0]];
        let i = result.iodine[[0, 0, 0]];

        assert!(w.abs() < 0.01, "water should be ~0, got {w}");
        assert!((l - 1.0).abs() < 0.01, "lipid should be ~1.0, got {l}");
        assert!(i.abs() < 0.01, "iodine should be ~0, got {i}");
    }

    #[test]
    fn test_mixed_water_lipid() {
        let config = test_config();
        let shape = (1, 1, 1);
        // 50% water + 50% lipid:
        let v0 = Array3::from_elem(shape, (0.5 * 0.0193 + 0.5 * 0.0171) as f32);
        let v1 = Array3::from_elem(shape, (0.5 * 0.0171 + 0.5 * 0.0159) as f32);
        let v2 = Array3::from_elem(shape, (0.5 * 0.0155 + 0.5 * 0.0148) as f32);
        let v3 = Array3::from_elem(shape, (0.5 * 0.0152 + 0.5 * 0.0146) as f32);

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});

        let w = result.water[[0, 0, 0]];
        let l = result.lipid[[0, 0, 0]];
        let i = result.iodine[[0, 0, 0]];

        assert!((w - 0.5).abs() < 0.05, "water should be ~0.5, got {w}");
        assert!((l - 0.5).abs() < 0.05, "lipid should be ~0.5, got {l}");
        assert!(i.abs() < 0.05, "iodine should be ~0, got {i}");
    }

    #[test]
    fn test_pure_iodine() {
        let config = test_config();
        let shape = (1, 1, 1);
        let v0 = Array3::from_elem(shape, 0.0800_f32);
        let v1 = Array3::from_elem(shape, 0.0250_f32);
        let v2 = Array3::from_elem(shape, 0.0130_f32);
        let v3 = Array3::from_elem(shape, 0.0120_f32);

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});

        let w = result.water[[0, 0, 0]];
        let l = result.lipid[[0, 0, 0]];
        let i = result.iodine[[0, 0, 0]];

        assert!(w.abs() < 0.01, "water should be ~0, got {w}");
        assert!(l.abs() < 0.01, "lipid should be ~0, got {l}");
        assert!((i - 1.0).abs() < 0.01, "iodine should be ~1.0, got {i}");
    }

    #[test]
    fn test_simplex_projection() {
        // Already on simplex.
        let mut x = [0.3, 0.5, 0.2];
        project_simplex(&mut x);
        assert!((x[0] + x[1] + x[2] - 1.0).abs() < 1e-10);

        // Negative values → should be clipped.
        let mut x2 = [1.5, -0.3, -0.2];
        project_simplex(&mut x2);
        assert!(x2.iter().all(|&v| v >= 0.0));
        assert!((x2[0] + x2[1] + x2[2] - 1.0).abs() < 1e-10);

        // All equal.
        let mut x3 = [2.0, 2.0, 2.0];
        project_simplex(&mut x3);
        for v in &x3 {
            assert!((*v - 1.0 / 3.0).abs() < 1e-10);
        }
    }

    #[test]
    fn test_prefilter_skips_out_of_range() {
        let config = test_config();
        let shape = (1, 1, 2);
        // Pixel 0: in range, pixel 1: out of range (too high)
        let v0 = Array3::from_shape_vec(shape, vec![0.0193, 2.0]).unwrap();
        let v1 = Array3::from_shape_vec(shape, vec![0.0171, 2.0]).unwrap();
        let v2 = Array3::from_shape_vec(shape, vec![0.0155, 2.0]).unwrap();
        let v3 = Array3::from_shape_vec(shape, vec![0.0152, 2.0]).unwrap();

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});

        // Pixel 0 should be decomposed (water).
        assert!(result.water[[0, 0, 0]] > 0.9);
        // Pixel 1 should be skipped (all zeros).
        assert_eq!(result.water[[0, 0, 1]], 0.0);
        assert_eq!(result.lipid[[0, 0, 1]], 0.0);
        assert_eq!(result.iodine[[0, 0, 1]], 0.0);
    }
}
