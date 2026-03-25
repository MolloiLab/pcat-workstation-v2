use ndarray::Array3;
use serde::Serialize;

/// FAI (Fat Attenuation Index) statistics for a single vessel.
#[derive(Serialize, Clone, Debug)]
pub struct FaiStats {
    /// Vessel name (e.g. "LAD", "LCx", "RCA").
    pub vessel: String,
    /// Total number of voxels in the perivascular VOI.
    pub n_voi_voxels: usize,
    /// Number of voxels with HU in the fat range within the VOI.
    pub n_fat_voxels: usize,
    /// Fraction of VOI voxels that are fat (n_fat / n_voi).
    pub fat_fraction: f64,
    /// Mean HU of fat voxels.
    pub hu_mean: f64,
    /// Standard deviation of HU of fat voxels.
    pub hu_std: f64,
    /// Median HU of fat voxels.
    pub hu_median: f64,
    /// Risk classification based on mean FAI.
    pub fai_risk: String,
    /// Histogram bin centers (100 bins from -200 to 200).
    pub histogram_bins: Vec<f64>,
    /// Histogram counts for each bin.
    pub histogram_counts: Vec<usize>,
}

/// Compute PCAT (pericoronary adipose tissue) statistics.
///
/// Collects HU values from the VOI mask, filters to the fat HU range,
/// and computes summary statistics and a histogram.
pub fn compute_pcat_stats(
    volume: &Array3<f32>,
    voi_mask: &Array3<bool>,
    vessel: &str,
    hu_range: (f64, f64), // (-190.0, -30.0)
) -> FaiStats {
    let shape = volume.shape();
    let (nz, ny, nx) = (shape[0], shape[1], shape[2]);

    // 1. Collect all HU values where voi_mask is true
    let mut voi_values = Vec::new();
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                if voi_mask[[z, y, x]] {
                    voi_values.push(volume[[z, y, x]] as f64);
                }
            }
        }
    }

    let n_voi_voxels = voi_values.len();

    // 2. Filter to fat HU range
    let mut fat_values: Vec<f64> = voi_values
        .iter()
        .filter(|&&hu| hu >= hu_range.0 && hu <= hu_range.1)
        .copied()
        .collect();

    let n_fat_voxels = fat_values.len();

    // 3. Compute statistics
    let (hu_mean, hu_std, hu_median) = if fat_values.is_empty() {
        (0.0, 0.0, 0.0)
    } else {
        let mean = fat_values.iter().sum::<f64>() / fat_values.len() as f64;

        let variance = fat_values
            .iter()
            .map(|&v| (v - mean).powi(2))
            .sum::<f64>()
            / fat_values.len() as f64;
        let std = variance.sqrt();

        // Median
        fat_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = if fat_values.len() % 2 == 0 {
            let mid = fat_values.len() / 2;
            (fat_values[mid - 1] + fat_values[mid]) / 2.0
        } else {
            fat_values[fat_values.len() / 2]
        };

        (mean, std, median)
    };

    // 4. Fat fraction
    let fat_fraction = if n_voi_voxels > 0 {
        n_fat_voxels as f64 / n_voi_voxels as f64
    } else {
        0.0
    };

    // 5. Risk classification
    // Based on Oikonomou et al. (2018): FAI > -70.1 HU indicates high risk
    let fai_risk = if hu_mean > -70.1 {
        "HIGH".to_string()
    } else {
        "LOW".to_string()
    };

    // 6. Histogram: 100 bins from -200 to 200
    let n_bins = 100;
    let bin_min = -200.0;
    let bin_max = 200.0;
    let bin_width = (bin_max - bin_min) / n_bins as f64;

    let histogram_bins: Vec<f64> = (0..n_bins)
        .map(|i| bin_min + (i as f64 + 0.5) * bin_width)
        .collect();
    let mut histogram_counts = vec![0usize; n_bins];

    for &hu in &voi_values {
        if hu >= bin_min && hu < bin_max {
            let idx = ((hu - bin_min) / bin_width) as usize;
            let idx = idx.min(n_bins - 1);
            histogram_counts[idx] += 1;
        }
    }

    FaiStats {
        vessel: vessel.to_string(),
        n_voi_voxels,
        n_fat_voxels,
        fat_fraction,
        hu_mean,
        hu_std,
        hu_median,
        fai_risk,
        histogram_bins,
        histogram_counts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array3;

    #[test]
    fn test_compute_pcat_stats_basic() {
        // Create a small volume and mask
        let mut vol = Array3::<f32>::zeros((10, 10, 10));
        let mut mask = Array3::<bool>::default((10, 10, 10));

        // Fill some voxels with fat-range HU values
        for z in 2..8 {
            for y in 2..8 {
                for x in 2..8 {
                    vol[[z, y, x]] = -80.0; // typical fat HU
                    mask[[z, y, x]] = true;
                }
            }
        }

        let stats = compute_pcat_stats(&vol, &mask, "LAD", (-190.0, -30.0));

        assert_eq!(stats.vessel, "LAD");
        assert_eq!(stats.n_voi_voxels, 6 * 6 * 6);
        assert_eq!(stats.n_fat_voxels, 6 * 6 * 6); // all are in fat range
        assert!((stats.fat_fraction - 1.0).abs() < 1e-10);
        assert!((stats.hu_mean - (-80.0)).abs() < 0.1);
        assert!(stats.hu_std < 0.01); // all same value
        assert!((stats.hu_median - (-80.0)).abs() < 0.1);
        assert_eq!(stats.fai_risk, "LOW"); // -80 < -70.1
    }

    #[test]
    fn test_compute_pcat_stats_high_risk() {
        let mut vol = Array3::<f32>::zeros((10, 10, 10));
        let mut mask = Array3::<bool>::default((10, 10, 10));

        // HU = -60 is above -70.1 threshold
        for z in 0..5 {
            for y in 0..5 {
                for x in 0..5 {
                    vol[[z, y, x]] = -60.0;
                    mask[[z, y, x]] = true;
                }
            }
        }

        let stats = compute_pcat_stats(&vol, &mask, "RCA", (-190.0, -30.0));
        assert_eq!(stats.fai_risk, "HIGH"); // -60 > -70.1
        assert!((stats.hu_mean - (-60.0)).abs() < 0.1);
    }

    #[test]
    fn test_compute_pcat_stats_empty_voi() {
        let vol = Array3::<f32>::zeros((10, 10, 10));
        let mask = Array3::<bool>::default((10, 10, 10));

        let stats = compute_pcat_stats(&vol, &mask, "LCx", (-190.0, -30.0));

        assert_eq!(stats.n_voi_voxels, 0);
        assert_eq!(stats.n_fat_voxels, 0);
        assert!((stats.fat_fraction - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_compute_pcat_stats_mixed() {
        let mut vol = Array3::<f32>::zeros((10, 10, 10));
        let mut mask = Array3::<bool>::default((10, 10, 10));

        // Mix of fat and non-fat in the VOI
        for x in 0..10 {
            vol[[5, 5, x]] = if x < 5 { -80.0 } else { 50.0 }; // fat vs non-fat
            mask[[5, 5, x]] = true;
        }

        let stats = compute_pcat_stats(&vol, &mask, "LAD", (-190.0, -30.0));
        assert_eq!(stats.n_voi_voxels, 10);
        assert_eq!(stats.n_fat_voxels, 5); // only the -80 HU voxels
        assert!((stats.fat_fraction - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_histogram_bins() {
        let vol = Array3::<f32>::zeros((10, 10, 10));
        let mask = Array3::<bool>::default((10, 10, 10));

        let stats = compute_pcat_stats(&vol, &mask, "LAD", (-190.0, -30.0));

        assert_eq!(stats.histogram_bins.len(), 100);
        assert_eq!(stats.histogram_counts.len(), 100);
        // First bin center should be at -200 + 0.5 * 4 = -198
        assert!((stats.histogram_bins[0] - (-198.0)).abs() < 0.01);
    }
}
