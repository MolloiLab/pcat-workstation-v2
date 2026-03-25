use std::path::Path;
use std::sync::Mutex;

use tauri_plugin_dialog::DialogExt;

use crate::pipeline::dicom_loader;
use crate::state::AppState;

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
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<VolumeInfo, String> {
    let dir = path.clone();

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
