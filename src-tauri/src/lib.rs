mod commands;
mod error;
pub mod pipeline;
mod state;

use state::AppState;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(Mutex::new(AppState::new()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::dicom::open_dicom_dialog,
            commands::dicom::load_dicom,
            commands::dicom::get_recent_dicoms,
            commands::dicom::save_seeds,
            commands::dicom::load_seeds,
            commands::volume::get_slice,
            commands::cpr::build_cpr_frame,
            commands::cpr::render_cpr_image,
            commands::cpr::render_curved_cpr_image,
            commands::cpr::render_cross_sections,
            commands::cpr::compute_cpr_image,
            commands::cpr::compute_cross_section_image,
            commands::cpr::compute_cross_sections_batch,
            commands::cpr::get_cpr_projection_info,
            commands::pipeline::run_pipeline,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
