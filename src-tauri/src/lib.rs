mod error;
pub mod gps;
mod import;
mod metadata;
mod model;
pub mod scan;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(import::ImportState::new())
        .invoke_handler(tauri::generate_handler![
            scan::scan_folder,
            metadata::probe_file,
            gps::extract_gps,
            gps::extract_gps_batch,
            import::discover_sources,
            import::start_import,
            import::cancel_import,
            import::resolve_unknowns,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
