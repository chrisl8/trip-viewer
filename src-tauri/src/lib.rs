mod error;
pub mod gps;
mod import;
mod metadata;
mod model;
pub mod scan;
#[cfg(target_os = "linux")]
mod video_server;

/// Tauri state wrapping the loopback video server port.
/// On non-Linux platforms this is always 0 and the frontend falls back to
/// Tauri's built-in asset protocol.
struct VideoPort(u16);

#[tauri::command]
fn get_video_port(port: tauri::State<VideoPort>) -> u16 {
    port.0
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    let video_port = video_server::start()
        .inspect(|&p| {
            eprintln!("[video-server] listening on 127.0.0.1:{p}");
        })
        .unwrap_or_else(|e| {
            eprintln!("[video-server] failed to start: {e}");
            0
        });
    #[cfg(not(target_os = "linux"))]
    let video_port: u16 = 0;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(import::ImportState::new())
        .manage(VideoPort(video_port))
        .invoke_handler(tauri::generate_handler![
            scan::scan_folder,
            metadata::probe_file,
            gps::extract_gps,
            gps::extract_gps_batch,
            import::discover_sources,
            import::start_import,
            import::cancel_import,
            import::resolve_unknowns,
            get_video_port,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
