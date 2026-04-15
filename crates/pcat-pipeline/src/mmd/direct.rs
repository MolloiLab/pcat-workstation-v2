use ndarray::Array3;
use rayon::prelude::*;

use super::materials::{Material, MaterialLibrary};

/// Result of decomposing a volume into basis material fractions and mass concentrations.
pub struct MmdResult {
    pub water_frac: Array3<f32>,
    pub lipid_frac: Array3<f32>,
    pub iodine_frac: Array3<f32>,
    pub calcium_frac: Array3<f32>, // always 0 for direct (3-material only)
    pub water_mass: Array3<f32>,   // mg/mL
    pub lipid_mass: Array3<f32>,
    pub iodine_mass: Array3<f32>,
    pub calcium_mass: Array3<f32>, // always 0
    pub total_density: Array3<f32>,
    pub mask: Array3<bool>,
    pub iterations: usize,  // 1 for direct
    pub converged: bool,    // true for direct
}

/// Determinant of a 3x3 matrix given as row-major [[r0], [r1], [r2]].
#[inline]
fn det3(m: [[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// Decompose a single voxel from dual-energy HU values into water/lipid/iodine
/// volume fractions using direct matrix inversion (Cramer's rule).
///
/// Returns `(f_water, f_lipid, f_iodine)` clamped to [0, 1].
fn decompose_voxel(hu_low: f64, hu_high: f64, lib: &MaterialLibrary) -> (f64, f64, f64) {
    // 1. Convert measured HU to LAC
    let mu_low = lib.hu_to_lac(hu_low, 0);
    let mu_high = lib.hu_to_lac(hu_high, 1);

    // 2. Build 3x3 system:
    //   [ mu_w(E1)  mu_l(E1)  mu_i(E1) ] [ f_w ]   [ mu_meas(E1) ]
    //   [ mu_w(E2)  mu_l(E2)  mu_i(E2) ] [ f_l ] = [ mu_meas(E2) ]
    //   [    1         1         1      ] [ f_i ]   [      1       ]
    let mu_w_lo = lib.lac_low(Material::Water);
    let mu_l_lo = lib.lac_low(Material::Lipid);
    let mu_i_lo = lib.lac_low(Material::Iodine);
    let mu_w_hi = lib.lac_high(Material::Water);
    let mu_l_hi = lib.lac_high(Material::Lipid);
    let mu_i_hi = lib.lac_high(Material::Iodine);

    let a = [
        [mu_w_lo, mu_l_lo, mu_i_lo],
        [mu_w_hi, mu_l_hi, mu_i_hi],
        [1.0, 1.0, 1.0],
    ];

    let b = [mu_low, mu_high, 1.0];

    // 3. Solve via Cramer's rule
    let det_a = det3(a);
    if det_a.abs() < 1e-15 {
        // Singular system — return all-water fallback
        return (1.0, 0.0, 0.0);
    }

    let det_w = det3([
        [b[0], a[0][1], a[0][2]],
        [b[1], a[1][1], a[1][2]],
        [b[2], a[2][1], a[2][2]],
    ]);

    let det_l = det3([
        [a[0][0], b[0], a[0][2]],
        [a[1][0], b[1], a[1][2]],
        [a[2][0], b[2], a[2][2]],
    ]);

    let det_i = det3([
        [a[0][0], a[0][1], b[0]],
        [a[1][0], a[1][1], b[1]],
        [a[2][0], a[2][1], b[2]],
    ]);

    let f_w = (det_w / det_a).clamp(0.0, 1.0);
    let f_l = (det_l / det_a).clamp(0.0, 1.0);
    let f_i = (det_i / det_a).clamp(0.0, 1.0);

    (f_w, f_l, f_i)
}

/// Decompose a dual-energy CT volume into water/lipid/iodine fractions and
/// mass concentrations using direct 3-material matrix inversion.
///
/// Only voxels where `mask[i] == true` are decomposed; the rest remain zero.
/// Uses rayon for parallel iteration over voxels.
pub fn decompose_volume_direct(
    low_energy: &Array3<f32>,
    high_energy: &Array3<f32>,
    mask: &Array3<bool>,
    materials: &MaterialLibrary,
) -> MmdResult {
    let shape = low_energy.shape();
    assert_eq!(shape, high_energy.shape(), "low/high energy shape mismatch");
    assert_eq!(shape, mask.shape(), "mask shape mismatch");

    let dim = (shape[0], shape[1], shape[2]);

    let rho_w = materials.density_mg_ml(Material::Water) as f32;
    let rho_l = materials.density_mg_ml(Material::Lipid) as f32;
    let rho_i = materials.density_mg_ml(Material::Iodine) as f32;

    // Flatten input arrays and decompose in parallel.
    let lo_slice = low_energy.as_slice().expect("low_energy not contiguous");
    let hi_slice = high_energy.as_slice().expect("high_energy not contiguous");
    let mask_slice = mask.as_slice().expect("mask not contiguous");

    // Per-voxel result: (fw, fl, fi, mw, ml, mi, td)
    let results: Vec<[f32; 7]> = lo_slice
        .par_iter()
        .zip(hi_slice.par_iter())
        .zip(mask_slice.par_iter())
        .map(|((&hu_lo, &hu_hi), &m)| {
            if !m {
                return [0.0; 7];
            }
            let (f_w, f_l, f_i) =
                decompose_voxel(hu_lo as f64, hu_hi as f64, materials);

            let fw = f_w as f32;
            let fl = f_l as f32;
            let fi = f_i as f32;
            let mw = fw * rho_w;
            let ml = fl * rho_l;
            let mi = fi * rho_i;
            let td = mw + ml + mi;
            [fw, fl, fi, mw, ml, mi, td]
        })
        .collect();

    // Scatter results into output arrays.
    let n = results.len();
    let mut wf_vec = vec![0.0f32; n];
    let mut lf_vec = vec![0.0f32; n];
    let mut if_vec = vec![0.0f32; n];
    let mut wm_vec = vec![0.0f32; n];
    let mut lm_vec = vec![0.0f32; n];
    let mut im_vec = vec![0.0f32; n];
    let mut td_vec = vec![0.0f32; n];

    for (i, r) in results.iter().enumerate() {
        wf_vec[i] = r[0];
        lf_vec[i] = r[1];
        if_vec[i] = r[2];
        wm_vec[i] = r[3];
        lm_vec[i] = r[4];
        im_vec[i] = r[5];
        td_vec[i] = r[6];
    }

    let water_frac = Array3::from_shape_vec(dim, wf_vec).unwrap();
    let lipid_frac = Array3::from_shape_vec(dim, lf_vec).unwrap();
    let iodine_frac = Array3::from_shape_vec(dim, if_vec).unwrap();
    let calcium_frac = Array3::<f32>::zeros(dim);
    let water_mass = Array3::from_shape_vec(dim, wm_vec).unwrap();
    let lipid_mass = Array3::from_shape_vec(dim, lm_vec).unwrap();
    let iodine_mass = Array3::from_shape_vec(dim, im_vec).unwrap();
    let calcium_mass = Array3::<f32>::zeros(dim);
    let total_density = Array3::from_shape_vec(dim, td_vec).unwrap();

    MmdResult {
        water_frac,
        lipid_frac,
        iodine_frac,
        calcium_frac,
        water_mass,
        lipid_mass,
        iodine_mass,
        calcium_mass,
        total_density,
        mask: mask.clone(),
        iterations: 1,
        converged: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::array;

    const EPS: f64 = 1e-4;

    #[test]
    fn pure_water_gives_fw_one() {
        // HU = 0 at both energies should give f_w ~= 1, f_l ~= 0, f_i ~= 0
        let lib = MaterialLibrary::naeotom_70_150();
        let (fw, fl, fi) = decompose_voxel(0.0, 0.0, &lib);

        assert!(
            (fw - 1.0).abs() < EPS,
            "water fraction: expected ~1.0, got {fw}"
        );
        assert!(fl.abs() < EPS, "lipid fraction: expected ~0.0, got {fl}");
        assert!(fi.abs() < EPS, "iodine fraction: expected ~0.0, got {fi}");
    }

    #[test]
    fn known_mixture_recovery() {
        // Construct a known mixture: 60% water, 30% lipid, 10% iodine.
        // Compute the expected HU values, then verify decomposition recovers them.
        let lib = MaterialLibrary::naeotom_70_150();

        let f_w = 0.6;
        let f_l = 0.3;
        let f_i = 0.1;

        // Synthesize LAC at both energies
        let mu_lo = f_w * lib.lac_low(Material::Water)
            + f_l * lib.lac_low(Material::Lipid)
            + f_i * lib.lac_low(Material::Iodine);
        let mu_hi = f_w * lib.lac_high(Material::Water)
            + f_l * lib.lac_high(Material::Lipid)
            + f_i * lib.lac_high(Material::Iodine);

        // Convert back to HU
        let hu_lo = (mu_lo / lib.lac_low(Material::Water) - 1.0) * 1000.0;
        let hu_hi = (mu_hi / lib.lac_high(Material::Water) - 1.0) * 1000.0;

        let (rw, rl, ri) = decompose_voxel(hu_lo, hu_hi, &lib);

        assert!(
            (rw - f_w).abs() < EPS,
            "water: expected {f_w}, got {rw}"
        );
        assert!(
            (rl - f_l).abs() < EPS,
            "lipid: expected {f_l}, got {rl}"
        );
        assert!(
            (ri - f_i).abs() < EPS,
            "iodine: expected {f_i}, got {ri}"
        );
    }

    #[test]
    fn fractions_clamped() {
        // Use extremely negative HU that would produce negative fractions
        // before clamping.
        let lib = MaterialLibrary::naeotom_70_150();
        let (fw, fl, fi) = decompose_voxel(-900.0, -900.0, &lib);

        assert!(fw >= 0.0, "water fraction should be >= 0, got {fw}");
        assert!(fl >= 0.0, "lipid fraction should be >= 0, got {fl}");
        assert!(fi >= 0.0, "iodine fraction should be >= 0, got {fi}");
        assert!(fw <= 1.0, "water fraction should be <= 1, got {fw}");
        assert!(fl <= 1.0, "lipid fraction should be <= 1, got {fl}");
        assert!(fi <= 1.0, "iodine fraction should be <= 1, got {fi}");
    }

    #[test]
    fn volume_mask_respected() {
        // 2x2x1 volume: only two voxels are masked
        let lib = MaterialLibrary::naeotom_70_150();

        let low = array![[[0.0_f32, 0.0], [100.0, -50.0]]];
        let high = array![[[0.0_f32, 0.0], [80.0, -40.0]]];
        let mask = array![[[true, false], [true, false]]];

        let result = decompose_volume_direct(&low, &high, &mask, &lib);

        // Masked-out voxels should be zero
        assert_eq!(result.water_frac[[0, 0, 1]], 0.0);
        assert_eq!(result.lipid_frac[[0, 0, 1]], 0.0);
        assert_eq!(result.iodine_frac[[0, 0, 1]], 0.0);
        assert_eq!(result.total_density[[0, 0, 1]], 0.0);

        assert_eq!(result.water_frac[[0, 1, 1]], 0.0);
        assert_eq!(result.lipid_frac[[0, 1, 1]], 0.0);

        // Masked-in voxels should have non-zero decomposition
        // HU=0 at both energies => pure water => water_frac ~= 1
        assert!(result.water_frac[[0, 0, 0]] > 0.5);

        // Calcium arrays always zero for direct method
        assert_eq!(result.calcium_frac[[0, 0, 0]], 0.0);
        assert_eq!(result.calcium_mass[[0, 0, 0]], 0.0);

        // Metadata
        assert_eq!(result.iterations, 1);
        assert!(result.converged);
    }

    #[test]
    fn volume_mass_consistency() {
        // Verify mass = fraction * density for a pure-water voxel
        let lib = MaterialLibrary::naeotom_70_150();

        let low = array![[[0.0_f32]]];
        let high = array![[[0.0_f32]]];
        let mask = array![[[true]]];

        let result = decompose_volume_direct(&low, &high, &mask, &lib);

        let fw = result.water_frac[[0, 0, 0]];
        let mw = result.water_mass[[0, 0, 0]];
        let expected_mw = fw * 1000.0; // water density = 1000 mg/mL

        assert!(
            (mw - expected_mw).abs() < 0.1,
            "water mass: expected {expected_mw}, got {mw}"
        );

        // total_density should equal sum of all masses
        let td = result.total_density[[0, 0, 0]];
        let sum = result.water_mass[[0, 0, 0]]
            + result.lipid_mass[[0, 0, 0]]
            + result.iodine_mass[[0, 0, 0]]
            + result.calcium_mass[[0, 0, 0]];
        assert!(
            (td - sum).abs() < 0.01,
            "total_density {td} != sum of masses {sum}"
        );
    }
}
