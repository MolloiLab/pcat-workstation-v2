mod common;

use pcat_pipeline::dicom_scan::scan_series;

#[tokio::test]
async fn scan_returns_one_series_for_mini_ct() {
    let dir = tempfile::tempdir().unwrap();
    common::write_mini_ct(dir.path());

    let series = scan_series(dir.path()).await.unwrap();
    assert_eq!(series.len(), 1);

    let s = &series[0];
    assert_eq!(s.uid, "1.2.826.0.1.3680043.99.1");
    assert_eq!(s.description, "MiniCT");
    assert_eq!(s.num_slices, common::FIXTURE_SLICES);
    assert_eq!(s.rows, common::FIXTURE_ROWS);
    assert_eq!(s.cols, common::FIXTURE_COLS);
    assert_eq!(s.file_paths.len(), common::FIXTURE_SLICES);

    // Files are z-sorted
    let zs: Vec<f64> = (0..common::FIXTURE_SLICES)
        .map(|i| i as f64 * common::FIXTURE_SPACING_Z)
        .collect();
    for (i, p) in s.file_paths.iter().enumerate() {
        assert!(
            p.to_string_lossy().contains(&format!("slice_{:03}", i)),
            "slice at index {i} is {p:?}, expected contains slice_{:03}",
            i
        );
        assert!((s.slice_positions_z[i] - zs[i]).abs() < 1e-9);
    }
}

#[tokio::test]
async fn scan_returns_two_series_for_dual_folder() {
    let dir = tempfile::tempdir().unwrap();
    common::write_mini_ct(dir.path());
    common::write_second_series(dir.path());

    let mut series = scan_series(dir.path()).await.unwrap();
    series.sort_by(|a, b| a.uid.cmp(&b.uid));
    assert_eq!(series.len(), 2);
    assert_eq!(series[1].description, "MonoPlus 70 keV");
    assert_eq!(series[1].image_comments.as_deref(), Some("E = 70 keV"));
}

#[tokio::test]
async fn scan_silently_skips_non_dicom_files() {
    let dir = tempfile::tempdir().unwrap();
    common::write_mini_ct(dir.path());
    std::fs::write(dir.path().join(".DS_Store"), b"junk").unwrap();
    std::fs::write(dir.path().join("README.txt"), b"hello").unwrap();

    let series = scan_series(dir.path()).await.unwrap();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].num_slices, common::FIXTURE_SLICES);
}

#[tokio::test]
async fn scan_errors_when_folder_missing() {
    let err = scan_series(std::path::Path::new("/nonexistent/path/xyz"))
        .await
        .unwrap_err();
    let s = format!("{err}");
    assert!(s.contains("folder not readable") || s.contains("No such file"));
}
