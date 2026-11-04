//! Pantheon 3D: the web edition in a native Tauri v2 window (WebKitGTK on Linux,
//! WebView2 on Windows). The HTTP backend runs inside this process, so closing
//! the window is the whole lifecycle. No browser is ever involved.
//!
//! Environment:
//!   PANTHEON_START_URL  page to open: full URL, or a path/query for the local server
//!   PANTHEON_PORT       preferred backend port (default 8090)
//!   PANTHEON_QA=1       enable the test hooks in the backend (see serve.rs)
//!   PANTHEON_WEBVIEW_DATA  where WebKit keeps its data (default: webview-data beside the binary)
//!   WEBKIT_DMABUF_RENDERER_FORCE_SHM  set by this app on NVIDIA unless you set it, or
//!                       WEBKIT_DISABLE_DMABUF_RENDERER, yourself
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use tauri::{WebviewUrl, WebviewWindowBuilder};

/// On the NVIDIA driver, WebKitGTK can't hand frames over as GPU buffers: on Wayland it
/// dies with "Error 71", and under X11 it logs "Failed to create GBM buffer" and leaves
/// the window blank. Its shared-memory handover works on both. It also ran at 55-62 fps
/// on Wayland, where WEBKIT_DISABLE_DMABUF_RENDERER=1 gave 40-48. Other drivers keep
/// WebKit's defaults. Runs before any thread exists; a value set by the user wins.
fn prepare_webkit_env() {
    let nvidia = std::path::Path::new("/proc/driver/nvidia/version").exists();
    let user_chose = std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_some()
        || std::env::var_os("WEBKIT_DMABUF_RENDERER_FORCE_SHM").is_some();
    if nvidia && !user_chose {
        std::env::set_var("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1");
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

/// WebKit's data (the first-run flag, its cache) stays in the checkout with the
/// binary and the datasets, so uninstalling leaves nothing in the home folder.
fn webview_data_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("PANTHEON_WEBVIEW_DATA")
        .map(Into::into)
        .or_else(|| Some(std::env::current_exe().ok()?.parent()?.join("webview-data")))
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
            // the window's size and limits live in tauri.conf.json; only its address is
            // known here, because that depends on the port the backend got
            let mut config = app.config().app.windows[0].clone();
            config.url = url;
            let mut window = WebviewWindowBuilder::from_config(app, &config)?;
            if let Some(dir) = webview_data_dir() {
                window = window.data_directory(dir);
            }
            window.build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Pantheon 3D window failed");
}
