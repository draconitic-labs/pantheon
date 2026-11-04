//! Pantheon 3D: the web edition in a native Tauri v2 window (WebKitGTK on Linux,
//! WebView2 on Windows). The HTTP backend runs inside this process, so closing
//! the window is the whole lifecycle. No browser is ever involved.
//!
//! Environment:
//!   PANTHEON_START_URL  page to open: full URL, or a path/query for the local server
//!   PANTHEON_PORT       preferred backend port (default 8090)
//!   PANTHEON_QA=1       enable the test hooks in the backend (see serve.rs)
//!   WEBKIT_DISABLE_DMABUF_RENDERER  set to 0 to keep the DMA-BUF renderer on Wayland
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use tauri::{WebviewUrl, WebviewWindowBuilder};

/// WebKitGTK's DMA-BUF renderer dies with Wayland "Error 71" on this NVIDIA
/// driver. Runs before any thread exists; an explicit value from the user wins.
fn prepare_webkit_env() {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland");
    if wayland && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}

fn start_url(port: u16) -> String {
    let base = format!("http://127.0.0.1:{port}");
    match std::env::var("PANTHEON_START_URL") {
        Ok(u)
            if u.starts_with("http://")
                || u.starts_with("https://")
                || u.starts_with("tauri://") =>
        {
            u
        }
        Ok(u) if u.starts_with('/') => format!("{base}{u}"),
        Ok(u) if !u.is_empty() => format!("{base}/{u}"),
        _ => format!("{base}/"),
    }
}

fn main() {
    prepare_webkit_env();
    let port_req = std::env::var("PANTHEON_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8090);
    let backend = pantheon::serve::spawn_background(port_req);
    tauri::Builder::default()
        .setup(move |app| {
            let url = match &backend {
                Ok(port) => WebviewUrl::External(start_url(*port).parse()?),
                Err(e) => {
                    eprintln!("Pantheon 3D backend failed: {e:#}");
                    WebviewUrl::App("index.html".into())
                }
            };
            WebviewWindowBuilder::new(app, "main", url)
                .title("Pantheon 3D")
                .inner_size(1600.0, 950.0)
                .min_inner_size(640.0, 420.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Pantheon 3D window failed");
}
