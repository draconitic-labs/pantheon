//! Local web server for the three.js edition (`pantheon-cli --serve`).
//!
//! The Rust core stays the scientific backend: it serves the static
//! `pantheon-web/` app, the cached datasets under `/data/`, and a small API —
//! `/api/states` (SPICE-grade heliocentric states + finite-difference
//! velocities for client-side extrapolation), `/api/tracks` (parsed Horizons
//! spacecraft arcs), `/api/meta`. Binds 127.0.0.1 only; sequential handling
//! with `Connection: close` keeps it dependency-free and simple.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

pub fn serve(port_req: u16, open: bool) -> anyhow::Result<()> {
    let web = web_root()?;
    // Bind BEFORE the slow kernel/catalog load: the browser can open at once
    // and its first request just waits in the accept backlog. If the port is
    // taken by another Pantheon (double-clicked launcher), reuse it; if taken
    // by something else, walk forward to the next free port.
    let mut bound = None;
    for cand in port_req..port_req.saturating_add(10) {
        match TcpListener::bind(("127.0.0.1", cand)) {
            Ok(l) => {
                bound = Some((l, cand));
                break;
            }
            Err(_) => {
                if existing_pantheon(cand) {
                    println!("pantheon web already running — http://127.0.0.1:{cand}/");
                    if open {
                        spawn_browser(cand);
                    }
                    return Ok(());
                }
            }
        }
    }
    let Some((listener, port)) = bound else {
        anyhow::bail!("no free port in {port_req}..{}", port_req.saturating_add(9));
    };
    println!("pantheon web — http://127.0.0.1:{port}/");
    if open {
        spawn_browser(port);
    }
    println!("  loading kernels and catalogs…");
    let (uni, _health) = crate::data::Universe::load_with_health();
    let spice_loaded = crate::spice::spice().is_some();
    println!(
        "  web root {} · spice {} · {} stars · {} exo systems · Ctrl-C to stop",
        web.display(),
        if spice_loaded { "loaded" } else { "NOT loaded (mean elements)" },
        uni.stars.len(),
        uni.exos.len()
    );
    for stream in listener.incoming() {
        let Ok(s) = stream else { continue };
        let _ = handle(s, &uni, &web);
    }
    Ok(())
}

fn spawn_browser(port: u16) {
    std::process::Command::new("cmd")
        .args(["/C", "start", "", &format!("http://127.0.0.1:{port}/")])
        .spawn()
        .ok();
}

/// Is the thing on this port one of ours? Cheap raw-HTTP probe of /api/meta.
fn existing_pantheon(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(400)) else {
        return false;
    };
    s.set_read_timeout(Some(std::time::Duration::from_millis(800))).ok();
    if s.write_all(b"GET /api/meta HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").is_err() {
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
    anyhow::bail!("pantheon-web/index.html not found near {}", assets.display())
}

fn handle(stream: TcpStream, uni: &crate::data::Universe, web: &PathBuf) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
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
        let b64: Vec<u8> = body.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
        return match b64_decode(&b64) {
            Some(bytes) => {
                std::fs::create_dir_all("exports/webshots").ok();
                let path = format!("exports/webshots/{name}.jpg");
                match std::fs::write(&path, bytes) {
                    Ok(()) => respond(&mut stream, 200, "text/plain", path.as_bytes()),
                    Err(e) => respond(&mut stream, 500, "text/plain", e.to_string().as_bytes()),
                }
            }
            None => respond(&mut stream, 400, "text/plain", b"bad base64"),
        };
    }
    let mut stream = reader.into_inner();
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

fn mime(p: &PathBuf) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "geojson" => "application/json",
        "csv" => "text/csv",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn respond(s: &mut TcpStream, code: u16, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let phrase = match code {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {code} {phrase}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
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
    let now_jd = 2_440_587.5 + std::time::SystemTime::now()
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
            let v = (crate::sim::moon_rel(mi, jd + h) - crate::sim::moon_rel(mi, jd - h)) / (2.0 * h);
            serde_json::json!({
                "name": m.name,
                "parent": crate::sim::PLANETS[m.planet].name,
                "pos": [p.x, p.y, p.z],
                "vel": [v.x, v.y, v.z],
            })
        })
        .collect();
    json_out(s, serde_json::json!({ "jd": jd, "src": src, "bodies": bodies, "moons": moons }))
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
    json_out(
        s,
        serde_json::json!({
            "retrieved": uni.retrieved,
            "kernels": kernels,
            "stars": uni.stars.len(),
            "exo_systems": uni.exos.len(),
            "craft": uni.craft.len(),
        }),
    )
}
