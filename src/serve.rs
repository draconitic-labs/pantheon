//! Local backend for Pantheon 3D. It runs inside the `pantheon-3d` Tauri window
//! (src-tauri), or alone with `pantheon-cli --serve`; it never opens a browser.
//!
//! The Rust core stays the scientific backend: it serves the static
//! `pantheon-web/` app, the cached datasets under `/data/`, the bundled fonts
//! under `/fonts/`, and a small API: `/api/states` (SPICE-grade heliocentric
//! states + finite-difference velocities), `/api/tracks` (parsed Horizons
//! spacecraft arcs), `/api/meta` (what data is present, so the app can say what
//! is missing). Binds 127.0.0.1 only; `Connection: close` keeps it simple.
//!
//! With `PANTHEON_QA=1` it also exposes test hooks: `POST /api/qa/eval` runs
//! JavaScript in the open window (which polls `/api/qa/poll` when loaded with
//! `?qa`) and returns the result; `POST /api/qa/log` collects page errors.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

/// Bind the first free port from `port_req`; if a port is taken by another
/// Pantheon, report that instead so callers can reuse it.
enum Bound {
    Listener(TcpListener, u16),
    Existing(u16),
}

fn bind_or_find(port_req: u16) -> anyhow::Result<Bound> {
    for cand in port_req..port_req.saturating_add(10) {
        match TcpListener::bind(("127.0.0.1", cand)) {
            Ok(l) => return Ok(Bound::Listener(l, cand)),
            Err(_) => {
                if existing_pantheon(cand) {
                    return Ok(Bound::Existing(cand));
                }
            }
        }
    }
    anyhow::bail!("no free port in {port_req}..{}", port_req.saturating_add(9))
}

/// Load the universe and serve forever. Thread per connection: browsers open
/// speculative sockets that never send a request; served sequentially each one
/// would stall the accept loop for its whole read timeout and the page
/// appears frozen.
fn run_loop(listener: TcpListener, web: std::sync::Arc<PathBuf>) {
    let (uni, _health) = crate::data::Universe::load_with_health();
    let uni = std::sync::Arc::new(uni);
    let spice_loaded = crate::spice::spice().is_some();
    println!(
        "  web root {} · spice {} · {} stars · {} exo systems",
        web.display(),
        if spice_loaded {
            "loaded"
        } else {
            "NOT loaded (mean elements)"
        },
        uni.stars.len(),
        uni.exos.len()
    );
    for stream in listener.incoming() {
        let Ok(s) = stream else { continue };
        s.set_nodelay(true).ok();
        let uni = uni.clone();
        let web = web.clone();
        std::thread::spawn(move || {
            let _ = handle(s, &uni, &web);
        });
    }
}

/// Start a server on a background thread and return its port: used by the
/// pantheon-3d window binary. It takes the first free port and never shares one
/// with another Pantheon, because this process's window owns this server's life.
/// Binding happens before the slow kernel load, so the first page request just
/// waits in the accept backlog.
pub fn spawn_background(port_req: u16) -> anyhow::Result<u16> {
    let web = std::sync::Arc::new(web_root()?);
    for port in port_req..port_req.saturating_add(10) {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            std::thread::spawn(move || run_loop(listener, web));
            return Ok(port);
        }
    }
    anyhow::bail!("no free port in {port_req}..{}", port_req.saturating_add(9))
}

/// Backend mode: serve and block. It never opens a browser; the app's window is
/// the `pantheon-3d` binary (src-tauri), which embeds this same server.
pub fn serve(port_req: u16) -> anyhow::Result<()> {
    let web = std::sync::Arc::new(web_root()?);
    match bind_or_find(port_req)? {
        Bound::Existing(port) => {
            println!("pantheon backend already running on http://127.0.0.1:{port}/");
            Ok(())
        }
        Bound::Listener(listener, port) => {
            println!("pantheon backend on http://127.0.0.1:{port}/ (the window is pantheon-3d)");
            println!("  loading kernels and catalogs… (Ctrl-C to stop)");
            run_loop(listener, web);
            Ok(())
        }
    }
}

/// Is the thing on this port one of ours? Cheap raw-HTTP probe of /api/meta.
fn existing_pantheon(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(400)) else {
        return false;
    };
    s.set_read_timeout(Some(std::time::Duration::from_millis(800)))
        .ok();
    if s.write_all(b"GET /api/meta HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut buf = Vec::new();
    let _ = std::io::Read::read_to_end(&mut s, &mut buf);
    let text = String::from_utf8_lossy(&buf);
    text.contains("\"kernels\"") && text.contains("\"retrieved\"")
}

fn web_root() -> anyhow::Result<PathBuf> {
    let assets = crate::data::asset_root();
    let mut cands = vec![assets.parent().unwrap_or(&assets).join("pantheon-web")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            cands.push(dir.join("../../pantheon-web"));
        }
    }
    cands.push(PathBuf::from("pantheon-web"));
    for c in &cands {
        if c.join("index.html").exists() {
            return Ok(c.clone());
        }
    }
    anyhow::bail!(
        "pantheon-web/index.html not found near {}",
        assets.display()
    )
}

fn handle(stream: TcpStream, uni: &crate::data::Universe, web: &Path) -> std::io::Result<()> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    // drain headers, keeping Content-Length for POST bodies
    let mut content_length: usize = 0;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    if method == "POST" && path == "/api/shot" {
        // dev/verification aid: body = raw base64 JPEG (no data: prefix);
        // ?name=foo -> exports/webshots/foo.jpg
        let mut body = vec![0u8; content_length.min(32 << 20)];
        std::io::Read::read_exact(&mut reader, &mut body)?;
        let mut stream = reader.into_inner();
        let name: String = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("name="))
            .unwrap_or("shot")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        let b64: Vec<u8> = body
            .iter()
            .copied()
            .filter(|b| !b.is_ascii_whitespace())
            .collect();
        return match b64_decode(&b64) {
            Some(bytes) => {
                let dir = std::env::var("PANTHEON_SHOT_DIR")
                    .unwrap_or_else(|_| "exports/webshots".into());
                std::fs::create_dir_all(&dir).ok();
                let path = format!("{dir}/{name}.jpg");
                match std::fs::write(&path, bytes) {
                    Ok(()) => respond(&mut stream, 200, "text/plain", path.as_bytes()),
                    Err(e) => respond(&mut stream, 500, "text/plain", e.to_string().as_bytes()),
                }
            }
            None => respond(&mut stream, 400, "text/plain", b"bad base64"),
        };
    }
    if method == "POST" && path.starts_with("/api/qa/") && qa_enabled() {
        let mut body = vec![0u8; content_length.min(4 << 20)];
        std::io::Read::read_exact(&mut reader, &mut body)?;
        let mut stream = reader.into_inner();
        return qa_post(&mut stream, path, query, body);
    }
    let mut stream = reader.into_inner();
    if method == "GET" && path == "/api/qa/poll" && qa_enabled() {
        return qa_poll(&mut stream);
    }
    if method != "GET" {
        return respond(&mut stream, 405, "text/plain", b"method not allowed");
    }
    if path.contains("..") {
        return respond(&mut stream, 403, "text/plain", b"forbidden");
    }
    match path {
        "/api/states" => api_states(&mut stream, query),
        "/api/tracks" => api_tracks(&mut stream, uni),
        "/api/meta" => api_meta(&mut stream, uni),
        _ => {
            let rel = path.trim_start_matches('/');
            let file = if rel.is_empty() {
                web.join("index.html")
            } else if let Some(d) = rel.strip_prefix("data/") {
                crate::data::asset_root().join("data").join(d)
            } else if let Some(d) = rel.strip_prefix("fonts/") {
                crate::data::asset_root().join("fonts").join(d)
            } else {
                web.join(rel)
            };
            match std::fs::read(&file) {
                Ok(body) => respond(&mut stream, 200, mime(&file), &body),
                Err(_) => respond(&mut stream, 404, "text/plain", b"not found"),
            }
        }
    }
}

fn b64_decode(s: &[u8]) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a' + 26) as u32),
            b'0'..=b'9' => Some((b - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let s: &[u8] = {
        let mut end = s.len();
        while end > 0 && s[end - 1] == b'=' {
            end -= 1;
        }
        &s[..end]
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4 + 3);
    for chunk in s.chunks(4) {
        let mut acc: u32 = 0;
        for (i, &b) in chunk.iter().enumerate() {
            acc |= val(b)? << (18 - 6 * i);
        }
        let n = chunk.len();
        if n >= 2 {
            out.push((acc >> 16) as u8);
        }
        if n >= 3 {
            out.push((acc >> 8) as u8);
        }
        if n == 4 {
            out.push(acc as u8);
        }
    }
    Some(out)
}

fn mime(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "geojson" => "application/json",
        "csv" => "text/csv",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ttf" => "font/ttf",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Same-origin only. `connect-src 'self'` is what lets fetch() reach /api and
/// /data inside WebKitGTK; the inline script is the page's import map.
const CSP: &str =
    "default-src 'self'; connect-src 'self'; img-src 'self' blob: data:; font-src 'self'; \
style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; media-src 'self' blob:";
/// The test hooks evaluate scripts in the page, which also needs 'unsafe-eval'.
const CSP_QA: &str = "default-src 'self'; connect-src 'self'; img-src 'self' blob: data:; font-src 'self'; \
style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; media-src 'self' blob:";

fn respond(s: &mut TcpStream, code: u16, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let phrase = match code {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {code} {phrase}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nContent-Security-Policy: {}\r\nConnection: close\r\n\r\n",
        body.len(),
        if qa_enabled() { CSP_QA } else { CSP },
    );
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    s.flush()
}

fn json_out(s: &mut TcpStream, v: serde_json::Value) -> std::io::Result<()> {
    respond(s, 200, "application/json", v.to_string().as_bytes())
}

fn query_f64(query: &str, key: &str) -> Option<f64> {
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
}

fn api_states(s: &mut TcpStream, query: &str) -> std::io::Result<()> {
    let now_jd = 2_440_587.5
        + std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
            / 86_400.0
        + 69.184 / 86_400.0;
    let jd = query_f64(query, "jd").unwrap_or(now_jd);
    let h = 0.01; // days, central difference for velocity
    let src = match crate::spice::spice() {
        Some(sp) if jd >= sp.jd0 && jd <= sp.jd1 => "spice",
        _ => "elements",
    };
    let bodies: Vec<serde_json::Value> = (0..crate::sim::PLANETS.len())
        .map(|i| {
            let p = crate::sim::body_pos(i, jd);
            let v = (crate::sim::body_pos(i, jd + h) - crate::sim::body_pos(i, jd - h)) / (2.0 * h);
            serde_json::json!({
                "name": crate::sim::PLANETS[i].name,
                "pos": [p.x, p.y, p.z],
                "vel": [v.x, v.y, v.z],
            })
        })
        .collect();
    let moons: Vec<serde_json::Value> = (0..crate::sim::MOONS.len())
        .map(|mi| {
            let m = &crate::sim::MOONS[mi];
            let p = crate::sim::moon_rel(mi, jd);
            let v =
                (crate::sim::moon_rel(mi, jd + h) - crate::sim::moon_rel(mi, jd - h)) / (2.0 * h);
            serde_json::json!({
                "name": m.name,
                "parent": crate::sim::PLANETS[m.planet].name,
                "pos": [p.x, p.y, p.z],
                "vel": [v.x, v.y, v.z],
            })
        })
        .collect();
    json_out(
        s,
        serde_json::json!({ "jd": jd, "src": src, "bodies": bodies, "moons": moons }),
    )
}

fn api_tracks(s: &mut TcpStream, uni: &crate::data::Universe) -> std::io::Result<()> {
    let tracks: Vec<serde_json::Value> = uni
        .craft
        .iter()
        .map(|c| {
            let samples: Vec<serde_json::Value> = c
                .track
                .samples
                .iter()
                .map(|(jd, p, v)| serde_json::json!([jd, p.x, p.y, p.z, v.x, v.y, v.z]))
                .collect();
            serde_json::json!({ "name": c.name, "note": c.note, "samples": samples })
        })
        .collect();
    json_out(s, serde_json::Value::Array(tracks))
}

fn api_meta(s: &mut TcpStream, uni: &crate::data::Universe) -> std::io::Result<()> {
    let kernels = crate::spice::spice()
        .map(|sp| sp.kernel_summary())
        .unwrap_or_else(|| "none".into());
    // the app asks for textures only if they exist: a fresh clone has none
    let textures: Vec<String> = std::fs::read_dir(crate::data::asset_root().join("data/textures"))
        .map(|dir| {
            dir.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    json_out(
        s,
        serde_json::json!({
            "retrieved": uni.retrieved,
            "kernels": kernels,
            "spice": crate::spice::spice().is_some(),
            "textures": textures,
            "stars": uni.stars.len(),
            "exo_systems": uni.exos.len(),
            "craft": uni.craft.len(),
        }),
    )
}

// ---- test hooks (PANTHEON_QA=1 only) ------------------------------------------

fn qa_enabled() -> bool {
    std::env::var_os("PANTHEON_QA").is_some_and(|v| v != "0")
}

#[derive(Default)]
struct QaState {
    pending: std::collections::VecDeque<(u64, String)>,
    results: std::collections::HashMap<u64, String>,
    next_id: u64,
}

fn qa_state() -> &'static (std::sync::Mutex<QaState>, std::sync::Condvar) {
    static STATE: std::sync::OnceLock<(std::sync::Mutex<QaState>, std::sync::Condvar)> =
        std::sync::OnceLock::new();
    STATE.get_or_init(Default::default)
}

/// The window long-polls here for the next script to run.
fn qa_poll(s: &mut TcpStream) -> std::io::Result<()> {
    let (lock, cv) = qa_state();
    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let (mut guard, _) = cv
        .wait_timeout_while(guard, std::time::Duration::from_secs(20), |st| {
            st.pending.is_empty()
        })
        .unwrap_or_else(|e| e.into_inner());
    match guard.pending.pop_front() {
        Some((id, js)) => json_out(s, serde_json::json!({ "id": id, "js": js })),
        None => json_out(s, serde_json::json!({})),
    }
}

fn qa_post(s: &mut TcpStream, path: &str, query: &str, body: Vec<u8>) -> std::io::Result<()> {
    let (lock, cv) = qa_state();
    let text = String::from_utf8_lossy(&body).into_owned();
    match path {
        "/api/qa/log" => {
            eprintln!("[page] {text}");
            respond(s, 200, "text/plain", b"ok")
        }
        "/api/qa/result" => {
            if let Some(id) = query_f64(query, "id") {
                lock.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .results
                    .insert(id as u64, text);
                cv.notify_all();
            }
            respond(s, 200, "text/plain", b"ok")
        }
        "/api/qa/eval" => {
            let wait = query_f64(query, "timeout").unwrap_or(30.0);
            let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
            let id = st.next_id;
            st.next_id += 1;
            st.pending.push_back((id, text));
            cv.notify_all();
            let (mut st, res) = cv
                .wait_timeout_while(st, std::time::Duration::from_secs_f64(wait), |st| {
                    !st.results.contains_key(&id)
                })
                .unwrap_or_else(|e| e.into_inner());
            if res.timed_out() {
                st.pending.retain(|(i, _)| *i != id);
                return respond(s, 504, "text/plain", b"no answer from the window");
            }
            let out = st.results.remove(&id).unwrap_or_default();
            respond(s, 200, "application/json", out.as_bytes())
        }
        _ => respond(s, 404, "text/plain", b"not found"),
    }
}
