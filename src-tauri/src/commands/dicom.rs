use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use tauri::ipc::Response;

use pcat_pipeline::dicom_loader::{self, SeriesInfo};
use pcat_pipeline::dicom_scan::{self, SeriesDescriptor};
use pcat_pipeline::dicom_load;
use crate::state::AppState;
use crate::commands::framed::encode_frame;

const MAX_RECENT: usize = 10;

fn recent_paths_file(app: &tauri::AppHandle) -> PathBuf {
    let dir = app.path().app_data_dir().expect("app data dir");
    dir.join("recent_dicoms.json")
}

fn load_recent_list(app: &tauri::AppHandle) -> Vec<String> {
    let path = recent_paths_file(app);
    if let Ok(data) = std::fs::read_to_string(&path) {
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        Vec::new()
    }
}

fn save_recent_list(app: &tauri::AppHandle, paths: &[String]) {
    let path = recent_paths_file(app);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, serde_json::to_string_pretty(paths).unwrap_or_default());
}

fn push_recent(app: &tauri::AppHandle, new_path: &str) {
    let mut list = load_recent_list(app);
    list.retain(|p| p != new_path);
    list.insert(0, new_path.to_string());
    list.truncate(MAX_RECENT);
    save_recent_list(app, &list);
}

#[derive(serde::Serialize)]
pub struct VolumeInfo {
    pub shape: [usize; 3],
    pub spacing: [f64; 3],
    pub origin: [f64; 3],
    pub direction: [f64; 9],
    pub window_center: f64,
    pub window_width: f64,
    pub patient_name: String,
    pub study_description: String,
}

/// Opens a native folder-picker dialog. Returns the selected path as a string,
/// or `None` if the user cancelled.
#[tauri::command]
pub async fn open_dicom_dialog(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let path = tokio::task::spawn_blocking(move || {
        app.dialog().file().blocking_pick_folder()
    })
    .await
    .map_err(|e| format!("dialog task failed: {e}"))?;

    Ok(path.map(|p| p.to_string()))
}

/// Loads all DICOM slices from `path`, stores the volume in AppState,
/// and returns summary metadata.
#[tauri::command]
pub async fn load_dicom(
    path: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<VolumeInfo, String> {
    let dir = path.clone();
    push_recent(&app, &path);

    let volume = tokio::task::spawn_blocking(move || {
        dicom_loader::load_dicom_directory(Path::new(&dir))
    })
    .await
    .map_err(|e| format!("load task failed: {e}"))?
    .map_err(|e| e.to_string())?;

    let info = VolumeInfo {
        shape: [
            volume.data.shape()[0],
            volume.data.shape()[1],
            volume.data.shape()[2],
        ],
        spacing: volume.spacing,
        origin: volume.origin,
        direction: volume.direction,
        window_center: volume.window_center,
        window_width: volume.window_width,
        patient_name: volume.patient_name.clone(),
        study_description: volume.study_description.clone(),
    };

    // Store in app state
    let mut guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
    guard.volume = Some(volume);

    Ok(info)
}

/// Return the list of recently opened DICOM folder paths.
#[tauri::command]
pub async fn get_recent_dicoms(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    Ok(load_recent_list(&app))
}

/// Sanitize a path string into a safe filename component.
fn sanitize_for_filename(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect::<String>()
        .replace("..", "_")
}

/// Save seeds JSON keyed by DICOM folder path.
#[tauri::command]
pub async fn save_seeds(app: tauri::AppHandle, seeds_json: String, dicom_path: String) -> Result<String, String> {
    let dir = app.path().app_data_dir().expect("app data dir").join("seeds");
    let _ = std::fs::create_dir_all(&dir);
    // Use last path component as human-readable key
    let key = Path::new(&dicom_path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| sanitize_for_filename(&dicom_path));
    let path = dir.join(format!("{}.json", sanitize_for_filename(&key)));
    std::fs::write(&path, &seeds_json).map_err(|e| format!("write failed: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// Load seeds JSON keyed by DICOM folder path.
#[tauri::command]
pub async fn load_seeds(app: tauri::AppHandle, dicom_path: String) -> Result<Option<String>, String> {
    let dir = app.path().app_data_dir().expect("app data dir").join("seeds");
    let key = Path::new(&dicom_path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| sanitize_for_filename(&dicom_path));
    let path = dir.join(format!("{}.json", sanitize_for_filename(&key)));
    if path.exists() {
        let data = std::fs::read_to_string(&path).map_err(|e| format!("read failed: {e}"))?;
        Ok(Some(data))
    } else {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Dual-energy DICOM commands
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct DualEnergyInfo {
    pub shape: [usize; 3],
    pub spacing: [f64; 3],
    pub low_kev: f64,
    pub high_kev: f64,
    pub patient_name: String,
}

/// Scan a DICOM directory and return the list of series found.
#[tauri::command]
pub async fn scan_series(
    path: String,
) -> Result<Vec<SeriesInfo>, String> {
    let dir = path;
    tokio::task::spawn_blocking(move || {
        dicom_loader::scan_dicom_series(Path::new(&dir))
    })
    .await
    .map_err(|e| format!("scan task failed: {e}"))?
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Patient browser: list patient folders + status from saved annotations
// ---------------------------------------------------------------------------

/// Per-patient progress summary for the patient browser.
#[derive(serde::Serialize)]
pub struct PatientInfo {
    /// Folder name (e.g. "57955439"), used as a stable patient ID.
    pub id: String,
    /// Absolute path to the patient's DICOM folder.
    pub path: String,
    /// `not_started` | `in_progress` | `complete`.
    pub status: String,
    /// Number of cross-sections marked finalized in saved annotations (0 if none).
    pub finalized_count: usize,
    /// Whether MMD has been run and stored in saved annotations.
    pub has_mmd: bool,
}

/// Decide whether a directory looks like a patient folder.
///
/// Heuristic: name is non-hidden, contains at least one regular file (we don't
/// scan deeply for DICOM headers — that would be slow over SMB).
fn looks_like_patient_dir(entry: &std::fs::DirEntry) -> bool {
    if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
        return false;
    }
    let name = entry.file_name();
    let name = name.to_string_lossy();
    if name.starts_with('.') || name.starts_with('_') {
        return false;
    }
    // Quick check: directory must contain at least one regular file (DICOM slice).
    if let Ok(mut iter) = std::fs::read_dir(entry.path()) {
        iter.any(|e| e.ok().and_then(|e| e.file_type().ok()).is_some_and(|t| t.is_file()))
    } else {
        false
    }
}

/// Reads saved annotation JSON for a given patient folder name and returns
/// (finalized_count, has_mmd). Returns (0, false) if no save exists.
fn read_annotation_summary(app: &tauri::AppHandle, folder_name: &str) -> (usize, bool) {
    let dir = app.path().app_data_dir().expect("app data dir").join("annotations");
    // Filename is `sanitize(folder_name).json` — match the convention in
    // `save_annotations` / `load_annotations`.
    let file = dir.join(format!("{}.json", sanitize_for_filename(folder_name)));
    let Ok(data) = std::fs::read_to_string(&file) else {
        return (0, false);
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) else {
        return (0, false);
    };
    let finalized_count = json
        .get("finalized")
        .and_then(|v| v.as_object())
        .map(|m| m.values().filter(|v| v.as_bool().unwrap_or(false)).count())
        .unwrap_or(0);
    let has_mmd = json
        .get("mmd_method")
        .map(|v| !v.is_null())
        .unwrap_or(false);
    (finalized_count, has_mmd)
}

/// Walk `root_dir` and return a sorted list of patient folders with status badges.
///
/// Status: `complete` if MMD has been run AND ≥1 contour finalized,
/// `in_progress` if any annotation file exists, `not_started` otherwise.
#[tauri::command]
pub async fn list_patients(
    app: tauri::AppHandle,
    root_dir: String,
) -> Result<Vec<PatientInfo>, String> {
    let root = PathBuf::from(&root_dir);
    if !root.is_dir() {
        return Err(format!("not a directory: {root_dir}"));
    }

    // Collect candidate patient directories on a blocking thread (SMB walks
    // can be slow).
    let root_for_walk = root.clone();
    let entries = tokio::task::spawn_blocking(move || -> Result<Vec<(String, PathBuf)>, String> {
        let mut out = Vec::new();
        let read = std::fs::read_dir(&root_for_walk).map_err(|e| format!("read_dir: {e}"))?;
        for entry in read.flatten() {
            if looks_like_patient_dir(&entry) {
                let name = entry.file_name().to_string_lossy().to_string();
                out.push((name, entry.path()));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    })
    .await
    .map_err(|e| format!("walk task failed: {e}"))??;

    // Cross-reference annotation saves on the main task (cheap local FS reads).
    let mut patients = Vec::with_capacity(entries.len());
    for (id, path) in entries {
        let (finalized_count, has_mmd) = read_annotation_summary(&app, &id);
        let status = if has_mmd && finalized_count > 0 {
            "complete"
        } else if finalized_count > 0 {
            "in_progress"
        } else {
            "not_started"
        };
        patients.push(PatientInfo {
            id,
            path: path.to_string_lossy().to_string(),
            status: status.to_string(),
            finalized_count,
            has_mmd,
        });
    }

    Ok(patients)
}

/// One immediate subdirectory of a patient folder — typically a single DICOM
/// series (e.g. `MonoPlus_70keV`). Used by the patient browser to expand a
/// patient into its series without reading any DICOM headers.
#[derive(serde::Serialize)]
pub struct SeriesDirInfo {
    /// Folder name (e.g. `MonoPlus_70keV`).
    pub name: String,
    /// Absolute path to the series folder.
    pub path: String,
    /// Number of regular files in the folder (≈ DICOM slices).
    pub num_files: usize,
}

/// List immediate subdirectories of a patient folder with file counts.
///
/// Returns folders sorted by name. Skips hidden / underscore-prefixed entries.
/// Fast — does not parse any DICOM headers.
#[tauri::command]
pub async fn list_series_dirs(patient_path: String) -> Result<Vec<SeriesDirInfo>, String> {
    let root = PathBuf::from(&patient_path);
    if !root.is_dir() {
        return Err(format!("not a directory: {patient_path}"));
    }

    tokio::task::spawn_blocking(move || -> Result<Vec<SeriesDirInfo>, String> {
        let mut out = Vec::new();
        let read = std::fs::read_dir(&root).map_err(|e| format!("read_dir: {e}"))?;
        for entry in read.flatten() {
            let Ok(file_type) = entry.file_type() else { continue };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.starts_with('_') {
                continue;
            }
            // Count regular files (cheap directory scan, no DICOM parse).
            let num_files = std::fs::read_dir(entry.path())
                .map(|d| {
                    d.flatten()
                        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                        .count()
                })
                .unwrap_or(0);
            out.push(SeriesDirInfo {
                name,
                path: entry.path().to_string_lossy().to_string(),
                num_files,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    })
    .await
    .map_err(|e| format!("list task failed: {e}"))?
}

/// Load a dual-energy volume from two series in a DICOM directory,
/// store it in AppState, and return summary metadata.
#[tauri::command]
pub async fn load_dual_energy(
    path: String,
    low_series_uid: String,
    high_series_uid: String,
    low_kev: f64,
    high_kev: f64,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<DualEnergyInfo, String> {
    let dir = path;
    let low_uid = low_series_uid;
    let high_uid = high_series_uid;

    let volume = tokio::task::spawn_blocking(move || {
        dicom_loader::load_dual_energy(
            Path::new(&dir),
            &low_uid,
            &high_uid,
            low_kev,
            high_kev,
        )
    })
    .await
    .map_err(|e| format!("load task failed: {e}"))?
    .map_err(|e| e.to_string())?;

    let info = DualEnergyInfo {
        shape: [
            volume.low.shape()[0],
            volume.low.shape()[1],
            volume.low.shape()[2],
        ],
        spacing: volume.spacing,
        low_kev: volume.low_energy_kev,
        high_kev: volume.high_energy_kev,
        patient_name: volume.patient_name.clone(),
    };

    let mut guard = state.lock().map_err(|e| format!("lock poisoned: {e}"))?;
    guard.dual_energy = Some(volume);

    Ok(info)
}

// ---------------------------------------------------------------------------
// Fast header-only series scan (v2)
// ---------------------------------------------------------------------------

/// Scan a DICOM folder for series (new shape — replaces legacy scan).
/// Returns header-only metadata per series; pixel data is not decoded.
#[tauri::command]
pub async fn scan_series_v2(path: String) -> Result<Vec<SeriesDescriptorDto>, String> {
    let dir = PathBuf::from(path);
    let series = dicom_scan::scan_series(&dir)
        .await
        .map_err(|e| e.to_string())?;
    Ok(series.into_iter().map(SeriesDescriptorDto::from).collect())
}

#[derive(serde::Serialize)]
pub struct SeriesDescriptorDto {
    pub uid: String,
    pub description: String,
    pub image_comments: Option<String>,
    pub rows: u32,
    pub cols: u32,
    pub num_slices: usize,
    pub pixel_spacing: [f64; 2],
    pub slice_spacing: f64,
    pub orientation: [f64; 6],
    pub rescale_slope: f64,
    pub rescale_intercept: f64,
    pub window_center: f64,
    pub window_width: f64,
    pub patient_name: String,
    pub study_description: String,
    /// Absolute file paths in z-sorted order.
    pub file_paths: Vec<String>,
    pub slice_positions_z: Vec<f64>,
}

impl From<SeriesDescriptor> for SeriesDescriptorDto {
    fn from(d: SeriesDescriptor) -> Self {
        Self {
            uid: d.uid,
            description: d.description,
            image_comments: d.image_comments,
            rows: d.rows,
            cols: d.cols,
            num_slices: d.num_slices,
            pixel_spacing: d.pixel_spacing,
            slice_spacing: d.slice_spacing,
            orientation: d.orientation,
            rescale_slope: d.rescale_slope,
            rescale_intercept: d.rescale_intercept,
            window_center: d.window_center,
            window_width: d.window_width,
            patient_name: d.patient_name,
            study_description: d.study_description,
            file_paths: d.file_paths.into_iter().map(|p| p.to_string_lossy().into_owned()).collect(),
            slice_positions_z: d.slice_positions_z,
        }
    }
}

// ---------------------------------------------------------------------------
// Bulk binary load (v2)
// ---------------------------------------------------------------------------

/// Load a single series as one framed binary response:
///   [u32 LE: metadata_json_length] [metadata_json] [i16 LE voxel bytes]
/// Frontend receives this as an ArrayBuffer.
#[tauri::command]
pub async fn load_series_v2(dir: String, uid: String) -> Result<Response, String> {
    let dir_path = PathBuf::from(dir);
    let vol = dicom_load::load_series(&dir_path, &uid)
        .await
        .map_err(|e| e.to_string())?;

    let voxel_bytes: Vec<u8> = bytemuck::cast_slice(&vol.voxels_i16).to_vec();
    let framed = encode_frame(&vol.metadata, &voxel_bytes)?;
    Ok(Response::new(framed))
}
