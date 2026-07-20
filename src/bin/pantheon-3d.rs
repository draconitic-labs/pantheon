//! Pantheon 3D — the web edition in its own native window (tao + wry, the
//! windowing/webview engine underneath Tauri; WebView2 on Windows). Runs the
//! HTTP server on a background thread inside this process, so closing the
//! window is the whole lifecycle — no console, no browser.
#![cfg_attr(windows, windows_subsystem = "windows")]

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;

fn main() {
    log_step("main start");
    std::panic::set_hook(Box::new(|info| {
        log_step(&format!("PANIC: {info}"));
        msgbox(&format!("Pantheon 3D crashed:\n\n{info}"));
    }));
    if let Err(e) = run() {
        log_step(&format!("ERROR: {e:#}"));
        msgbox(&format!("Pantheon 3D failed to start:\n\n{e:#}"));
        std::process::exit(1);
    }
}

/// Append a boot-trace line to %LOCALAPPDATA%\Pantheon3D\boot.log — a
/// windows-subsystem process has no console, so this is the only way to see
/// how far startup got on a user machine.
fn log_step(msg: &str) {
    let dir = std::env::var("LOCALAPPDATA")
        .map(|l| std::path::PathBuf::from(l).join("Pantheon3D"))
        .unwrap_or_else(|_| std::env::temp_dir().join("Pantheon3D"));
    let _ = std::fs::create_dir_all(&dir);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("boot.log")) {
        use std::io::Write;
        let _ = writeln!(f, "[{t}] {msg}");
    }
}

fn run() -> anyhow::Result<()> {
    // WebView2 reads extra Chromium switches from the environment — pin the
    // hardware D3D11 path exactly like the browser launcher did
    std::env::set_var(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "--use-angle=d3d11 --ignore-gpu-blocklist",
    );
    let port = pantheon::serve::spawn_background(8090).map_err(|e| anyhow::anyhow!("server: {e:#}"))?;
    let url = format!("http://127.0.0.1:{port}/");
    log_step(&format!("server ready on port {port}"));

    let event_loop = EventLoop::new();
    log_step("event loop created");
    let mut builder = WindowBuilder::new()
        .with_title("Pantheon 3D — An Infinite Vector Atlas of the Living Universe")
        .with_inner_size(LogicalSize::new(1600.0, 950.0));
    if let Some(icon) = load_icon() {
        builder = builder.with_window_icon(Some(icon));
    }
    let window = builder.build(&event_loop)?;
    log_step("window built");

    // keep the WebView2 profile out of the repo tree
    let data_dir = std::env::var("LOCALAPPDATA")
        .map(|l| std::path::PathBuf::from(l).join("Pantheon3D").join("webview2"))
        .unwrap_or_else(|_| std::env::temp_dir().join("pantheon3d-webview2"));
    std::fs::create_dir_all(&data_dir).ok();
    let mut ctx = wry::WebContext::new(Some(data_dir));
    let _webview = wry::WebViewBuilder::new_with_web_context(&mut ctx)
        .with_url(&url)
        .build(&window)
        .map_err(|e| anyhow::anyhow!("webview (is the WebView2 runtime installed?): {e}"))?;
    log_step("webview built — entering event loop");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = event {
            log_step("close requested");
            *control_flow = ControlFlow::Exit;
        }
    });
}

fn load_icon() -> Option<tao::window::Icon> {
    let path = pantheon::data::asset_root().join("icon.png");
    let img = image::open(path).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    tao::window::Icon::from_rgba(img.into_raw(), w, h).ok()
}

#[cfg(windows)]
fn msgbox(text: &str) {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    let t: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let c: Vec<u16> = "Pantheon 3D".encode_utf16().chain([0]).collect();
    unsafe {
        MessageBoxW(0, t.as_ptr(), c.as_ptr(), 0x10); // MB_ICONERROR
    }
}

#[cfg(not(windows))]
fn msgbox(text: &str) {
    eprintln!("{text}");
}
