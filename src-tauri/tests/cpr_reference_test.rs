//! Integration test: render curved CPR from real patient data and saved seeds.
//!
//! Loads the DICOM volume from Rahaf_Patients/1200.2, uses the saved seed
//! positions to build a centerline, renders a curved CPR, and saves the
//! output as a raw f32 file + metadata JSON for visual comparison against
//! syngo.via reference.
//!
//! Run with: cargo test --test cpr_reference_test -- --ignored --nocapture

use std::path::Path;

// Import from the library crate
use pcat_workstation_v2_lib::pipeline::cpr::CprFrame;
use pcat_workstation_v2_lib::pipeline::dicom_loader;
use pcat_workstation_v2_lib::pipeline::spline::CubicSpline3D;

/// Seeds saved from the app (RCA, in [x, y, z] cornerstone world coords).
/// We convert to [z, y, x] for Rust.
const SEEDS_XYZ: &[[f64; 3]] = &[
    [44.949, -177.084, 1844.686],
    [43.262, -183.413, 1844.686],
    [40.730, -188.476, 1844.686],
    [36.089, -191.852, 1844.686],
    [31.248, -192.696, 1843.425],
    [23.574, -193.118, 1842.424],
    [17.235, -192.696, 1842.424],
    [12.564, -194.942, 1840.923],
    [9.895, -194.942, 1836.752],
    [10.228, -195.609, 1830.079],
    [12.230, -194.942, 1824.741],
    [14.232, -194.942, 1819.403],
    [15.900, -192.940, 1814.898],
    [18.569, -192.021, 1811.729],
    [20.238, -188.780, 1807.725],
    [24.241, -180.677, 1805.390],
    [28.579, -168.362, 1806.724],
    [37.587, -157.342, 1806.391],
    [45.928, -150.861, 1806.724],
    [48.747, -148.813, 1808.057],
    [49.932, -145.027, 1811.395],
    [54.603, -139.517, 1818.068],
    [58.940, -139.517, 1821.404],
    [65.947, -134.332, 1822.406],
];

#[test]
#[ignore] // Run manually: cargo test --test cpr_reference_test -- --ignored --nocapture
fn render_rca_cpr_from_saved_seeds() {
    let dicom_dir = Path::new("/Users/shunie/Developer/PCAT/Rahaf_Patients/1200.2");
    if !dicom_dir.exists() {
        eprintln!("DICOM directory not found, skipping test");
        return;
    }

    // 1. Load the volume
    println!("Loading DICOM volume from {:?}...", dicom_dir);
    let volume = dicom_loader::load_dicom_directory(dicom_dir)
        .expect("Failed to load DICOM");
    println!(
        "Volume loaded: shape {:?}, spacing {:?}, origin {:?}",
        volume.data.shape(), volume.spacing, volume.origin
    );

    // 2. Convert seeds from [x,y,z] to [z,y,x] for Rust
    let seeds_zyx: Vec<[f64; 3]> = SEEDS_XYZ
        .iter()
        .map(|[x, y, z]| [*z, *y, *x])
        .collect();

    // 3. Fit spline through seeds and sample densely
    let spline = CubicSpline3D::fit(&seeds_zyx);
    let total_arc = spline.total_arc();
    let n_samples = 768;
    let mut centerline = Vec::with_capacity(n_samples);
    for i in 0..n_samples {
        let s = total_arc * (i as f64) / ((n_samples - 1) as f64);
        centerline.push(spline.eval(s));
    }
    println!("Centerline: {} points, total arc = {:.1} mm", centerline.len(), total_arc);

    // 4. Build CPR frame
    let frame = CprFrame::from_centerline(&centerline, n_samples);
    println!("Frame built: {} columns", frame.n_cols());

    // 5. Render curved CPR at multiple rotation angles
    let width_mm = 40.0;
    let pixels_wide = 768;
    let pixels_high = 384;
    let slab_mm = 1.0;

    let output_dir = Path::new("/Users/shunie/Developer/PCAT/pcat-workstation-v2/test_output");
    std::fs::create_dir_all(output_dir).unwrap();

    for rotation_deg in [0.0, 90.0, 180.0, 270.0] {
        let result = frame.render_curved_cpr(
            &volume.data,
            volume.spacing,
            volume.origin,
            rotation_deg,
            width_mm,
            pixels_wide,
            pixels_high,
            slab_mm,
        );

        assert_eq!(result.image.len(), pixels_wide * pixels_high);

        // Count NaN pixels and valid range
        let valid: Vec<f32> = result.image.iter().copied().filter(|v| !v.is_nan()).collect();
        let nan_count = result.image.len() - valid.len();
        let nan_pct = 100.0 * nan_count as f64 / result.image.len() as f64;

        let (vmin, vmax) = if valid.is_empty() {
            (f32::NAN, f32::NAN)
        } else {
            let vmin = valid.iter().copied().fold(f32::INFINITY, f32::min);
            let vmax = valid.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            (vmin, vmax)
        };

        println!(
            "Rotation {:.0}°: {}x{}, {:.1}% NaN, HU range [{:.0}, {:.0}]",
            rotation_deg, result.pixels_wide, result.pixels_high,
            nan_pct, vmin, vmax
        );

        // Save raw f32 image
        let filename = format!("rca_curved_rot{:.0}.raw", rotation_deg);
        let raw_path = output_dir.join(&filename);
        let bytes: &[u8] = bytemuck::cast_slice(&result.image);
        std::fs::write(&raw_path, bytes).unwrap();

        // Save metadata
        let meta = serde_json::json!({
            "width": result.pixels_wide,
            "height": result.pixels_high,
            "rotation_deg": rotation_deg,
            "width_mm": width_mm,
            "nan_percent": nan_pct,
            "hu_min": vmin,
            "hu_max": vmax,
            "total_arc_mm": total_arc,
        });
        let meta_path = output_dir.join(format!("rca_curved_rot{:.0}.json", rotation_deg));
        std::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap()).unwrap();
    }

    // 6. Also render straightened CPR for comparison
    let straight = frame.render_cpr(
        &volume.data,
        volume.spacing,
        volume.origin,
        0.0,
        width_mm,
        pixels_high,
        slab_mm,
    );

    let valid: Vec<f32> = straight.image.iter().copied().filter(|v| !v.is_nan()).collect();
    let nan_pct = 100.0 * (straight.image.len() - valid.len()) as f64 / straight.image.len() as f64;
    println!(
        "Straightened: {}x{}, {:.1}% NaN",
        straight.pixels_wide, straight.pixels_high, nan_pct
    );

    let bytes: &[u8] = bytemuck::cast_slice(&straight.image);
    std::fs::write(output_dir.join("rca_straightened.raw"), bytes).unwrap();
    let meta = serde_json::json!({
        "width": straight.pixels_wide,
        "height": straight.pixels_high,
        "rotation_deg": 0.0,
        "width_mm": width_mm,
        "nan_percent": nan_pct,
        "total_arc_mm": total_arc,
    });
    std::fs::write(
        output_dir.join("rca_straightened.json"),
        serde_json::to_string_pretty(&meta).unwrap(),
    ).unwrap();

    println!("\nOutput saved to {:?}", output_dir);
    println!("View with: python3 -c \"import numpy as np; from PIL import Image; d=np.fromfile('test_output/rca_curved_rot0.raw',np.float32).reshape(384,768); d=np.nan_to_num(d,-1024); d=np.clip((d+200)/600*255,0,255).astype(np.uint8); Image.fromarray(d).save('test_output/rca_curved_rot0.png')\"");
}
