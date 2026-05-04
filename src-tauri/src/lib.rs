mod app_settings;
mod db;
mod error;
pub mod gps;
mod import;
mod issues;
mod metadata;
mod migration_v2;
mod model;
mod paths;
mod places;
pub mod scan;
mod scans;
mod storage;
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

/// Append-only panic log so users on bundled builds (where stderr is
/// invisible on Windows GUI subsystem and macOS .app launches) can
/// attach an actionable trace to a bug report.
fn install_panic_hook(log_dir: std::path::PathBuf) {
    use std::io::Write;
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        prev(info);

        if std::fs::create_dir_all(&log_dir).is_err() {
            return;
        }
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("panic.log"))
        else {
            return;
        };

        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>");
        let payload = info.payload();
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            *s
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.as_str()
        } else {
            "<non-string panic payload>"
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        let backtrace = std::backtrace::Backtrace::force_capture();

        let _ = writeln!(
            file,
            "----\n[{timestamp}] thread '{thread_name}' panicked at {location}:\n{msg}\n\nBacktrace:\n{backtrace}",
        );
    }));
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
            let app_data_dir = match app.path().app_data_dir() {
                Ok(d) => d,
                Err(e) => {
                    app.dialog()
                        .message(format!(
                            "Trip Viewer can't determine its data directory:\n\n{e}\n\n\
                             Please report this at \
                             https://github.com/chrisl8/trip-viewer/issues."
                        ))
                        .kind(MessageDialogKind::Error)
                        .title("Trip Viewer — Startup error")
                        .blocking_show();
                    return Err(Box::new(e));
                }
            };
            install_panic_hook(app_data_dir.join("logs"));

            // Sweep stale files left by removed features (e.g.
            // recovery-config.json) so they don't accumulate in
            // app_data_dir for users who upgrade in place.
            migration_v2::cleanup_orphan_files(&app_data_dir);

            // Per-machine settings live in app_data_dir/settings.json
            // (see src/app_settings.rs).
            let settings = app_settings::AppSettingsHandle::load(&app_data_dir);

            // If a legacy single-archive DB still sits at
            // app_data_dir/tripviewer.db and the user hasn't yet
            // picked an archive folder, derive an archive root from
            // segment paths and move the file into
            // <archive>/.tripviewer/tripviewer.db. Failure is
            // non-fatal — silently retried on every launch.
            //
            // Auto-derivation only handles archives produced by the
            // import pipeline (parent-of-Videos layout). Other shapes
            // are left in place; PR 3's "Open Archive" picker handles
            // them explicitly.
            match migration_v2::run_if_needed(&app_data_dir, &settings) {
                Ok(migration_v2::MigrationOutcome::Migrated { archive_root }) => {
                    eprintln!(
                        "[migration_v2] migrated DB → {}/.tripviewer/tripviewer.db",
                        archive_root.display()
                    );
                }
                Ok(migration_v2::MigrationOutcome::Skipped { reason }) => {
                    eprintln!("[migration_v2] skipped: {reason}");
                }
                Ok(migration_v2::MigrationOutcome::NotNeeded) => {}
                Err(e) => eprintln!("[migration_v2] {e}"),
            }

            // Determine the archive root for this session. With the
            // single-archive-at-a-time model, this comes from
            // last_archive in settings.json. If unset (fresh install
            // or skipped migration), fall back to app_data_dir so the
            // app still starts cleanly — the multi-archive switcher
            // in a follow-up change replaces this fallback with a
            // proper "no archive open" UI state.
            let archive_root = settings
                .read()
                .last_archive
                .as_ref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| app_data_dir.clone());

            let handle = match db::open(&archive_root) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!(
                        "[db] failed to open archive at {}: {e}",
                        archive_root.display()
                    );
                    app.dialog()
                        .message(format!(
                            "Trip Viewer can't open the archive database:\n\n{e}\n\n\
                             Archive folder:\n{}\n\n\
                             If this archive was written by a newer version of Trip Viewer, \
                             upgrade. If the drive isn't mounted, plug it in and relaunch.",
                            archive_root.display()
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

            // Per-archive cross-OS rewrite: rebuild segment / trip
            // UUIDs from archive-relative paths. Idempotent (gated by
            // settings.cross_os_migrated_archives) so it runs at most
            // once per archive root.
            match migration_v2::rebuild_for_cross_os(&handle, &settings) {
                Ok(migration_v2::RebuildOutcome::AlreadyDone) => {}
                Ok(migration_v2::RebuildOutcome::Migrated { segments_remapped }) => {
                    eprintln!(
                        "[migration_v2] cross-OS rewrite: {segments_remapped} segments remapped at {}",
                        archive_root.display()
                    );
                }
                Err(e) => eprintln!("[migration_v2] cross-OS rewrite failed: {e}"),
            }

            app.manage(settings);
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
            timelapse::commands::clear_timelapse_settings,
            timelapse::commands::test_ffmpeg,
            timelapse::commands::is_ffmpeg_quarantined,
            timelapse::commands::clear_ffmpeg_quarantine,
            timelapse::commands::start_timelapse,
            timelapse::commands::cancel_timelapse,
            timelapse::commands::list_timelapse_jobs,
            places::commands::list_places,
            places::commands::add_place,
            places::commands::update_place,
            places::commands::delete_place,
            storage::get_library_storage_summary,
            get_video_port,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
