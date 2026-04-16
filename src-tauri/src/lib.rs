mod error;
pub mod gps;
mod import;
mod metadata;
mod model;
pub mod scan;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod video_server;
mod window_fit;

/// Tauri state wrapping the loopback video server port.
/// On Windows this is always 0 and the frontend falls back to Tauri's
/// built-in asset protocol. Linux and macOS run the loopback HTTP server
/// because their WebView video pipelines can't use `asset://` directly —
/// Linux WebKitGTK has no URI handler for it, and macOS WKWebView's asset
/// handler doesn't honor HTTP Range requests (breaks moov-at-end MP4s).
struct VideoPort(u16);

#[tauri::command]
fn get_video_port(port: tauri::State<VideoPort>) -> u16 {
    port.0
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let video_port = video_server::start()
        .inspect(|&p| {
            eprintln!("[video-server] listening on 127.0.0.1:{p}");
        })
        .unwrap_or_else(|e| {
            eprintln!("[video-server] failed to start: {e}");
            0
        });
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let video_port: u16 = 0;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(import::ImportState::new())
        .manage(VideoPort(video_port))
        .setup(|app| {
            use tauri::Manager;
            if let Some(window) = app.get_webview_window("main") {
                if let Err(e) = window_fit::fit_to_work_area(&window) {
                    eprintln!("[window-fit] failed to clamp window to work area: {e}");
                }
            }
            Ok(())
        })
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
