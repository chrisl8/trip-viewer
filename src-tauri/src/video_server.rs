//! Minimal localhost HTTP server for serving local video files on Linux and macOS.
//!
//! Background: Tauri's `asset://` scheme on Linux is routed through the
//! WebView for fetch/XHR, but WebKitGTK's GStreamer-based `<video>` element
//! has no URI handler for the `asset` scheme and fails with FormatError.
//! `file://` URLs are blocked by the cross-origin policy between the
//! localhost webview and the filesystem. The workaround is to serve local
//! files from 127.0.0.1 over plain HTTP so the `<video>` element can load
//! them normally, with full support for Range requests (seeking).
//!
//! macOS needs the same server for a different reason: Tauri v2's `asset://`
//! handler on WKWebView does NOT honor HTTP Range requests. AVFoundation
//! needs to fetch the `moov` atom to build sample tables before it can
//! decode, and Wolfbox firmware writes MP4s with `moov` at the END of the
//! file. Without range support, WKWebView feeds AVFoundation a forward-only
//! byte stream, stalling playback of the primary channel for ~14 s while it
//! linearly buffers through mdat to reach moov. Serving over 127.0.0.1 with
//! full 206 Partial Content support lets AVFoundation seek to EOF for moov
//! and start decoding immediately.
//!
//! Windows uses Tauri's built-in asset protocol (Chromium-based WebView2
//! handles range reads correctly) and doesn't need this.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;

/// Start the video server on a random free loopback port. Returns the port.
pub fn start() -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();

    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread::spawn(move || {
                let _ = handle_client(stream);
            });
        }
    });

    Ok(port)
}

/// Convert a raw HTTP request-path into an absolute filesystem PathBuf.
///
/// The URL path *is* the absolute filesystem path. This tolerates extra
/// leading slashes — `//home/x.mp4` parses the same as `/home/x.mp4` — so
/// the server is robust to clients that accidentally produce double slashes
/// between authority and path.
pub(crate) fn request_path(raw: &str) -> Option<PathBuf> {
    // Drop query string / fragment — we don't use them.
    let raw = raw.split(['?', '#']).next().unwrap_or(raw);
    // Collapse any run of leading slashes into exactly one.
    let trimmed = raw.trim_start_matches('/');
    let normalized = format!("/{trimmed}");
    let decoded = percent_decode(&normalized)?;
    let path = PathBuf::from(decoded);
    if path.is_absolute() {
        Some(path)
    } else {
        None
    }
}

fn log_request(method: &str, raw_path: &str, status: u16, range: Option<&str>) {
    match range {
        Some(r) => eprintln!("[video-server] {method} {raw_path} -> {status} {r}"),
        None => eprintln!("[video-server] {method} {raw_path} -> {status}"),
    }
}

fn handle_client(mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let raw_path = parts.next().unwrap_or("/").to_string();

    if method != "GET" && method != "HEAD" {
        log_request(&method, &raw_path, 405, None);
        return write_status(&mut stream, 405, "Method Not Allowed");
    }

    // Parse headers and extract Range if present.
    let mut range_header: Option<String> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(value) = header_value(&line, "range") {
            range_header = Some(value);
        }
    }

    let Some(path) = request_path(&raw_path) else {
        log_request(&method, &raw_path, 400, None);
        return write_status(&mut stream, 400, "Bad Request");
    };

    let mut file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => {
            log_request(&method, &raw_path, 404, None);
            return write_status(&mut stream, 404, "Not Found");
        }
    };

    let total_len = file.metadata()?.len();
    let mime = mime_for(&path);

    let (start, end, is_partial) = if let Some(h) = range_header {
        match parse_range(&h, total_len) {
            Some(r) => r,
            None => {
                log_request(&method, &raw_path, 416, Some(&format!("bad {h}")));
                write!(
                    stream,
                    "HTTP/1.1 416 Range Not Satisfiable\r\n\
                     Content-Range: bytes */{total_len}\r\n\
                     Content-Length: 0\r\n\
                     Access-Control-Allow-Origin: *\r\n\r\n"
                )?;
                return Ok(());
            }
        }
    } else {
        (0u64, total_len.saturating_sub(1), false)
    };

    let content_length = if total_len == 0 { 0 } else { end - start + 1 };

    if is_partial {
        log_request(
            &method,
            &raw_path,
            206,
            Some(&format!("bytes={start}-{end}/{total_len}")),
        );
        write!(
            stream,
            "HTTP/1.1 206 Partial Content\r\n\
             Content-Type: {mime}\r\n\
             Content-Length: {content_length}\r\n\
             Content-Range: bytes {start}-{end}/{total_len}\r\n\
             Accept-Ranges: bytes\r\n\
             Access-Control-Allow-Origin: *\r\n\r\n"
        )?;
    } else {
        log_request(
            &method,
            &raw_path,
            200,
            Some(&format!("{content_length}B")),
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\n\
             Content-Type: {mime}\r\n\
             Content-Length: {content_length}\r\n\
             Accept-Ranges: bytes\r\n\
             Access-Control-Allow-Origin: *\r\n\r\n"
        )?;
    }

    if method == "HEAD" || content_length == 0 {
        return Ok(());
    }

    file.seek(SeekFrom::Start(start))?;
    let mut remaining = content_length;
    let mut buf = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let to_read = std::cmp::min(buf.len() as u64, remaining) as usize;
        let n = file.read(&mut buf[..to_read])?;
        if n == 0 {
            break;
        }
        if stream.write_all(&buf[..n]).is_err() {
            // Client disconnected mid-transfer; normal for seeks.
            break;
        }
        remaining -= n as u64;
    }
    Ok(())
}

fn header_value(line: &str, name: &str) -> Option<String> {
    let (h, v) = line.split_once(':')?;
    if h.trim().eq_ignore_ascii_case(name) {
        Some(v.trim().trim_end_matches(['\r', '\n']).to_string())
    } else {
        None
    }
}

fn parse_range(header: &str, total_len: u64) -> Option<(u64, u64, bool)> {
    let rest = header.strip_prefix("bytes=")?;
    let (s, e) = rest.split_once('-')?;
    let start: u64 = s.trim().parse().ok()?;
    let end: u64 = if e.trim().is_empty() {
        total_len.saturating_sub(1)
    } else {
        e.trim().parse().ok()?
    };
    if start > end || end >= total_len {
        return None;
    }
    Some((start, end, true))
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        _ => "application/octet-stream",
    }
}

fn write_status(stream: &mut TcpStream, code: u16, reason: &str) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {code} {reason}\r\n\
         Content-Length: 0\r\n\
         Access-Control-Allow-Origin: *\r\n\r\n"
    )
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_nibble(bytes[i + 1])?;
                let lo = hex_nibble(bytes[i + 2])?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_percent_decode_basic() {
        assert_eq!(percent_decode("/home/user").as_deref(), Some("/home/user"));
        assert_eq!(
            percent_decode("/a%20b").as_deref(),
            Some("/a b")
        );
        assert_eq!(
            percent_decode("%2Fhome%2Fchris10%2Fvideo.MP4").as_deref(),
            Some("/home/chris10/video.MP4")
        );
    }

    #[test]
    fn test_percent_decode_invalid() {
        assert_eq!(percent_decode("%ZZ"), None);
    }

    #[test]
    fn test_parse_range() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99, true)));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999, true)));
        assert_eq!(parse_range("bytes=0-0", 1000), Some((0, 0, true)));
        // end >= total_len is invalid
        assert_eq!(parse_range("bytes=0-1000", 1000), None);
        // start > end is invalid
        assert_eq!(parse_range("bytes=500-100", 1000), None);
        // no bytes= prefix
        assert_eq!(parse_range("items=0-99", 1000), None);
    }

    #[test]
    fn test_mime_for() {
        assert_eq!(mime_for(Path::new("/a/b.mp4")), "video/mp4");
        assert_eq!(mime_for(Path::new("/a/b.MP4")), "video/mp4");
        assert_eq!(mime_for(Path::new("/a/b.mov")), "video/quicktime");
        assert_eq!(mime_for(Path::new("/a/b.xyz")), "application/octet-stream");
    }

    #[test]
    fn test_header_value() {
        assert_eq!(
            header_value("Range: bytes=0-99\r\n", "range").as_deref(),
            Some("bytes=0-99")
        );
        assert_eq!(
            header_value("RANGE: bytes=0-99\r\n", "range").as_deref(),
            Some("bytes=0-99")
        );
        assert_eq!(header_value("Accept: */*\r\n", "range"), None);
    }

    #[test]
    fn test_start_binds_port() {
        let port = start().unwrap();
        assert!(port > 0);
    }

    #[test]
    fn test_request_path_absolute() {
        assert_eq!(
            request_path("/home/x.mp4"),
            Some(PathBuf::from("/home/x.mp4"))
        );
    }

    #[test]
    fn test_request_path_double_slash() {
        // Clients that build URLs by concatenating `http://host:port` + a
        // filesystem path that already starts with `/` can end up sending
        // `GET //home/x.mp4`. Treat this the same as `/home/x.mp4`.
        assert_eq!(
            request_path("//home/x.mp4"),
            Some(PathBuf::from("/home/x.mp4"))
        );
        assert_eq!(
            request_path("///home/x.mp4"),
            Some(PathBuf::from("/home/x.mp4"))
        );
    }

    #[test]
    fn test_request_path_percent_encoded() {
        assert_eq!(
            request_path("/home/a%20b/c.mp4"),
            Some(PathBuf::from("/home/a b/c.mp4"))
        );
        assert_eq!(
            request_path("//home/Wolfbox%20Dashcam/Videos/x.MP4"),
            Some(PathBuf::from("/home/Wolfbox Dashcam/Videos/x.MP4"))
        );
    }

    #[test]
    fn test_request_path_strips_query_and_fragment() {
        assert_eq!(
            request_path("/home/x.mp4?range=0-99"),
            Some(PathBuf::from("/home/x.mp4"))
        );
        assert_eq!(
            request_path("/home/x.mp4#frag"),
            Some(PathBuf::from("/home/x.mp4"))
        );
    }

    #[test]
    fn test_request_path_rejects_invalid_encoding() {
        assert_eq!(request_path("/home/%ZZ.mp4"), None);
    }

    // End-to-end round-trip: start the server, write a tempfile, issue a
    // real HTTP request through a TCP client, and verify the response.
    // This is the test that would have caught the double-slash 400 bug in
    // CI before it ever reached a user.
    #[test]
    fn test_roundtrip_get() {
        use std::io::Write as _;
        use std::net::TcpStream;
        use tempfile::NamedTempFile;

        let mut tmp = NamedTempFile::new().unwrap();
        let body = b"hello world! this is a fake video payload.";
        tmp.write_all(body).unwrap();
        // Persist so the path is stable regardless of handle state.
        let tmp_path = tmp.path().to_path_buf();

        let port = start().unwrap();

        // Single slash — canonical form.
        let resp = http_get(port, &tmp_path.to_string_lossy());
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp:.200}");
        assert!(resp.contains("Content-Length: "), "got: {resp:.200}");
        assert!(resp.ends_with(std::str::from_utf8(body).unwrap()));

        // Double slash — the bug we just fixed. Path `//tmp/...` must be
        // normalized to `/tmp/...` and still hit the file.
        let double_slash = format!("/{}", tmp_path.to_string_lossy());
        let resp2 = http_get(port, &double_slash);
        assert!(
            resp2.starts_with("HTTP/1.1 200"),
            "double-slash expected 200, got: {resp2:.200}"
        );
        assert!(resp2.ends_with(std::str::from_utf8(body).unwrap()));

        // Range request returns 206 with exactly the requested bytes.
        let resp3 = http_get_range(port, &tmp_path.to_string_lossy(), 0, 4);
        assert!(
            resp3.starts_with("HTTP/1.1 206"),
            "expected 206, got: {resp3:.200}"
        );
        assert!(resp3.contains("Content-Range: bytes 0-4/"));
        assert!(resp3.ends_with("hello"));

        // Sanity: the server is still alive after three requests.
        let resp4 = http_get(port, "/definitely/not/a/real/path");
        assert!(resp4.starts_with("HTTP/1.1 404"), "got: {resp4:.200}");

        fn http_get(port: u16, raw_path: &str) -> String {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(
                s,
                "GET {raw_path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut out = String::new();
            std::io::Read::read_to_string(&mut s, &mut out).unwrap();
            out
        }

        fn http_get_range(port: u16, raw_path: &str, start: u64, end: u64) -> String {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(
                s,
                "GET {raw_path} HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes={start}-{end}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut out = String::new();
            std::io::Read::read_to_string(&mut s, &mut out).unwrap();
            out
        }
    }
}
