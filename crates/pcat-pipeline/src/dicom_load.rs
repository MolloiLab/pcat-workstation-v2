//! Parallel pixel decode + volume assembly.
//!
//! Given a folder and a series UID, walks headers (to locate files), then
//! decodes pixel data in parallel (rayon) and returns a densely packed i16
//! volume in z-major order.

use std::path::Path;

use rayon::prelude::*;

use crate::dicom_decode::decode_slice_i16;
use crate::dicom_errors::DicomLoadError;
use crate::dicom_scan::{scan_series, SeriesDescriptor};

/// 4 GB soft limit (conservative — covers 1000-slice 512² i16 at 1.5 GB).
const VOLUME_SIZE_LIMIT_MB: usize = 4096;

/// Metadata subset that travels with a loaded volume's pixel bytes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VolumeMetadata {
    pub series_uid: String,
    pub series_description: String,
    pub image_comments: Option<String>,
    pub rows: u32,
    pub cols: u32,
    pub num_slices: usize,
    pub pixel_spacing: [f64; 2],
    pub slice_spacing: f64,
    pub orientation: [f64; 6],
    pub window_center: f64,
    pub window_width: f64,
    pub patient_name: String,
    pub study_description: String,
    pub slice_positions_z: Vec<f64>,
}

impl From<&SeriesDescriptor> for VolumeMetadata {
    fn from(d: &SeriesDescriptor) -> Self {
        Self {
            series_uid: d.uid.clone(),
            series_description: d.description.clone(),
            image_comments: d.image_comments.clone(),
            rows: d.rows,
            cols: d.cols,
            num_slices: d.num_slices,
            pixel_spacing: d.pixel_spacing,
            slice_spacing: d.slice_spacing,
            orientation: d.orientation,
            window_center: d.window_center,
            window_width: d.window_width,
            patient_name: d.patient_name.clone(),
            study_description: d.study_description.clone(),
            slice_positions_z: d.slice_positions_z.clone(),
        }
    }
}

#[derive(Debug)]
pub struct LoadedVolume {
    pub metadata: VolumeMetadata,
    /// Tightly packed i16, z-major order: `voxels_i16[z * rows * cols + r * cols + c]`.
    pub voxels_i16: Vec<i16>,
}

/// Return Err if a planned volume would exceed the size limit.
pub fn check_volume_size_mb(requested_mb: usize) -> Result<(), DicomLoadError> {
    if requested_mb > VOLUME_SIZE_LIMIT_MB {
        Err(DicomLoadError::VolumeTooLarge {
            requested_mb,
            limit_mb: VOLUME_SIZE_LIMIT_MB,
        })
    } else {
        Ok(())
    }
}

/// Load a single series by UID. Runs `scan_series` to find the descriptor, then
/// rayon-parallel decodes all slices.
pub async fn load_series(
    dir: &Path,
    uid: &str,
) -> Result<LoadedVolume, DicomLoadError> {
    let descriptors = scan_series(dir).await?;
    let desc = descriptors
        .into_iter()
        .find(|d| d.uid == uid)
        .ok_or_else(|| DicomLoadError::SeriesNotFound { uid: uid.to_string() })?;

    let slice_len = (desc.rows as usize) * (desc.cols as usize);
    let total_voxels = slice_len * desc.num_slices;
    let total_bytes_mb = (total_voxels * 2) / (1024 * 1024);
    check_volume_size_mb(total_bytes_mb)?;

    let file_paths = desc.file_paths.clone();
    let rescale_slope = desc.rescale_slope;
    let rescale_intercept = desc.rescale_intercept;
    let rows = desc.rows;
    let cols = desc.cols;
    let dir_owned = dir.to_path_buf();

    let voxels = tokio::task::spawn_blocking(move || {
        let mut out = vec![0i16; total_voxels];
        let results: Vec<Result<(usize, Vec<i16>), DicomLoadError>> = file_paths
            .par_iter()
            .enumerate()
            .map(|(z, p)| {
                decode_slice_i16(p, rescale_slope, rescale_intercept, rows, cols)
                    .map(|px| (z, px))
            })
            .collect();
        for r in results {
            let (z, px) = r?;
            out[z * slice_len..(z + 1) * slice_len].copy_from_slice(&px);
        }
        Ok::<_, DicomLoadError>(out)
    })
    .await
    .map_err(|e| DicomLoadError::ParseFailed {
        path: dir_owned,
        reason: format!("decode task panicked: {e}"),
    })??;

    Ok(LoadedVolume {
        metadata: VolumeMetadata::from(&desc),
        voxels_i16: voxels,
    })
}
