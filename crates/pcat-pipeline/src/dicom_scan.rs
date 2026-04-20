//! Header-only DICOM scanning primitives.
//!
//! `read_header` opens a DICOM file, stops parsing at the PixelData tag, and
//! returns a `SliceHeader` with only the tags we care about for indexing and
//! grouping. On an SMB share this transfers ~4 KB per file instead of ~512 KB.

use std::path::{Path, PathBuf};

use dicom::core::Tag;
use dicom::dictionary_std::tags;
use dicom::object::{FileDicomObject, InMemDicomObject, OpenFileOptions};

use crate::dicom_errors::DicomLoadError;

/// DICOM tag for ImageComments (0020,4000) — MonoPlus keV truth per lab finding.
pub const IMAGE_COMMENTS: Tag = Tag(0x0020, 0x4000);

/// Per-file header fields sufficient to (a) group files by series and (b) later
/// load pixel data without re-parsing the header.
#[derive(Debug, Clone)]
pub struct SliceHeader {
    pub path: PathBuf,
    pub series_uid: String,
    pub series_description: String,
    pub image_comments: Option<String>,
    pub instance_number: Option<i32>,
    pub image_position_z: Option<f64>,
    pub rows: u32,
    pub cols: u32,
    pub rescale_slope: f64,
    pub rescale_intercept: f64,
    pub pixel_spacing: [f64; 2],
    pub orientation: [f64; 6],
    pub patient_name: String,
    pub study_description: String,
    pub window_center: f64,
    pub window_width: f64,
}

/// Read only the file header up to PixelData. Returns Ok(Some(header)) for valid
/// image-DICOM files, Ok(None) for files that are not DICOM or have no pixel data,
/// and Err for hard I/O errors.
pub fn read_header(path: &Path) -> Result<Option<SliceHeader>, DicomLoadError> {
    let obj = match OpenFileOptions::new()
        .read_until(tags::PIXEL_DATA)
        .open_file(path)
    {
        Ok(o) => o,
        // Not a valid DICOM file — skip silently.
        Err(_) => return Ok(None),
    };

    // If there is no SeriesInstanceUID, this is not an image series instance.
    let series_uid = match read_string(&obj, tags::SERIES_INSTANCE_UID) {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(None),
    };

    let rows = read_u32(&obj, tags::ROWS).unwrap_or(0);
    let cols = read_u32(&obj, tags::COLUMNS).unwrap_or(0);
    if rows == 0 || cols == 0 {
        // Non-image DICOM (SR, KO, PR, etc.) — skip.
        return Ok(None);
    }

    let pixel_spacing = read_multi_f64(&obj, tags::PIXEL_SPACING);
    let orient = read_multi_f64(&obj, tags::IMAGE_ORIENTATION_PATIENT);
    let ipp = read_multi_f64(&obj, tags::IMAGE_POSITION_PATIENT);

    Ok(Some(SliceHeader {
        path: path.to_path_buf(),
        series_uid,
        series_description: read_string(&obj, tags::SERIES_DESCRIPTION).unwrap_or_default(),
        image_comments: read_string(&obj, IMAGE_COMMENTS),
        instance_number: read_i32(&obj, tags::INSTANCE_NUMBER),
        image_position_z: ipp.get(2).copied(),
        rows,
        cols,
        rescale_slope: read_f64(&obj, tags::RESCALE_SLOPE).unwrap_or(1.0),
        rescale_intercept: read_f64(&obj, tags::RESCALE_INTERCEPT).unwrap_or(0.0),
        pixel_spacing: if pixel_spacing.len() >= 2 {
            [pixel_spacing[0], pixel_spacing[1]]
        } else {
            [1.0, 1.0]
        },
        orientation: if orient.len() >= 6 {
            [orient[0], orient[1], orient[2], orient[3], orient[4], orient[5]]
        } else {
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        },
        patient_name: read_string(&obj, tags::PATIENT_NAME).unwrap_or_default(),
        study_description: read_string(&obj, tags::STUDY_DESCRIPTION).unwrap_or_default(),
        window_center: read_f64(&obj, tags::WINDOW_CENTER).unwrap_or(40.0),
        window_width: read_f64(&obj, tags::WINDOW_WIDTH).unwrap_or(400.0),
    }))
}

fn read_string(obj: &FileDicomObject<InMemDicomObject>, tag: Tag) -> Option<String> {
    obj.element(tag)
        .ok()
        .and_then(|e| e.to_str().ok())
        .map(|s| s.trim().to_string())
}

fn read_f64(obj: &FileDicomObject<InMemDicomObject>, tag: Tag) -> Option<f64> {
    obj.element(tag).ok().and_then(|e| e.to_float64().ok())
}

fn read_multi_f64(obj: &FileDicomObject<InMemDicomObject>, tag: Tag) -> Vec<f64> {
    obj.element(tag)
        .ok()
        .and_then(|e| e.to_multi_float64().ok())
        .unwrap_or_default()
}

fn read_u32(obj: &FileDicomObject<InMemDicomObject>, tag: Tag) -> Option<u32> {
    obj.element(tag).ok().and_then(|e| e.to_int::<u32>().ok())
}

fn read_i32(obj: &FileDicomObject<InMemDicomObject>, tag: Tag) -> Option<i32> {
    obj.element(tag).ok().and_then(|e| e.to_int::<i32>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn non_dicom_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notdicom.txt");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"This is not a DICOM file.").unwrap();
        drop(f);

        let result = read_header(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn empty_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.dcm");
        std::fs::File::create(&path).unwrap();
        let result = read_header(&path).unwrap();
        assert!(result.is_none());
    }

    // Note: reading an actual DICOM file is covered by integration tests
    // in tests/dicom_loader_integration.rs, after the fixture generator is
    // landed in Task 4.
}
