//! Integration test: run MMD on real patient mono-energetic DICOM data.
//! Verifies outputs are physically reasonable.
//!
//! NOTE: Temporarily disabled — the `mmd` module has not been implemented
//! in `pcat-pipeline` yet. Re-enable once it lands.

// TODO: uncomment when pcat_pipeline::mmd is implemented
/*
use std::path::Path;
use pcat_pipeline::dicom_loader::load_dicom_directory;
use pcat_pipeline::mmd::{self, MmdConfig};

#[test]
fn test_mmd_real_patient_57955439() {
    let base = "/Users/shunie/Developer/PCAT/UCI NAEOTOM CCTA Data/57955439";
    let dirs = [
        format!("{}/MonoPlus_70keV", base),
        format!("{}/MonoPlus_100keV", base),
        format!("{}/MonoPlus_140keV", base),
        format!("{}/MonoPlus_150keV", base),
    ];

    // Check data exists.
    for d in &dirs {
        if !Path::new(d).exists() {
            eprintln!("Skipping: data not found at {d}");
            return;
        }
    }

    // Load volumes.
    let vols: Vec<_> = dirs.iter().map(|d| {
        load_dicom_directory(Path::new(d)).expect(&format!("Failed to load {d}"))
    }).collect();

    let shape = vols[0].data.raw_dim();
    println!("Volume shape: {:?}", shape);
    println!("Spacing: {:?}", vols[0].spacing);

    // Verify all shapes match.
    for (i, v) in vols.iter().enumerate() {
        assert_eq!(v.data.raw_dim(), shape, "Shape mismatch at energy {i}");
    }

    // Run decomposition.
    let config = MmdConfig::default();
    println!("Iodine concentration: {} mg/mL", config.iodine_concentration_mg_ml);

    let refs = [&*vols[0].data, &*vols[1].data, &*vols[2].data, &*vols[3].data];
    let result = mmd::decompose(refs, &config, |p| {
        if (p * 100.0) as u32 % 25 == 0 {
            // print every 25%
        }
    });

    // Compute statistics.
    let n = result.water.len() as f64;
    let mean_w: f64 = result.water.iter().map(|&v| v as f64).sum::<f64>() / n;
    let mean_l: f64 = result.lipid.iter().map(|&v| v as f64).sum::<f64>() / n;
    let mean_i: f64 = result.iodine.iter().map(|&v| v as f64).sum::<f64>() / n;
    let mean_r: f64 = result.residual.iter().map(|&v| v as f64).sum::<f64>() / n;

    println!("\n=== MMD Results ===");
    println!("Mean water:    {:.1}%", mean_w * 100.0);
    println!("Mean lipid:    {:.1}%", mean_l * 100.0);
    println!("Mean iodine:   {:.1}%", mean_i * 100.0);
    println!("Mean residual: {:.6} cm-1", mean_r);
    println!("Sum check:     {:.1}%", (mean_w + mean_l + mean_i) * 100.0);

    // Sample a middle axial slice for spot checks.
    let mid_z = shape[0] / 2;
    let mid_y = shape[1] / 2;
    let mid_x = shape[2] / 2;

    let center_w = result.water[[mid_z, mid_y, mid_x]];
    let center_l = result.lipid[[mid_z, mid_y, mid_x]];
    let center_i = result.iodine[[mid_z, mid_y, mid_x]];
    let center_hu_70 = vols[0].data[[mid_z, mid_y, mid_x]];

    println!("\nCenter voxel [{mid_z},{mid_y},{mid_x}]:");
    println!("  HU@70keV: {:.1}", center_hu_70);
    println!("  water: {:.1}%, lipid: {:.1}%, iodine: {:.1}%",
        center_w * 100.0, center_l * 100.0, center_i * 100.0);

    // Check a voxel in the fat region (subcutaneous fat, typically outer ring).
    // Fat should be: high lipid, low water, low iodine.
    // Look for a voxel with HU ~ -80 to -100 at 70 keV.
    let mut fat_count = 0u64;
    let mut fat_lipid_sum = 0.0_f64;
    let mut water_count = 0u64;
    let mut water_water_sum = 0.0_f64;
    let mut enhanced_count = 0u64;
    let mut enhanced_iodine_sum = 0.0_f64;

    let v70 = &*vols[0].data;
    for z in 0..shape[0] {
        for y in 0..shape[1] {
            for x in 0..shape[2] {
                let hu70 = v70[[z, y, x]];
                if hu70 > -120.0 && hu70 < -50.0 {
                    // Fat-like voxel.
                    fat_count += 1;
                    fat_lipid_sum += result.lipid[[z, y, x]] as f64;
                }
                if hu70 > -10.0 && hu70 < 30.0 {
                    // Water-like voxel (soft tissue near 0 HU).
                    water_count += 1;
                    water_water_sum += result.water[[z, y, x]] as f64;
                }
                if hu70 > 200.0 && hu70 < 500.0 {
                    // Contrast-enhanced voxel.
                    enhanced_count += 1;
                    enhanced_iodine_sum += result.iodine[[z, y, x]] as f64;
                }
            }
        }
    }

    println!("\n=== Tissue-specific validation ===");
    if fat_count > 0 {
        let avg = fat_lipid_sum / fat_count as f64;
        println!("Fat voxels (HU -120..-50): n={fat_count}, mean lipid fraction: {:.1}%", avg * 100.0);
        assert!(avg > 0.3, "Fat voxels should have lipid > 30%, got {:.1}%", avg * 100.0);
    }
    if water_count > 0 {
        let avg = water_water_sum / water_count as f64;
        println!("Water-like voxels (HU -10..30): n={water_count}, mean water fraction: {:.1}%", avg * 100.0);
        assert!(avg > 0.3, "Water-like voxels should have water > 30%, got {:.1}%", avg * 100.0);
    }
    if enhanced_count > 0 {
        let avg = enhanced_iodine_sum / enhanced_count as f64;
        println!("Enhanced voxels (HU 200..500): n={enhanced_count}, mean iodine fraction: {:.1}%", avg * 100.0);
        assert!(avg > 0.1, "Enhanced voxels should have iodine > 10%, got {:.1}%", avg * 100.0);
    }

    println!("\nAll tissue-specific checks passed.");
}
*/
