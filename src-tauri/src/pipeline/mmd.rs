//! Multi-Material Decomposition (MMD) for photon-counting CT mono-energetic data.
//!
//! Implements the Mendonça et al. (IEEE TMI 2014) framework:
//!   1. Convert HU images to LAC (linear attenuation coefficient) using NIST water reference
//!   2. Solve μ_L(E) = Σ αᵢ μ_L,i(E) in LAC space via WLS with simplex constraints
//!
//! Basis material LACs are from NIST XCOM tables (physical constants).
//! With 4 energies and 3 materials, the system is overdetermined (4 eq, 3 unknowns).

use nalgebra::{Matrix3, Matrix3x4, Matrix4x3, Vector3, Vector4};
use ndarray::Array3;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

const N_ENERGIES: usize = 4;
const N_MATERIALS: usize = 3;

// ---------------------------------------------------------------------------
// NIST XCOM physical constants (cm⁻¹)
// ---------------------------------------------------------------------------

/// Water LAC at [70, 100, 140, 150] keV (cm⁻¹).
/// Source: NIST XCOM, ρ = 1.000 g/cm³.
const WATER_LAC: [f64; N_ENERGIES] = [0.1937, 0.1707, 0.1538, 0.1505];

/// Adipose tissue (ICRU-44) LAC at [70, 100, 140, 150] keV (cm⁻¹).
/// Source: NIST XCOM, ρ = 0.950 g/cm³.
const ADIPOSE_LAC: [f64; N_ENERGIES] = [0.1785, 0.1604, 0.1454, 0.1425];

/// Iodine elemental mass attenuation coefficients at [70, 100, 140, 150] keV (cm²/g).
/// Source: NIST XCOM.
const IODINE_MU_RHO: [f64; N_ENERGIES] = [5.0174, 1.9420, 0.8306, 0.6978];

/// Compute LAC of iodine contrast solution at given concentration.
/// μ_L(E) = μ_water(E) + C_iodine * (μ/ρ)_iodine(E)
/// where C_iodine is in g/cm³ (e.g., 0.01 g/cm³ = 10 mg/mL).
fn iodine_solution_lac(concentration_g_per_cm3: f64) -> [f64; N_ENERGIES] {
    let mut lac = [0.0; N_ENERGIES];
    for i in 0..N_ENERGIES {
        lac[i] = WATER_LAC[i] + concentration_g_per_cm3 * IODINE_MU_RHO[i];
    }
    lac
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// MMD configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MmdConfig {
    /// Iodine concentration of the reference material (mg/mL).
    /// Used to compute the iodine basis LAC from NIST tables.
    /// Typical clinical value: 10-15 mg/mL in vessel lumen.
    /// Default: 10.0 mg/mL.
    #[serde(default = "default_iodine_concentration")]
    pub iodine_concentration_mg_ml: f64,

    /// Per-energy noise variance (HU²), estimated from a uniform ROI.
    /// Default: [100, 100, 100, 100] (~10 HU std).
    #[serde(default = "default_noise_variances")]
    pub noise_variances: [f64; N_ENERGIES],

    /// HU upper bound for pre-filtering (skip voxels above this).
    #[serde(default = "default_hu_upper")]
    pub hu_upper: f64,

    /// HU lower bound for pre-filtering (skip voxels below this).
    #[serde(default = "default_hu_lower")]
    pub hu_lower: f64,
}

fn default_iodine_concentration() -> f64 { 10.0 }
fn default_noise_variances() -> [f64; N_ENERGIES] { [100.0; N_ENERGIES] }
fn default_hu_upper() -> f64 { 500.0 }
fn default_hu_lower() -> f64 { -500.0 }

impl Default for MmdConfig {
    fn default() -> Self {
        Self {
            iodine_concentration_mg_ml: default_iodine_concentration(),
            noise_variances: default_noise_variances(),
            hu_upper: default_hu_upper(),
            hu_lower: default_hu_lower(),
        }
    }
}

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

/// Per-voxel volume fraction maps.
pub struct MmdResult {
    pub water: Array3<f32>,
    pub lipid: Array3<f32>,
    pub iodine: Array3<f32>,
    /// L2 fitting residual in LAC space (cm⁻¹).
    pub residual: Array3<f32>,
}

// ---------------------------------------------------------------------------
// WLS projector in LAC space
// ---------------------------------------------------------------------------

/// Precomputed WLS projector: P = (AᵀV⁻¹A)⁻¹AᵀV⁻¹ (3×4).
/// Maps a 4×1 LAC measurement vector → 3×1 volume fractions [water, lipid, iodine].
struct WlsProjector {
    p: Matrix3x4<f64>,
    a: Matrix4x3<f64>,
}

impl WlsProjector {
    fn new(config: &MmdConfig) -> Self {
        // Iodine basis LAC at the specified concentration.
        let iodine_lac = iodine_solution_lac(
            config.iodine_concentration_mg_ml / 1000.0, // mg/mL → g/cm³
        );

        // Build 4×3 composition matrix A₀ in LAC space (cm⁻¹).
        // Columns: water, adipose, iodine solution.
        let a = Matrix4x3::new(
            WATER_LAC[0], ADIPOSE_LAC[0], iodine_lac[0],
            WATER_LAC[1], ADIPOSE_LAC[1], iodine_lac[1],
            WATER_LAC[2], ADIPOSE_LAC[2], iodine_lac[2],
            WATER_LAC[3], ADIPOSE_LAC[3], iodine_lac[3],
        );

        // Convert HU noise variances to LAC noise variances.
        // LAC = (HU/1000 + 1) * μ_water(E)
        // var(LAC) = (μ_water(E)/1000)² * var(HU)
        let mut v_inv = [0.0_f64; N_ENERGIES];
        for i in 0..N_ENERGIES {
            let scale = WATER_LAC[i] / 1000.0;
            let lac_var = scale * scale * config.noise_variances[i];
            v_inv[i] = if lac_var > 1e-20 { 1.0 / lac_var } else { 1.0 };
        }

        // AᵀV⁻¹ (3×4)
        let at = a.transpose();
        let mut at_vinv = at;
        for col in 0..N_ENERGIES {
            for row in 0..N_MATERIALS {
                at_vinv[(row, col)] *= v_inv[col];
            }
        }

        // (AᵀV⁻¹A)⁻¹ (3×3)
        let ata: Matrix3<f64> = at_vinv * a;
        let ata_inv = ata
            .try_inverse()
            .expect("A₀ᵀV⁻¹A₀ is singular — basis materials are not spectrally separable");

        let p: Matrix3x4<f64> = ata_inv * at_vinv;
        Self { p, a }
    }

    #[inline]
    fn solve(&self, mu_lac: &Vector4<f64>) -> Vector3<f64> {
        self.p * mu_lac
    }

    #[inline]
    fn residual(&self, x: &Vector3<f64>, mu_lac: &Vector4<f64>) -> f64 {
        (self.a * x - mu_lac).norm()
    }
}

// ---------------------------------------------------------------------------
// HU → LAC conversion
// ---------------------------------------------------------------------------

/// Convert a single HU value to LAC (cm⁻¹) at a given energy index.
/// μ_L = (HU/1000 + 1) × μ_water(E)
#[inline]
fn hu_to_lac(hu: f64, energy_idx: usize) -> f64 {
    (hu / 1000.0 + 1.0) * WATER_LAC[energy_idx]
}

// ---------------------------------------------------------------------------
// Simplex projection (Duchi et al., 2008)
// ---------------------------------------------------------------------------

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
/// Steps per Mendonça 2014:
///   1. Convert HU → LAC using NIST water reference
///   2. Solve overdetermined 4×3 WLS in LAC space
///   3. Project onto probability simplex (volume conservation + non-negativity)
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

                let m0 = v0[idx];
                if m0 > hu_upper || m0 < hu_lower {
                    continue;
                }

                // Step 1: Convert HU → LAC (cm⁻¹).
                let mu_lac = Vector4::new(
                    hu_to_lac(m0 as f64, 0),
                    hu_to_lac(v1[idx] as f64, 1),
                    hu_to_lac(v2[idx] as f64, 2),
                    hu_to_lac(v3[idx] as f64, 3),
                );

                // Step 2: WLS solve in LAC space → [α_water, α_lipid, α_iodine].
                let x_raw = proj.solve(&mu_lac);
                let mut x = [x_raw[0], x_raw[1], x_raw[2]];

                // Step 3: Simplex projection (Σαᵢ = 1, αᵢ ≥ 0).
                project_simplex(&mut x);

                w_chunk[local] = x[0] as f32;
                l_chunk[local] = x[1] as f32;
                i_chunk[local] = x[2] as f32;

                let x_vec = Vector3::new(x[0], x[1], x[2]);
                r_chunk[local] = proj.residual(&x_vec, &mu_lac) as f32;
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

    fn default_cfg() -> MmdConfig {
        MmdConfig {
            iodine_concentration_mg_ml: 10.0,
            noise_variances: [100.0; 4],
            hu_upper: 1000.0,
            hu_lower: -1000.0,
        }
    }

    #[test]
    fn test_pure_water() {
        let config = default_cfg();
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

        assert!((w - 1.0).abs() < 0.05, "water={w}, expected ~1.0");
        assert!(l.abs() < 0.05, "lipid={l}, expected ~0");
        assert!(i.abs() < 0.05, "iodine={i}, expected ~0");
    }

    #[test]
    fn test_pure_fat() {
        let config = default_cfg();
        let shape = (1, 1, 1);
        // Fat ≈ -80 HU (adipose tissue). Compute expected HU from NIST LACs.
        // HU(E) = 1000 * (μ_adipose(E) / μ_water(E) - 1)
        let hu: Vec<f32> = (0..4)
            .map(|i| (1000.0 * (ADIPOSE_LAC[i] / WATER_LAC[i] - 1.0)) as f32)
            .collect();

        let v0 = Array3::from_elem(shape, hu[0]); // ~ -78 HU
        let v1 = Array3::from_elem(shape, hu[1]); // ~ -60 HU
        let v2 = Array3::from_elem(shape, hu[2]); // ~ -55 HU
        let v3 = Array3::from_elem(shape, hu[3]); // ~ -53 HU

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});
        let w = result.water[[0, 0, 0]];
        let l = result.lipid[[0, 0, 0]];
        let i = result.iodine[[0, 0, 0]];

        assert!(w.abs() < 0.05, "water={w}, expected ~0");
        assert!((l - 1.0).abs() < 0.05, "lipid={l}, expected ~1.0");
        assert!(i.abs() < 0.05, "iodine={i}, expected ~0");
    }

    #[test]
    fn test_pure_iodine_solution() {
        let config = default_cfg();
        let shape = (1, 1, 1);
        // Iodine solution at reference concentration.
        let iodine_lac = iodine_solution_lac(0.01); // 10 mg/mL
        let hu: Vec<f32> = (0..4)
            .map(|i| (1000.0 * (iodine_lac[i] / WATER_LAC[i] - 1.0)) as f32)
            .collect();

        let v0 = Array3::from_elem(shape, hu[0]); // ~ +259 HU
        let v1 = Array3::from_elem(shape, hu[1]); // ~ +114 HU
        let v2 = Array3::from_elem(shape, hu[2]); // ~ +54 HU
        let v3 = Array3::from_elem(shape, hu[3]); // ~ +47 HU

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});
        let w = result.water[[0, 0, 0]];
        let l = result.lipid[[0, 0, 0]];
        let i = result.iodine[[0, 0, 0]];

        assert!(w.abs() < 0.05, "water={w}, expected ~0");
        assert!(l.abs() < 0.05, "lipid={l}, expected ~0");
        assert!((i - 1.0).abs() < 0.05, "iodine={i}, expected ~1.0");
    }

    #[test]
    fn test_mixed_50_50_water_lipid() {
        let config = default_cfg();
        let shape = (1, 1, 1);
        // 50% water + 50% adipose.
        let hu: Vec<f32> = (0..4)
            .map(|i| {
                let lac = 0.5 * WATER_LAC[i] + 0.5 * ADIPOSE_LAC[i];
                (1000.0 * (lac / WATER_LAC[i] - 1.0)) as f32
            })
            .collect();

        let v0 = Array3::from_elem(shape, hu[0]);
        let v1 = Array3::from_elem(shape, hu[1]);
        let v2 = Array3::from_elem(shape, hu[2]);
        let v3 = Array3::from_elem(shape, hu[3]);

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});
        let w = result.water[[0, 0, 0]];
        let l = result.lipid[[0, 0, 0]];
        let i = result.iodine[[0, 0, 0]];

        assert!((w - 0.5).abs() < 0.1, "water={w}, expected ~0.5");
        assert!((l - 0.5).abs() < 0.1, "lipid={l}, expected ~0.5");
        assert!(i.abs() < 0.05, "iodine={i}, expected ~0");
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
    fn test_hu_to_lac_roundtrip() {
        // Water at 0 HU should give water LAC.
        for i in 0..4 {
            let lac = hu_to_lac(0.0, i);
            assert!((lac - WATER_LAC[i]).abs() < 1e-10);
        }
        // Air at -1000 HU should give ~0 LAC.
        for i in 0..4 {
            let lac = hu_to_lac(-1000.0, i);
            assert!(lac.abs() < 1e-10);
        }
    }

    #[test]
    fn test_prefilter() {
        let config = default_cfg();
        let shape = (1, 1, 2);
        let v0 = Array3::from_shape_vec(shape, vec![0.0, 2000.0]).unwrap();
        let v1 = Array3::from_shape_vec(shape, vec![0.0, 2000.0]).unwrap();
        let v2 = Array3::from_shape_vec(shape, vec![0.0, 2000.0]).unwrap();
        let v3 = Array3::from_shape_vec(shape, vec![0.0, 2000.0]).unwrap();

        let result = decompose([&v0, &v1, &v2, &v3], &config, |_| {});
        assert!(result.water[[0, 0, 0]] > 0.9);
        assert_eq!(result.water[[0, 0, 1]], 0.0);
    }
}
