use std::path::Path;

use dicom::dictionary_std::tags;
use dicom::object::{open_file, FileDicomObject, InMemDicomObject};
use dicom_pixeldata::PixelDecoder;
use ndarray::Array3;
use walkdir::WalkDir;

use crate::error::AppError;
use crate::state::LoadedVolume;

/// Per-slice metadata extracted from a DICOM file.
struct SliceInfo {
    obj: FileDicomObject<InMemDicomObject>,
    z_position: f64,
    rescale_slope: f64,
    rescale_intercept: f64,
}

// ---------------------------------------------------------------------------
// Tag helper functions
// ---------------------------------------------------------------------------

/// Read a multi-value f64 tag, returning an empty vec on failure.
fn read_multi_f64(obj: &FileDicomObject<InMemDicomObject>, tag: dicom::core::Tag) -> Vec<f64> {
    obj.element(tag)
        .ok()
        .and_then(|e| e.to_multi_float64().ok())
        .unwrap_or_default()
}

/// Read a single f64 tag with a fallback default.
fn read_f64(obj: &FileDicomObject<InMemDicomObject>, tag: dicom::core::Tag, default: f64) -> f64 {
    obj.element(tag)
        .ok()
        .and_then(|e| e.to_float64().ok())
        .unwrap_or(default)
}

/// Read a string tag with a fallback default.
fn read_string(
    obj: &FileDicomObject<InMemDicomObject>,
    tag: dicom::core::Tag,
    default: &str,
) -> String {
    obj.element(tag)
        .ok()
        .and_then(|e| e.to_str().ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| default.to_string())
}

/// Read a u16 tag with a fallback default.
fn read_u16(obj: &FileDicomObject<InMemDicomObject>, tag: dicom::core::Tag, default: u16) -> u16 {
    obj.element(tag)
        .ok()
        .and_then(|e| e.to_int::<u16>().ok())
        .unwrap_or(default)
}

// ---------------------------------------------------------------------------
// Pixel data extraction
// ---------------------------------------------------------------------------

/// Decode pixel data from a DICOM object, apply rescale slope/intercept to
/// convert to Hounsfield Units, and clamp outliers.
///
/// Returns a flat `Vec<f32>` of length `rows * cols`.
fn decode_slice_hu(
    obj: &FileDicomObject<InMemDicomObject>,
    rescale_slope: f64,
    rescale_intercept: f64,
    expected_len: usize,
) -> Result<Vec<f32>, AppError> {
    let pixel_data = obj
        .decode_pixel_data()
        .map_err(|e| AppError::Dicom(format!("pixel decode error: {e}")))?;

    let pixel_repr = read_u16(obj, tags::PIXEL_REPRESENTATION, 0);

    let hu_values: Vec<f32> = if pixel_repr == 1 {
        // Signed 16-bit
        let raw: Vec<i16> = pixel_data
            .data()
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        raw.iter()
            .map(|&v| {
                let hu = v as f64 * rescale_slope + rescale_intercept;
                clamp_hu(hu) as f32
            })
            .collect()
    } else {
        // Unsigned 16-bit
        let raw: Vec<u16> = pixel_data
            .data()
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        raw.iter()
            .map(|&v| {
                let hu = v as f64 * rescale_slope + rescale_intercept;
                clamp_hu(hu) as f32
            })
            .collect()
    };

    if hu_values.len() != expected_len {
        return Err(AppError::Dicom(format!(
            "pixel count mismatch: got {} expected {}",
            hu_values.len(),
            expected_len
        )));
    }

    Ok(hu_values)
}

/// Clamp HU values: anything <= -8192 becomes -1024, anything > 3095 becomes 3095.
#[inline]
fn clamp_hu(hu: f64) -> f64 {
    if hu <= -8192.0 {
        -1024.0
    } else if hu > 3095.0 {
        3095.0
    } else {
        hu
    }
}

// ---------------------------------------------------------------------------
// Direction matrix from ImageOrientationPatient
// ---------------------------------------------------------------------------

/// Build a 3x3 direction cosine matrix (row-major, 9 elements) from
/// ImageOrientationPatient (6 elements: row_x, row_y, row_z, col_x, col_y, col_z).
/// The third row is the cross product of the row and column direction vectors.
fn build_direction_matrix(iop: &[f64]) -> [f64; 9] {
    if iop.len() < 6 {
        // Fallback: identity matrix
        return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    }
    let (rx, ry, rz) = (iop[0], iop[1], iop[2]);
    let (cx, cy, cz) = (iop[3], iop[4], iop[5]);

    // Cross product: normal = row x col
    let nx = ry * cz - rz * cy;
    let ny = rz * cx - rx * cz;
    let nz = rx * cy - ry * cx;

    [rx, ry, rz, cx, cy, cz, nx, ny, nz]
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load all DICOM slices from `dir`, sort by Z position, apply HU rescaling,
/// and return a `LoadedVolume`.
pub fn load_dicom_directory(dir: &Path) -> Result<LoadedVolume, AppError> {
    // 1. Scan directory and collect parseable DICOM files with per-slice metadata.
    let mut slices: Vec<SliceInfo> = Vec::new();

    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let obj = match open_file(entry.path()) {
            Ok(o) => o,
            Err(_) => continue, // skip non-DICOM files silently
        };

        let ipp = read_multi_f64(&obj, tags::IMAGE_POSITION_PATIENT);
        if ipp.len() < 3 {
            continue; // not a valid image slice
        }

        let rescale_slope = read_f64(&obj, tags::RESCALE_SLOPE, 1.0);
        let rescale_intercept = read_f64(&obj, tags::RESCALE_INTERCEPT, 0.0);

        slices.push(SliceInfo {
            obj,
            z_position: ipp[2],
            rescale_slope,
            rescale_intercept,
        });
    }

    if slices.is_empty() {
        return Err(AppError::Dicom(
            "no valid DICOM image slices found in directory".to_string(),
        ));
    }

    // 2. Sort by Z position ascending.
    slices.sort_by(|a, b| {
        a.z_position
            .partial_cmp(&b.z_position)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 3. Extract metadata from the first slice (representative).
    let first = &slices[0].obj;

    let rows = read_u16(first, tags::ROWS, 0) as usize;
    let cols = read_u16(first, tags::COLUMNS, 0) as usize;
    if rows == 0 || cols == 0 {
        return Err(AppError::Dicom("invalid image dimensions".to_string()));
    }

    let pixel_spacing = read_multi_f64(first, tags::PIXEL_SPACING);
    let (sy, sx) = if pixel_spacing.len() >= 2 {
        (pixel_spacing[0], pixel_spacing[1])
    } else {
        (1.0, 1.0)
    };

    let sz = if slices.len() > 1 {
        (slices[1].z_position - slices[0].z_position).abs()
    } else {
        1.0
    };

    let ipp_first = read_multi_f64(first, tags::IMAGE_POSITION_PATIENT);
    let origin = if ipp_first.len() >= 3 {
        [ipp_first[2], ipp_first[1], ipp_first[0]] // [oz, oy, ox]
    } else {
        [0.0, 0.0, 0.0]
    };

    let iop = read_multi_f64(first, tags::IMAGE_ORIENTATION_PATIENT);
    let direction = build_direction_matrix(&iop);

    let window_center = read_f64(first, tags::WINDOW_CENTER, 40.0);
    let window_width = read_f64(first, tags::WINDOW_WIDTH, 400.0);
    let patient_name = read_string(first, tags::PATIENT_NAME, "");
    let study_description = read_string(first, tags::STUDY_DESCRIPTION, "");

    // 4. Decode pixel data for every slice and stack into Array3.
    let n_slices = slices.len();
    let slice_len = rows * cols;
    let mut volume_data: Vec<f32> = Vec::with_capacity(n_slices * slice_len);

    for (i, si) in slices.iter().enumerate() {
        let hu = decode_slice_hu(&si.obj, si.rescale_slope, si.rescale_intercept, slice_len)
            .map_err(|e| AppError::Dicom(format!("slice {i}: {e}")))?;
        volume_data.extend_from_slice(&hu);
    }

    let data = Array3::from_shape_vec((n_slices, rows, cols), volume_data).map_err(|e| {
        AppError::Dicom(format!("failed to build 3D array: {e}"))
    })?;

    Ok(LoadedVolume {
        data,
        spacing: [sz, sy, sx],
        origin,
        direction,
        window_center,
        window_width,
        patient_name,
        study_description,
    })
}
