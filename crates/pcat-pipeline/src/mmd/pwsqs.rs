use ndarray::Array3;
use rayon::prelude::*;

use super::direct::{decompose_volume_direct, MmdResult};
use super::materials::{Material, MaterialLibrary};

/// Parameters for the PWSQS (Pixel-Wise Separable Quadratic Surrogate) solver.
pub struct PwsqsParams {
    /// Regularization strength (default: 0.01).
    pub beta: f64,
    /// Huber penalty transition width (default: 0.1).
    pub delta: f64,
    /// Maximum number of iterations (default: 50).
    pub max_iter: usize,
    /// Convergence tolerance on L-infinity norm of fraction change (default: 1e-5).
    pub tol: f64,
}

impl Default for PwsqsParams {
    fn default() -> Self {
        Self {
            beta: 0.01,
            delta: 0.1,
            max_iter: 50,
            tol: 1e-5,
        }
    }
}

/// Determinant of a 3x3 matrix given as row-major [[r0], [r1], [r2]].
#[inline]
fn det3(m: [[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// Invert a 3x3 matrix using the adjugate formula.
/// Returns `None` if the matrix is singular (|det| < eps).
#[inline]
fn invert3(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let d = det3(m);
    if d.abs() < 1e-15 {
        return None;
    }
    let inv_d = 1.0 / d;

    // Cofactor matrix (transposed = adjugate)
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_d,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_d,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_d,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_d,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_d,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_d,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_d,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_d,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_d,
        ],
    ])
}

/// Huber-like weight function: w(t) = psi'(t)/t = 1 / sqrt(1 + 3*t^2/delta^2).
/// This avoids division by zero when t=0.
#[inline]
fn huber_weight(t: f64, delta: f64) -> f64 {
    1.0 / (1.0 + 3.0 * t * t / (delta * delta)).sqrt()
}

/// Map a `Material` to its index in the 4-element fraction array
/// `[water, lipid, iodine, calcium]`.
#[inline]
fn material_index(mat: Material) -> usize {
    match mat {
        Material::Water => 0,
        Material::Lipid => 1,
        Material::Iodine => 2,
        Material::Calcium => 3,
    }
}

/// 6-connected neighbor offsets in 3D: +/-z, +/-y, +/-x.
const NEIGHBOR_OFFSETS: [(isize, isize, isize); 6] = [
    (-1, 0, 0),
    (1, 0, 0),
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
];

/// Solve the PWSQS (Pixel-Wise Separable Quadratic Surrogate) iterative
/// multi-material decomposition from dual-energy CT volumes.
///
/// This implements the Xue 2017 method: for each voxel, the algorithm
/// enumerates all C(4,3)=4 material triplets, solves a regularized 3x3
/// quadratic subproblem, and selects the triplet with minimum objective.
/// Spatial regularization via a Huber-like penalty encourages smooth
/// fraction maps while preserving edges.
///
/// Only voxels where `mask[i] == true` are decomposed; the rest remain zero.
///
/// # Arguments
///
/// * `low_energy` - HU volume at low energy (e.g. 70 keV VMI+)
/// * `high_energy` - HU volume at high energy (e.g. 150 keV VMI+)
/// * `mask` - Boolean mask; only `true` voxels are decomposed
/// * `materials` - Material library with LAC values
/// * `params` - PWSQS solver parameters
/// * `progress_cb` - Optional callback `(iteration, max_change)` for progress reporting
pub fn pwsqs_solve(
    low_energy: &Array3<f32>,
    high_energy: &Array3<f32>,
    mask: &Array3<bool>,
    materials: &MaterialLibrary,
    params: &PwsqsParams,
    progress_cb: Option<&dyn Fn(usize, f64)>,
) -> MmdResult {
    let shape = low_energy.shape();
    assert_eq!(shape, high_energy.shape(), "low/high energy shape mismatch");
    assert_eq!(shape, mask.shape(), "mask shape mismatch");

    let (nz, ny, nx) = (shape[0], shape[1], shape[2]);
    let dim = (nz, ny, nx);
    let nvoxels = nz * ny * nx;

    // --- Initialize from direct decomposition ---
    let direct = decompose_volume_direct(low_energy, high_energy, mask, materials);

    // Current fraction state: 4 flat arrays [water, lipid, iodine, calcium]
    let mut fracs: Vec<Vec<f64>> = vec![
        direct.water_frac.as_slice().unwrap().iter().map(|&v| v as f64).collect(),
        direct.lipid_frac.as_slice().unwrap().iter().map(|&v| v as f64).collect(),
        direct.iodine_frac.as_slice().unwrap().iter().map(|&v| v as f64).collect(),
        vec![0.0; nvoxels], // calcium starts at 0
    ];

    let lo_slice = low_energy.as_slice().expect("low_energy not contiguous");
    let hi_slice = high_energy.as_slice().expect("high_energy not contiguous");
    let mask_slice = mask.as_slice().expect("mask not contiguous");

    // Pre-compute material triplets and their system matrices
    let triplets = materials.triplets();
    let triplet_data: Vec<TripletInfo> = triplets
        .iter()
        .map(|tri| {
            let sys = materials.system_matrix(tri);
            let indices = [
                material_index(tri[0]),
                material_index(tri[1]),
                material_index(tri[2]),
            ];
            TripletInfo { sys, indices }
        })
        .collect();

    // Collect masked voxel linear indices for parallel iteration
    let masked_indices: Vec<usize> = mask_slice
        .iter()
        .enumerate()
        .filter_map(|(i, &m)| if m { Some(i) } else { None })
        .collect();

    let mut converged = false;
    let mut iterations = 0;

    for iter in 0..params.max_iter {
        // Each iteration produces new fractions; we compute them in parallel
        // then swap. We need read access to `fracs` during the parallel phase.
        let new_fracs: Vec<[f64; 4]> = masked_indices
            .par_iter()
            .map(|&idx| {
                let iz = idx / (ny * nx);
                let iy = (idx % (ny * nx)) / nx;
                let ix = idx % nx;

                // Measured LAC
                let mu_low = materials.hu_to_lac(lo_slice[idx] as f64, 0);
                let mu_high = materials.hu_to_lac(hi_slice[idx] as f64, 1);

                // Current fractions at this voxel
                let cur = [fracs[0][idx], fracs[1][idx], fracs[2][idx], fracs[3][idx]];

                // Collect neighbor fractions (only valid, in-mask neighbors)
                let mut neighbors: Vec<[f64; 4]> = Vec::with_capacity(6);
                for &(dz, dy, dx) in &NEIGHBOR_OFFSETS {
                    let nz_i = iz as isize + dz;
                    let ny_i = iy as isize + dy;
                    let nx_i = ix as isize + dx;
                    if nz_i >= 0
                        && nz_i < nz as isize
                        && ny_i >= 0
                        && ny_i < ny as isize
                        && nx_i >= 0
                        && nx_i < nx as isize
                    {
                        let nidx =
                            nz_i as usize * ny * nx + ny_i as usize * nx + nx_i as usize;
                        if mask_slice[nidx] {
                            neighbors.push([
                                fracs[0][nidx],
                                fracs[1][nidx],
                                fracs[2][nidx],
                                fracs[3][nidx],
                            ]);
                        }
                    }
                }

                // Evaluate each triplet and pick the best
                let mut best_obj = f64::INFINITY;
                let mut best_fracs = [0.0f64; 4];

                for td in &triplet_data {
                    let result = solve_triplet(
                        mu_low, mu_high, &cur, &neighbors, td, params,
                    );
                    if let Some((f_tri, obj)) = result {
                        if obj < best_obj {
                            best_obj = obj;
                            // Map triplet fractions back to 4-material vector
                            let mut f4 = [0.0f64; 4];
                            f4[td.indices[0]] = f_tri[0];
                            f4[td.indices[1]] = f_tri[1];
                            f4[td.indices[2]] = f_tri[2];
                            best_fracs = f4;
                        }
                    }
                }

                best_fracs
            })
            .collect();

        // Compute convergence metric and update fractions
        let mut max_change = 0.0f64;
        for (vi, &idx) in masked_indices.iter().enumerate() {
            for m in 0..4 {
                let delta = (new_fracs[vi][m] - fracs[m][idx]).abs();
                if delta > max_change {
                    max_change = delta;
                }
                fracs[m][idx] = new_fracs[vi][m];
            }
        }

        iterations = iter + 1;

        if let Some(cb) = &progress_cb {
            cb(iterations, max_change);
        }

        if max_change < params.tol {
            converged = true;
            break;
        }
    }

    // --- Build output MmdResult ---
    let rho_w = materials.density_mg_ml(Material::Water) as f32;
    let rho_l = materials.density_mg_ml(Material::Lipid) as f32;
    let rho_i = materials.density_mg_ml(Material::Iodine) as f32;
    let rho_c = materials.density_mg_ml(Material::Calcium) as f32;

    let wf: Vec<f32> = fracs[0].iter().map(|&v| v as f32).collect();
    let lf: Vec<f32> = fracs[1].iter().map(|&v| v as f32).collect();
    let ifr: Vec<f32> = fracs[2].iter().map(|&v| v as f32).collect();
    let cf: Vec<f32> = fracs[3].iter().map(|&v| v as f32).collect();

    let wm: Vec<f32> = wf.iter().map(|&f| f * rho_w).collect();
    let lm: Vec<f32> = lf.iter().map(|&f| f * rho_l).collect();
    let im: Vec<f32> = ifr.iter().map(|&f| f * rho_i).collect();
    let cm: Vec<f32> = cf.iter().map(|&f| f * rho_c).collect();

    let td: Vec<f32> = (0..nvoxels)
        .map(|i| wm[i] + lm[i] + im[i] + cm[i])
        .collect();

    MmdResult {
        water_frac: Array3::from_shape_vec(dim, wf).unwrap(),
        lipid_frac: Array3::from_shape_vec(dim, lf).unwrap(),
        iodine_frac: Array3::from_shape_vec(dim, ifr).unwrap(),
        calcium_frac: Array3::from_shape_vec(dim, cf).unwrap(),
        water_mass: Array3::from_shape_vec(dim, wm).unwrap(),
        lipid_mass: Array3::from_shape_vec(dim, lm).unwrap(),
        iodine_mass: Array3::from_shape_vec(dim, im).unwrap(),
        calcium_mass: Array3::from_shape_vec(dim, cm).unwrap(),
        total_density: Array3::from_shape_vec(dim, td).unwrap(),
        mask: mask.clone(),
        iterations,
        converged,
    }
}

/// Pre-computed info for a material triplet.
struct TripletInfo {
    /// 2x3 system matrix: sys[energy][material_in_triplet]
    sys: [[f64; 3]; 2],
    /// Indices of the 3 materials in the 4-element fraction array
    indices: [usize; 3],
}

/// Solve the regularized quadratic subproblem for a single triplet at a single
/// voxel. Returns `Some((fractions, objective))` or `None` if the system is
/// singular.
///
/// Builds the augmented 3x3 system (2 LAC equations + volume conservation),
/// adds Huber-weighted spatial regularization from neighbors, solves via
/// matrix inversion, clamps to [0,1], and normalizes.
fn solve_triplet(
    mu_low: f64,
    mu_high: f64,
    cur_fracs: &[f64; 4],
    neighbors: &[[f64; 4]],
    td: &TripletInfo,
    params: &PwsqsParams,
) -> Option<([f64; 3], f64)> {
    // Augmented 3x3 matrix: rows are [low LAC, high LAC, volume conservation]
    let a_aug = [
        td.sys[0],                // low energy LAC row
        td.sys[1],                // high energy LAC row
        [1.0, 1.0, 1.0],         // volume conservation
    ];
    let b_aug = [mu_low, mu_high, 1.0];

    // Data fidelity Hessian: H_data = A^T A (3x3)
    let mut h_data = [[0.0f64; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            for k in 0..3 {
                h_data[i][j] += a_aug[k][i] * a_aug[k][j];
            }
        }
    }

    // Data fidelity gradient: q_data = -A^T b
    let mut q_data = [0.0f64; 3];
    for i in 0..3 {
        for k in 0..3 {
            q_data[i] -= a_aug[k][i] * b_aug[k];
        }
    }

    // Regularization: Huber-weighted penalty from neighbors
    let mut h_reg = [0.0f64; 3]; // diagonal only
    let mut q_reg = [0.0f64; 3];

    for l in 0..3 {
        let mat_idx = td.indices[l];
        let f_lp = cur_fracs[mat_idx];

        for neighbor in neighbors {
            let f_lq = neighbor[mat_idx];
            let diff = f_lp - f_lq;
            let w = huber_weight(diff, params.delta);
            h_reg[l] += params.beta * w;
            q_reg[l] += params.beta * w * (-f_lq);
        }
    }

    // H_total = H_data + diag(h_reg)
    let mut h_total = h_data;
    for l in 0..3 {
        h_total[l][l] += h_reg[l];
    }

    // q_total = q_data + q_reg
    let q_total = [
        q_data[0] + q_reg[0],
        q_data[1] + q_reg[1],
        q_data[2] + q_reg[2],
    ];

    // Solve: f = -H^{-1} q
    let inv = invert3(h_total)?;
    let mut f_tri = [0.0f64; 3];
    for i in 0..3 {
        f_tri[i] = 0.0;
        for j in 0..3 {
            f_tri[i] -= inv[i][j] * q_total[j];
        }
    }

    // Clamp to [0, 1]
    for v in f_tri.iter_mut() {
        *v = v.clamp(0.0, 1.0);
    }

    // Normalize so sum = 1 (if sum > 0)
    let sum: f64 = f_tri.iter().sum();
    if sum > 1e-12 {
        for v in f_tri.iter_mut() {
            *v /= sum;
        }
    }

    // Compute objective: f^T H_data f + q_data^T f + regularization cost
    let mut obj = 0.0f64;
    // Quadratic data term: f^T H_data f
    for i in 0..3 {
        for j in 0..3 {
            obj += f_tri[i] * h_data[i][j] * f_tri[j];
        }
    }
    // Linear data term: 2 * q_data^T f  (since objective = f^T H f + 2 q^T f + const)
    // Actually our formulation: min f^T H f / 2 + q^T f => gradient = H f + q = 0
    // The quadratic form value is f^T H_data f + 2 q_data^T f
    for i in 0..3 {
        obj += 2.0 * q_data[i] * f_tri[i];
    }

    // Regularization cost: sum of Huber penalties
    for l in 0..3 {
        let mat_idx = td.indices[l];
        for neighbor in neighbors {
            let diff = f_tri[l] - neighbor[mat_idx];
            // psi(t) = (delta^2 / 3) * (sqrt(1 + 3*t^2/delta^2) - 1)
            let d2 = params.delta * params.delta;
            obj += params.beta * (d2 / 3.0) * ((1.0 + 3.0 * diff * diff / d2).sqrt() - 1.0);
        }
    }

    Some((f_tri, obj))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array3;

    fn make_uniform_volume(
        nz: usize,
        ny: usize,
        nx: usize,
        hu_low: f32,
        hu_high: f32,
    ) -> (Array3<f32>, Array3<f32>, Array3<bool>) {
        let low = Array3::from_elem((nz, ny, nx), hu_low);
        let high = Array3::from_elem((nz, ny, nx), hu_high);
        let mask = Array3::from_elem((nz, ny, nx), true);
        (low, high, mask)
    }

    #[test]
    fn pure_water_converges() {
        // All HU=0 => should converge to f_water ~ 1, others ~ 0
        let lib = MaterialLibrary::naeotom_70_150();
        let (low, high, mask) = make_uniform_volume(3, 3, 3, 0.0, 0.0);
        let params = PwsqsParams {
            max_iter: 20,
            ..Default::default()
        };

        let result = pwsqs_solve(&low, &high, &mask, &lib, &params, None);

        // Check center voxel
        let fw = result.water_frac[[1, 1, 1]];
        let fl = result.lipid_frac[[1, 1, 1]];
        let fi = result.iodine_frac[[1, 1, 1]];
        let fc = result.calcium_frac[[1, 1, 1]];

        assert!(
            (fw - 1.0).abs() < 0.02,
            "water fraction: expected ~1.0, got {fw}"
        );
        assert!(fl.abs() < 0.02, "lipid fraction: expected ~0.0, got {fl}");
        assert!(
            fi.abs() < 0.02,
            "iodine fraction: expected ~0.0, got {fi}"
        );
        assert!(
            fc.abs() < 0.02,
            "calcium fraction: expected ~0.0, got {fc}"
        );
        assert!(result.converged, "should converge within {} iterations", params.max_iter);
        assert!(
            result.iterations <= 5,
            "expected convergence in <= 5 iterations, got {}",
            result.iterations
        );
    }

    #[test]
    fn known_mixture_recovery() {
        // Synthesize HU from known 60%W/30%L/10%I mixture, verify recovery
        let lib = MaterialLibrary::naeotom_70_150();

        let f_w = 0.6;
        let f_l = 0.3;
        let f_i = 0.1;

        let mu_lo = f_w * lib.lac_low(Material::Water)
            + f_l * lib.lac_low(Material::Lipid)
            + f_i * lib.lac_low(Material::Iodine);
        let mu_hi = f_w * lib.lac_high(Material::Water)
            + f_l * lib.lac_high(Material::Lipid)
            + f_i * lib.lac_high(Material::Iodine);

        let hu_lo = ((mu_lo / lib.lac_low(Material::Water)) - 1.0) * 1000.0;
        let hu_hi = ((mu_hi / lib.lac_high(Material::Water)) - 1.0) * 1000.0;

        let (low, high, mask) =
            make_uniform_volume(3, 3, 3, hu_lo as f32, hu_hi as f32);
        let params = PwsqsParams::default();

        let result = pwsqs_solve(&low, &high, &mask, &lib, &params, None);

        let fw = result.water_frac[[1, 1, 1]] as f64;
        let fl = result.lipid_frac[[1, 1, 1]] as f64;
        let fi = result.iodine_frac[[1, 1, 1]] as f64;
        let fc = result.calcium_frac[[1, 1, 1]] as f64;

        assert!(
            (fw - f_w).abs() < 0.05,
            "water: expected {f_w}, got {fw}"
        );
        assert!(
            (fl - f_l).abs() < 0.05,
            "lipid: expected {f_l}, got {fl}"
        );
        assert!(
            (fi - f_i).abs() < 0.05,
            "iodine: expected {f_i}, got {fi}"
        );
        assert!(
            fc.abs() < 0.05,
            "calcium should be ~0 for W/L/I mixture, got {fc}"
        );
    }

    #[test]
    fn pwsqs_smoother_than_direct() {
        // Add noise to a uniform water volume and compare the total variation
        // (sum of absolute neighbor differences) of water fraction between
        // direct and PWSQS decomposition. PWSQS spatial regularization should
        // produce lower TV even when triplet selection adds some variability.
        let lib = MaterialLibrary::naeotom_70_150();
        let (nz, ny, nx) = (7, 7, 7);

        // Seeded pseudo-noise: simple LCG for reproducibility
        let mut rng_state: u64 = 42;
        let mut noise = || -> f32 {
            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let t = ((rng_state >> 33) as f64) / (u32::MAX as f64) * 2.0 - 1.0;
            (t * 20.0) as f32
        };

        let n = nz * ny * nx;
        let mut low_data = vec![0.0f32; n];
        let mut high_data = vec![0.0f32; n];
        for i in 0..n {
            low_data[i] = noise();
            high_data[i] = noise();
        }

        let low = Array3::from_shape_vec((nz, ny, nx), low_data).unwrap();
        let high = Array3::from_shape_vec((nz, ny, nx), high_data).unwrap();
        let mask = Array3::from_elem((nz, ny, nx), true);

        let direct = decompose_volume_direct(&low, &high, &mask, &lib);
        let params = PwsqsParams {
            beta: 5.0, // strong regularization to clearly show smoothing
            max_iter: 50,
            ..Default::default()
        };
        let pwsqs = pwsqs_solve(&low, &high, &mask, &lib, &params, None);

        // Compute total variation (sum of |f[i]-f[j]| for all neighbor pairs)
        let tv_direct = total_variation_3d(&direct.water_frac);
        let tv_pwsqs = total_variation_3d(&pwsqs.water_frac);

        assert!(
            tv_pwsqs < tv_direct,
            "PWSQS TV ({tv_pwsqs:.4}) should be less than direct TV ({tv_direct:.4})"
        );
    }

    #[test]
    fn convergence_metadata() {
        let lib = MaterialLibrary::naeotom_70_150();
        let (low, high, mask) = make_uniform_volume(3, 3, 3, 0.0, 0.0);
        let params = PwsqsParams::default();

        let result = pwsqs_solve(&low, &high, &mask, &lib, &params, None);

        assert!(result.converged, "clean signal should converge");
        assert!(
            result.iterations < params.max_iter,
            "should converge before max_iter ({} < {})",
            result.iterations,
            params.max_iter
        );
    }

    #[test]
    fn calcium_detection() {
        // Synthesize a voxel with calcium + water mixture and verify calcium_frac > 0.
        let lib = MaterialLibrary::naeotom_70_150();

        // 80% water + 20% calcium
        let f_w = 0.8;
        let f_ca = 0.2;

        let mu_lo = f_w * lib.lac_low(Material::Water) + f_ca * lib.lac_low(Material::Calcium);
        let mu_hi =
            f_w * lib.lac_high(Material::Water) + f_ca * lib.lac_high(Material::Calcium);

        let hu_lo = ((mu_lo / lib.lac_low(Material::Water)) - 1.0) * 1000.0;
        let hu_hi = ((mu_hi / lib.lac_high(Material::Water)) - 1.0) * 1000.0;

        let (low, high, mask) =
            make_uniform_volume(3, 3, 3, hu_lo as f32, hu_hi as f32);
        let params = PwsqsParams::default();

        let result = pwsqs_solve(&low, &high, &mask, &lib, &params, None);

        let fc = result.calcium_frac[[1, 1, 1]] as f64;
        assert!(
            fc > 0.05,
            "calcium fraction should be > 0 for calcium/water mixture, got {fc}"
        );

        // Direct method always gives calcium = 0
        let direct = decompose_volume_direct(&low, &high, &mask, &lib);
        assert_eq!(
            direct.calcium_frac[[1, 1, 1]], 0.0,
            "direct method should always have calcium = 0"
        );
    }

    #[test]
    fn progress_callback_fires() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let lib = MaterialLibrary::naeotom_70_150();
        let (low, high, mask) = make_uniform_volume(3, 3, 3, 0.0, 0.0);
        let params = PwsqsParams {
            max_iter: 10,
            ..Default::default()
        };

        let call_count = AtomicUsize::new(0);
        let cb = |_iter: usize, _delta: f64| {
            call_count.fetch_add(1, Ordering::Relaxed);
        };

        let result = pwsqs_solve(&low, &high, &mask, &lib, &params, Some(&cb));

        let count = call_count.load(Ordering::Relaxed);
        assert_eq!(
            count, result.iterations,
            "callback should fire once per iteration"
        );
    }

    #[test]
    fn mask_respected() {
        let lib = MaterialLibrary::naeotom_70_150();

        let low = Array3::from_elem((2, 2, 2), 50.0f32);
        let high = Array3::from_elem((2, 2, 2), 40.0f32);
        let mut mask = Array3::from_elem((2, 2, 2), true);
        mask[[0, 0, 0]] = false;
        mask[[1, 1, 1]] = false;

        let params = PwsqsParams::default();
        let result = pwsqs_solve(&low, &high, &mask, &lib, &params, None);

        // Masked-out voxels should be zero
        assert_eq!(result.water_frac[[0, 0, 0]], 0.0);
        assert_eq!(result.lipid_frac[[0, 0, 0]], 0.0);
        assert_eq!(result.iodine_frac[[0, 0, 0]], 0.0);
        assert_eq!(result.calcium_frac[[0, 0, 0]], 0.0);

        assert_eq!(result.water_frac[[1, 1, 1]], 0.0);

        // Masked-in voxels should have non-zero decomposition
        assert!(result.water_frac[[0, 0, 1]] > 0.0 || result.lipid_frac[[0, 0, 1]] > 0.0);
    }

    /// Helper: compute 3D total variation (sum of |f[p]-f[q]| over all
    /// 6-connected neighbor pairs, counting each pair once).
    fn total_variation_3d(vol: &Array3<f32>) -> f64 {
        let (nz, ny, nx) = (vol.shape()[0], vol.shape()[1], vol.shape()[2]);
        let mut tv = 0.0f64;
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    let v = vol[[z, y, x]] as f64;
                    // Only forward neighbors to avoid double counting
                    if z + 1 < nz {
                        tv += (v - vol[[z + 1, y, x]] as f64).abs();
                    }
                    if y + 1 < ny {
                        tv += (v - vol[[z, y + 1, x]] as f64).abs();
                    }
                    if x + 1 < nx {
                        tv += (v - vol[[z, y, x + 1]] as f64).abs();
                    }
                }
            }
        }
        tv
    }
}
