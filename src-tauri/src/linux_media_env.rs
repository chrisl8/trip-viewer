//! Linux media environment fixups that must happen before WebKitGTK and
//! GStreamer initialise (i.e. first thing in `main`, while single-threaded).
//!
//! 1. System libva inside the AppImage. The bundle ships the build host's
//!    libva (Ubuntu 24.04: VA-API 1.20) but VA drivers always come from the
//!    host. A newer host driver (e.g. Intel iHD built for VA-API 1.22, as on
//!    Debian 13) only exports `__vaDriverInit_1_22`, which the old libva
//!    cannot find, so `vah264dec` silently drops out and 1440p60 footage
//!    falls back to software decoding and stutters. libva is
//!    backwards-compatible (soname 2), so we preload the host copy — the
//!    one that matches the host driver — and re-exec once, since the
//!    dynamic loader only honours LD_PRELOAD at process start.
//!    Opt out with `TRIPVIEWER_SYSTEM_LIBVA=0`.
//!
//! 2. WebKit's DMABuf video sink. With VA decoders it hands decoder surfaces
//!    to the compositor zero-copy; surfaces get recycled while still on
//!    screen, which shows up as flicker / stale frames reappearing. The GL
//!    sink copies instead. Respected if the user already set
//!    `WEBKIT_GST_DMABUF_SINK_DISABLED`.

use std::env;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

const REEXEC_MARKER: &str = "TRIPVIEWER_SYSTEM_LIBVA_REEXEC";
const LIBVA_DIRS: &[&str] = &["/usr/lib/x86_64-linux-gnu", "/usr/lib64", "/usr/lib"];
const LIBVA_LIBS: &[&str] = &["libva.so.2", "libva-drm.so.2", "libva-x11.so.2"];

pub fn prepare() {
    if env::var_os("WEBKIT_GST_DMABUF_SINK_DISABLED").is_none() {
        env::set_var("WEBKIT_GST_DMABUF_SINK_DISABLED", "1");
    }
    preload_system_libva();
}

fn preload_system_libva() {
    if env::var_os("APPIMAGE").is_none()
        || env::var_os(REEXEC_MARKER).is_some()
        || env::var_os("TRIPVIEWER_SYSTEM_LIBVA").is_some_and(|v| v == "0")
    {
        return;
    }
    let dirs: Vec<&Path> = LIBVA_DIRS.iter().map(Path::new).collect();
    let Some(preload) = system_libva_preload(&dirs, env::var_os("LD_PRELOAD")) else {
        return;
    };

    let mut args = env::args_os();
    let arg0 = args.next().unwrap_or_default();
    let err = Command::new("/proc/self/exe")
        .arg0(arg0)
        .args(args)
        .env("LD_PRELOAD", &preload)
        .env(REEXEC_MARKER, "1")
        .exec();
    // Only reached if exec failed; carry on with the bundled libva.
    eprintln!("[media-env] re-exec with system libva failed: {err}");
}

/// LD_PRELOAD value for the host libva found in the first of `dirs` that has
/// `libva.so.2`, with any existing preload list appended. `None` if no
/// candidate directory has libva.
fn system_libva_preload(dirs: &[&Path], existing: Option<OsString>) -> Option<OsString> {
    let dir = dirs.iter().find(|d| d.join(LIBVA_LIBS[0]).is_file())?;
    let mut preload = OsString::new();
    for lib in LIBVA_LIBS {
        let path = dir.join(lib);
        if path.is_file() {
            if !preload.is_empty() {
                preload.push(":");
            }
            preload.push(path);
        }
    }
    if let Some(existing) = existing.filter(|v| !v.is_empty()) {
        preload.push(":");
        preload.push(existing);
    }
    Some(preload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn no_libva_means_no_preload() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(system_libva_preload(&[dir.path()], None), None);
    }

    #[test]
    fn picks_first_dir_with_libva_and_only_existing_libs() {
        let empty = tempfile::tempdir().unwrap();
        let host = tempfile::tempdir().unwrap();
        fs::write(host.path().join("libva.so.2"), b"").unwrap();
        fs::write(host.path().join("libva-drm.so.2"), b"").unwrap();
        let got = system_libva_preload(&[empty.path(), host.path()], None).unwrap();
        let want = format!(
            "{}:{}",
            host.path().join("libva.so.2").display(),
            host.path().join("libva-drm.so.2").display()
        );
        assert_eq!(got, OsString::from(want));
    }

    #[test]
    fn keeps_existing_preload_after_libva() {
        let host = tempfile::tempdir().unwrap();
        fs::write(host.path().join("libva.so.2"), b"").unwrap();
        let got = system_libva_preload(&[host.path()], Some("/x/libfoo.so".into())).unwrap();
        let want = format!("{}:/x/libfoo.so", host.path().join("libva.so.2").display());
        assert_eq!(got, OsString::from(want));
    }
}
