mod commands;
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
            commands::dicom::scan_series,
            commands::dicom::load_dual_energy,
            commands::dicom::list_patients,
            commands::dicom::list_series_dirs,
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
            commands::annotation::generate_annotation_targets,
            commands::annotation::init_snake,
            commands::annotation::evolve_snake,
            commands::annotation::update_snake_points,
            commands::annotation::add_snake_point,
            commands::annotation::finalize_contour,
            commands::annotation::run_mmd_on_roi,
            commands::annotation::sample_surfaces,
            commands::annotation::get_mmd_overlay,
            commands::annotation::save_annotations,
            commands::annotation::load_annotations,
            commands::annotation::export_mmd_csv,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
