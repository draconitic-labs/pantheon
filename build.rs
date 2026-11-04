// Windows only, for the pantheon-3d window in src-tauri, which builds this crate as a
// dependency in its own target folder. Through tauri and wry it can import
// WebView2Loader.dll, which webview2-com-sys leaves in its build OUT_DIR rather than beside
// the exe, and a missing import DLL aborts before main(). So copy it there if it's been
// built. Atlas-only builds find nothing and do nothing. Not tested since the window moved
// to src-tauri: see the Windows section of KNOWN_ISSUES.md.
fn main() {
    #[cfg(windows)]
    {
        let Ok(out_dir) = std::env::var("OUT_DIR") else {
            return;
        };
        // OUT_DIR = target/<profile>/build/pantheon-<hash>/out
        let Some(profile_dir) = std::path::Path::new(&out_dir)
            .ancestors()
            .nth(3)
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        let build_dir = profile_dir.join("build");
        let dest = profile_dir.join("WebView2Loader.dll");
        if dest.exists() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&build_dir) else {
            return;
        };
        for e in entries.flatten() {
            if e.file_name()
                .to_string_lossy()
                .starts_with("webview2-com-sys-")
            {
                let src = e.path().join("out").join("x64").join("WebView2Loader.dll");
                if src.exists() && std::fs::copy(&src, &dest).is_ok() {
                    return;
                }
            }
        }
    }
}
