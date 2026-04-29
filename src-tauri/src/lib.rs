mod db;
mod error;
pub mod gps;
mod import;
mod issues;
mod metadata;
mod model;
mod places;
pub mod scan;
mod scans;
mod tags;
mod timelapse;
mod trips;
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

    // Persist window size/position across runs. Skip VISIBLE so a window
    // closed while hidden (e.g., after a crash) doesn't come back invisible.
    let window_state_flags =
        tauri_plugin_window_state::StateFlags::all() - tauri_plugin_window_state::StateFlags::VISIBLE;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(
            // `skip_initial_state` prevents the plugin's auto-restore in
            // on_window_ready; we restore explicitly in `setup` below so the
            // order (restore → fit → show) is deterministic.
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(window_state_flags)
                .skip_initial_state("main")
                .build(),
        )
        .manage(import::ImportState::new())
        .manage(VideoPort(video_port))
        .manage(scans::worker::new_shared_state())
        .manage(timelapse::worker::new_shared_state())
        .setup(move |app| {
            use tauri::Manager;
            use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
            use tauri_plugin_window_state::WindowExt;
            let app_data_dir = app.path().app_data_dir()
                .expect("resolve app_data_dir");
            let db_path = app_data_dir.join("tripviewer.db");
            let handle = match db::open(&db_path) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("[db] failed to open {}: {e}", db_path.display());
                    // Without a managed DB, every command that takes
                    // tauri::State<DbHandle> would error with the opaque
                    // "state not managed" message. Show the user the real
                    // error and bail cleanly instead.
                    app.dialog()
                        .message(format!(
                            "Trip Viewer can't open its database:\n\n{e}\n\n\
                             If this database was created by a newer version of Trip Viewer, \
                             upgrade or move the file aside:\n\n{}",
                            db_path.display()
                        ))
                        .kind(MessageDialogKind::Error)
                        .title("Trip Viewer — Database error")
                        .blocking_show();
                    return Err(Box::new(e));
                }
            };
            if let Err(e) = timelapse::cleanup::cleanup_stale_jobs(&handle) {
                eprintln!("[timelapse] cleanup failed at startup: {e}");
            }
            app.manage(handle);
            if let Some(window) = app.get_webview_window("main") {
                // 1. Restore saved position/size/maximized first so the fit
                //    clamp runs against the real geometry the user expects.
                if let Err(e) = window.restore_state(window_state_flags) {
                    eprintln!("[window-state] failed to restore: {e}");
                }
                // 2. Clamp to the current monitor's work area if the restored
                //    (or default) size is too large. A no-op when maximized.
                if let Err(e) = window_fit::fit_to_work_area(&window) {
                    eprintln!("[window-fit] failed to clamp window: {e}");
                }
                // 3. Show last, so the user never sees an intermediate state.
                if let Err(e) = window.show() {
                    eprintln!("[window] failed to show: {e}");
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
            import::start_folder_import,
            import::cancel_import,
            import::resolve_unknowns,
            issues::issues_delete_to_trash,
            tags::commands::get_tags_for_trip,
            tags::commands::get_tag_counts_by_trip,
            tags::commands::get_all_tags,
            tags::commands::list_user_applicable_tags,
            tags::commands::add_user_tag,
            tags::commands::remove_user_tag,
            tags::commands::delete_segments_to_trash,
            trips::commands::list_archive_only_trips,
            trips::commands::delete_trip,
            trips::commands::assess_trip_merge,
            trips::commands::merge_trips,
            scans::commands::list_scans,
            scans::commands::list_scan_coverage,
            scans::commands::start_scan,
            scans::commands::cancel_scan,
            timelapse::commands::get_timelapse_settings,
            timelapse::commands::test_ffmpeg,
            timelapse::commands::start_timelapse,
            timelapse::commands::cancel_timelapse,
            timelapse::commands::list_timelapse_jobs,
            places::commands::list_places,
            places::commands::add_place,
            places::commands::update_place,
            places::commands::delete_place,
            get_video_port,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
