// The pantheon-3d window binary imports WebView2Loader.dll (via wry →
// webview2-com-sys, which ships the DLL in its build OUT_DIR but does not
// place it beside our exe). A missing import DLL aborts process init before
// main() — so copy it next to the artifacts on every build.
fn main() {
    #[cfg(windows)]
    {
        let Ok(out_dir) = std::env::var("OUT_DIR") else { return };
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
        let Ok(entries) = std::fs::read_dir(&build_dir) else { return };
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("webview2-com-sys-") {
                let src = e.path().join("out").join("x64").join("WebView2Loader.dll");
                if src.exists() && std::fs::copy(&src, &dest).is_ok() {
                    return;
                }
            }
        }
    }
}
