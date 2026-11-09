//! Local backend for Pantheon 3D. It runs inside the `pantheon-3d` Tauri window
//! (src-tauri), or alone with `pantheon-cli --serve`; it never opens a browser.
//!
//! The Rust core stays the scientific backend: it serves the static
//! `pantheon-web/` app, the catalogs and surface maps under `/data/`, the bundled
//! fonts under `/fonts/`, and a small API: `/api/states` (SPICE-grade heliocentric
//! states + finite-difference velocities), `/api/tracks` (parsed Horizons
//! spacecraft arcs), `/api/meta` (what data is present, so the app can say what
//! is missing). `Connection: close` keeps it simple.
//!
//! # Who can reach this server, and what stops them
//!
//! It binds 127.0.0.1, so nothing off this machine can connect. That still leaves
//! every local program and, through the browser, every web page the user has open.
//! A page can send requests to `http://127.0.0.1:<port>` even if it can't read the
//! replies, and with DNS rebinding its own hostname can resolve here, which makes it
//! same-origin as far as the browser is concerned. So, in order:
//!
//! 1. **Every request passes [`envelope_ok`] before anything else.** `Host` must be
//!    exactly `127.0.0.1:<bound port>` or `localhost:<bound port>`, which defeats
//!    rebinding. `Origin`, if present, must be that same origin. `Sec-Fetch-Site`, if
//!    present, must be `same-origin` or `none`, because an `<img>` on another site sends
//!    no `Origin` at all. Anything else gets a 403, and there are no CORS headers.
//! 2. **Files go through [`resolve`].** Every path component must be a plain name, the
//!    path must lie in a folder and have a type the page really fetches, and the
//!    canonicalized result must stay under its canonicalized base. Kernels, spacecraft
//!    tracks and everything else on disk are not reachable over HTTP. Files are streamed,
//!    so a large one costs no memory per request.
//! 3. **The test hooks exist only when `PANTHEON_QA` is exactly `1`.** `POST /api/qa/eval`
//!    runs JavaScript in the open window (which polls `/api/qa/poll` when loaded with
//!    `?qa`) and returns the result, `POST /api/qa/log` collects page errors, and
//!    `POST /api/shot` saves a screenshot the page sends. Without QA mode these routes
//!    don't exist. With it, any local program can drive the window, so it's for testing.
//!
//! The tests at the end of this file exercise all of this over real sockets.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Bind the first free port from `port_req`; if a port is taken by another
/// Pantheon, report that instead so callers can reuse it.
enum Bound {
    Listener(TcpListener, u16),
    Existing(u16),
}

fn bind_or_find(port_req: u16) -> anyhow::Result<Bound> {
    for cand in port_req..=port_req.saturating_add(9) {
        match TcpListener::bind(("127.0.0.1", cand)) {
            Ok(l) => {
                // not `cand`: port 0 means the kernel chose one
                let port = l.local_addr()?.port();
                return Ok(Bound::Listener(l, port));
            }
            Err(_) => {
                if existing_pantheon(cand) {
                    return Ok(Bound::Existing(cand));
                }
            }
        }
    }
    anyhow::bail!("no free port in {port_req}..{}", port_req.saturating_add(9))
}

/// What one running server answers for, fixed at startup.
struct Site {
    /// the port actually bound, which the `Host` check compares against
    port: u16,
    qa: bool,
    web: PathBuf,
    /// asset folders, searched in order: see [`crate::data::asset_roots`]
    assets: Vec<PathBuf>,
    shot_dir: PathBuf,
}

impl Site {
    /// `rel` in the first asset folder that has it
    fn asset(&self, rel: &str) -> PathBuf {
        crate::data::first_having(&self.assets, Path::new(rel))
    }
}

/// QA mode is on only for exactly `PANTHEON_QA=1`. Empty, `0` or anything else is off.
fn qa_requested(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| v == "1")
}

/// Load the universe and serve forever.
fn run_loop(listener: TcpListener, port: u16, web: PathBuf) {
    let (uni, _health) = crate::data::Universe::load_with_health();
    let spice_loaded = crate::spice::spice().is_some();
    for problem in crate::spice::problems() {
        eprintln!("pantheon: {problem}");
    }
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
    let site = Site {
        port,
        qa: qa_requested(std::env::var_os("PANTHEON_QA").as_deref()),
        web,
        assets: crate::data::asset_roots(),
        shot_dir: std::env::var_os("PANTHEON_SHOT_DIR")
            .map_or_else(|| PathBuf::from("exports/webshots"), PathBuf::from),
    };
    accept_loop(listener, Arc::new(uni), Arc::new(site));
}

/// Thread per connection: browsers open speculative sockets that never send a
/// request; served sequentially each one would stall the accept loop for its whole
/// read timeout and the page appears frozen.
fn accept_loop(listener: TcpListener, uni: Arc<crate::data::Universe>, site: Arc<Site>) {
    for stream in listener.incoming() {
        let Ok(s) = stream else { continue };
        s.set_nodelay(true).ok();
        let uni = uni.clone();
        let site = site.clone();
        std::thread::spawn(move || {
            let _ = handle(s, &uni, &site);
        });
    }
}

/// Start a server on a background thread and return its port: used by the
/// pantheon-3d window binary. It takes the first free port and never shares one
/// with another Pantheon, because this process's window owns this server's life.
/// Binding happens before the slow kernel load, so the first page request just
/// waits in the accept backlog.
pub fn spawn_background(port_req: u16) -> anyhow::Result<u16> {
    let web = web_root()?;
    for cand in port_req..=port_req.saturating_add(9) {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", cand)) {
            // not `cand`: port 0 means the kernel chose one
            let port = listener.local_addr()?.port();
            std::thread::spawn(move || run_loop(listener, port, web));
            return Ok(port);
        }
    }
    anyhow::bail!("no free port in {port_req}..{}", port_req.saturating_add(9))
}

/// Backend mode: serve and block. It never opens a browser; the app's window is
/// the `pantheon-3d` binary (src-tauri), which embeds this same server.
pub fn serve(port_req: u16) -> anyhow::Result<()> {
    let web = web_root()?;
    match bind_or_find(port_req)? {
        Bound::Existing(port) => {
            println!("pantheon backend already running on http://127.0.0.1:{port}/");
            Ok(())
        }
        Bound::Listener(listener, port) => {
            println!("pantheon backend on http://127.0.0.1:{port}/ (the window is pantheon-3d)");
            println!("  loading kernels and catalogs… (Ctrl-C to stop)");
            run_loop(listener, port, web);
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
    let probe =
        format!("GET /api/meta HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if s.write_all(probe.as_bytes()).is_err() {
        return false;
    }
    let mut buf = Vec::new();
    let _ = std::io::Read::read_to_end(&mut s, &mut buf);
    let text = String::from_utf8_lossy(&buf);
    text.contains("\"kernels\"") && text.contains("\"retrieved\"")
}

fn web_root() -> anyhow::Result<PathBuf> {
    let roots = crate::data::asset_roots();
    // an installed package's page comes from the package: never from the user's data
    // folder, which only holds downloads, or from the working directory
    let installed = crate::data::installed();
    let user = crate::data::user_data_dir()
        .map(|d| d.join("assets"))
        .map(|u| u.canonicalize().unwrap_or(u));
    let mut cands: Vec<PathBuf> = roots
        .iter()
        .filter(|a| !installed || Some(*a) != user.as_ref())
        .map(|a| a.parent().unwrap_or(a).join("pantheon-web"))
        .collect();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            cands.push(dir.join("../../pantheon-web"));
        }
    }
    if !installed {
        cands.push(PathBuf::from("pantheon-web"));
    }
    for c in &cands {
        if c.join("index.html").exists() {
            return Ok(c.clone());
        }
    }
    anyhow::bail!(
        "pantheon-web/index.html not found near {}",
        roots[0].display()
    )
}

/// A request line and its headers, with header names in lower case.
struct Head {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl Head {
    /// The value of a header that may appear at most once. `Err` if it's repeated.
    fn single(&self, name: &str) -> Result<Option<&str>, ()> {
        let mut values = self.headers.iter().filter(|(k, _)| k == name);
        let first = values.next().map(|(_, v)| v.as_str());
        match values.next() {
            Some(_) => Err(()),
            None => Ok(first),
        }
    }
}

/// The most a request line and headers may take. Nothing this app sends comes close.
const HEAD_MAX: u64 = 16 << 10;

/// `None` if the client went away or the head is malformed or too long.
fn read_head(reader: &mut BufReader<TcpStream>) -> std::io::Result<Option<Head>> {
    let mut limited = reader.by_ref().take(HEAD_MAX);
    let mut line = String::new();
    limited.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Ok(None);
    };
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if limited.read_line(&mut h)? == 0 {
            return Ok(None);
        }
        if h == "\r\n" || h == "\n" {
            break;
        }
        let Some((name, value)) = h.split_once(':') else {
            return Ok(None);
        };
        headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
    }
    Ok(Some(Head {
        method: method.to_string(),
        target: target.to_string(),
        headers,
    }))
}

/// Is this request from the app's own page, or from a local program speaking as one?
/// See the module comment for why each check is here.
fn envelope_ok(head: &Head, port: u16) -> bool {
    let (Ok(Some(host)), Ok(origin), Ok(fetch_site)) = (
        head.single("host"),
        head.single("origin"),
        head.single("sec-fetch-site"),
    ) else {
        return false;
    };
    let own_host = host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}");
    own_host
        && origin.is_none_or(|o| o == format!("http://{host}"))
        && fetch_site.is_none_or(|f| f == "same-origin" || f == "none")
}

/// Body limits for the test hooks: a screenshot, and anything else.
const SHOT_BODY_MAX: usize = 32 << 20;
const QA_BODY_MAX: usize = 4 << 20;

fn handle(stream: TcpStream, uni: &crate::data::Universe, site: &Site) -> std::io::Result<()> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(30)))
        .ok();
    let mut reader = BufReader::new(stream);
    let Some(head) = read_head(&mut reader)? else {
        return Ok(());
    };
    let (path, query) = head
        .target
        .split_once('?')
        .unwrap_or((head.target.as_str(), ""));
    let hook =
        site.qa && head.method == "POST" && (path == "/api/shot" || path.starts_with("/api/qa/"));
    let allowed = envelope_ok(&head, site.port);
    // read a hook's body while the reader still holds whatever it has buffered
    let content_length: usize = head
        .single("content-length")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let limit = if path == "/api/shot" {
        SHOT_BODY_MAX
    } else {
        QA_BODY_MAX
    };
    let mut body = Vec::new();
    if allowed && hook && content_length <= limit {
        body.resize(content_length, 0);
        reader.read_exact(&mut body)?;
    }
    let mut out = Conn {
        stream: reader.into_inner(),
        qa: site.qa,
    };
    if !allowed {
        return respond(&mut out, 403, "text/plain", b"forbidden");
    }
    if hook && content_length > limit {
        return respond(&mut out, 413, "text/plain", b"too large");
    }
    match (head.method.as_str(), path) {
        ("POST", "/api/shot") if hook => save_shot(&mut out, site, query, &body),
        ("POST", _) if hook => qa_post(&mut out, path, query, body),
        ("GET", "/api/qa/poll") if site.qa => qa_poll(&mut out),
        ("GET", "/api/states") => api_states(&mut out, query),
        ("GET", "/api/tracks") => api_tracks(&mut out, uni),
        ("GET", "/api/meta") => api_meta(&mut out, uni, site),
        ("GET", _) => match resolve(path, site) {
            Served::File(file, ctype) => send_file(&mut out, &file, ctype),
            Served::Missing => respond(&mut out, 404, "text/plain", b"not found"),
            Served::Refused => respond(&mut out, 403, "text/plain", b"forbidden"),
        },
        _ => respond(&mut out, 405, "text/plain", b"method not allowed"),
    }
}

/// What a request path leads to.
enum Served {
    File(PathBuf, &'static str),
    Missing,
    Refused,
}

/// A name that means only itself on every platform: no separators, no encoding, no
/// drive letters, not `.` or `..`, and nothing hidden.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

const HTML: &[(&str, &str)] = &[("html", "text/html; charset=utf-8")];
const CSS: &[(&str, &str)] = &[("css", "text/css; charset=utf-8")];
const SCRIPT: &[(&str, &str)] = &[("js", "application/javascript; charset=utf-8")];
const FONT: &[(&str, &str)] = &[("ttf", "font/ttf")];
const CATALOG: &[(&str, &str)] = &[("csv", "text/csv"), ("geojson", "application/json")];
const MAP: &[(&str, &str)] = &[("jpg", "image/jpeg"), ("png", "image/png")];

/// Decide which file, if any, a request path may read. These are the folders and types
/// the page really fetches, and nothing else: no special cases, no decoding.
fn resolve(path: &str, site: &Site) -> Served {
    let Some(rel) = path.strip_prefix('/') else {
        return Served::Refused;
    };
    let parts: Vec<&str> = if rel.is_empty() {
        vec!["index.html"]
    } else {
        rel.split('/').collect()
    };
    if !parts.iter().all(|p| plain_name(p)) {
        return Served::Refused;
    }
    // (folder this path must stay under, the path inside it, the types allowed there)
    let (base, inside, types) = match parts.as_slice() {
        ["index.html"] => (site.web.clone(), &parts[..], HTML),
        ["css", _] => (site.web.clone(), &parts[..], CSS),
        ["js" | "lib", ..] => (site.web.clone(), &parts[..], SCRIPT),
        ["fonts", _] => (site.asset("fonts"), &parts[1..], FONT),
        ["data", "textures", _] => (site.asset("data/textures"), &parts[2..], MAP),
        // a catalog may be a download or the copy a package ships, so look file by file
        ["data", name] => {
            let file = site.asset(&format!("data/{name}"));
            let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
            (dir, &parts[1..], CATALOG)
        }
        _ => return Served::Refused,
    };
    let name = parts[parts.len() - 1];
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
    let Some(&(_, ctype)) = types.iter().find(|(t, _)| *t == ext) else {
        return Served::Refused;
    };
    let wanted = inside.iter().fold(base.clone(), |p, part| p.join(part));
    let (Ok(root), Ok(file)) = (base.canonicalize(), wanted.canonicalize()) else {
        return Served::Missing;
    };
    if !file.starts_with(&root) {
        return Served::Refused;
    }
    if !file.is_file() {
        return Served::Missing;
    }
    Served::File(file, ctype)
}

/// Stream a file, so a large one costs no memory however many requests ask for it.
fn send_file(s: &mut Conn, file: &Path, ctype: &str) -> std::io::Result<()> {
    let opened = std::fs::File::open(file).and_then(|f| {
        let len = f.metadata()?.len();
        Ok((f, len))
    });
    let Ok((f, len)) = opened else {
        return respond(s, 404, "text/plain", b"not found");
    };
    write_head(s, 200, ctype, len)?;
    std::io::copy(&mut f.take(len), &mut s.stream)?;
    s.stream.flush()
}

/// Test hook: the body is a raw base64 JPEG (no data: prefix), and
/// `?name=foo` saves it as foo.jpg in the shot folder.
fn save_shot(s: &mut Conn, site: &Site, query: &str, body: &[u8]) -> std::io::Result<()> {
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
    match b64_decode(&b64) {
        Some(bytes) => {
            std::fs::create_dir_all(&site.shot_dir).ok();
            let path = site.shot_dir.join(format!("{name}.jpg"));
            match std::fs::write(&path, bytes) {
                Ok(()) => respond(s, 200, "text/plain", path.display().to_string().as_bytes()),
                Err(e) => respond(s, 500, "text/plain", e.to_string().as_bytes()),
            }
        }
        None => respond(s, 400, "text/plain", b"bad base64"),
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

/// Same-origin only. `connect-src 'self'` is what lets fetch() reach /api and
/// /data inside WebKitGTK; the inline script is the page's import map.
const CSP: &str =
    "default-src 'self'; connect-src 'self'; img-src 'self' blob: data:; font-src 'self'; \
style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; media-src 'self' blob:";
/// The test hooks evaluate scripts in the page, which also needs 'unsafe-eval'.
const CSP_QA: &str = "default-src 'self'; connect-src 'self'; img-src 'self' blob: data:; font-src 'self'; \
style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; media-src 'self' blob:";

/// The open connection, and whether this server is in QA mode (its CSP depends on it).
struct Conn {
    stream: TcpStream,
    qa: bool,
}

fn write_head(s: &mut Conn, code: u16, ctype: &str, len: u64) -> std::io::Result<()> {
    let phrase = match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        _ => "Error",
    };
    // same-origin resources only, for browsers that don't send Sec-Fetch-Site
    let head = format!(
        "HTTP/1.1 {code} {phrase}\r\nContent-Type: {ctype}\r\nContent-Length: {len}\r\nCache-Control: no-cache\r\nX-Content-Type-Options: nosniff\r\nCross-Origin-Resource-Policy: same-origin\r\nContent-Security-Policy: {}\r\nConnection: close\r\n\r\n",
        if s.qa { CSP_QA } else { CSP },
    );
    s.stream.write_all(head.as_bytes())
}

fn respond(s: &mut Conn, code: u16, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    write_head(s, code, ctype, body.len() as u64)?;
    s.stream.write_all(body)?;
    s.stream.flush()
}

fn json_out(s: &mut Conn, v: serde_json::Value) -> std::io::Result<()> {
    respond(s, 200, "application/json", v.to_string().as_bytes())
}

fn query_f64(query: &str, key: &str) -> Option<f64> {
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
}

fn api_states(s: &mut Conn, query: &str) -> std::io::Result<()> {
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
                "src": src,
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
                // moon by moon: no kernel carries the moons of Uranus, for one
                "src": if crate::sim::moon_is_spice(mi, jd) { "spice" } else { "elements" },
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

fn api_tracks(s: &mut Conn, uni: &crate::data::Universe) -> std::io::Result<()> {
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

fn api_meta(s: &mut Conn, uni: &crate::data::Universe, site: &Site) -> std::io::Result<()> {
    let kernels = crate::spice::spice()
        .map(|sp| sp.kernel_summary())
        .unwrap_or_else(|| "none".into());
    // the app asks for textures only if they exist: a fresh clone has none
    let textures: Vec<String> = std::fs::read_dir(site.asset("data/textures"))
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
            "kernel_problems": crate::spice::problems(),
            "textures": textures,
            "stars": uni.stars.len(),
            "exo_systems": uni.exos.len(),
            "craft": uni.craft.len(),
        }),
    )
}

// ---- test hooks (PANTHEON_QA=1 only) ------------------------------------------

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
fn qa_poll(s: &mut Conn) -> std::io::Result<()> {
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

fn qa_post(s: &mut Conn, path: &str, query: &str, body: Vec<u8>) -> std::io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::OnceLock;

    /// A throwaway web root and asset folder with a canary file beside them. Teardown removes
    /// exactly what this made, one path at a time.
    struct Fixture {
        root: PathBuf,
        made: Vec<PathBuf>,
    }

    const CANARY: &str = "canary that must stay put";

    impl Fixture {
        fn new() -> Fixture {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "pantheon-serve-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut fx = Fixture {
                root,
                made: Vec::new(),
            };
            for dir in [
                "",
                "web",
                "web/js",
                "web/js/scene",
                "web/css",
                "web/lib",
                "assets",
                "assets/data",
                "assets/data/spice",
                "assets/data/spacecraft",
                "assets/data/textures",
                "assets/fonts",
                "shots",
            ] {
                let path = fx.root.join(dir);
                std::fs::create_dir(&path).unwrap();
                fx.made.push(path);
            }
            for (file, text) in [
                ("outside.txt", CANARY),
                ("web/index.html", "<title>page</title>"),
                ("web/js/a.js", "// a"),
                ("web/js/scene/b.js", "// b"),
                ("web/css/style.css", "/* css */"),
                ("web/lib/three.js", "// three"),
                ("web/canary.txt", CANARY),
                ("web/notes.md", CANARY),
                ("assets/data/stars.csv", "ra,dec"),
                ("assets/data/land.geojson", "{}"),
                ("assets/data/fetch_log.txt", CANARY),
                ("assets/data/provenance.json", CANARY),
                ("assets/data/spice/kernel.bsp", CANARY),
                ("assets/data/spice/kernel.csv", CANARY),
                ("assets/data/spacecraft/voyager1.txt", CANARY),
                ("assets/data/textures/earth.jpg", "jpeg bytes"),
                ("assets/data/textures/rings.png", "png bytes"),
                ("assets/fonts/serif.ttf", "font bytes"),
                ("assets/fonts/OFL.txt", CANARY),
            ] {
                let path = fx.root.join(file);
                std::fs::write(&path, text).unwrap();
                fx.made.push(path);
            }
            #[cfg(unix)]
            for link in ["assets/data/escape.csv", "web/js/escape.js"] {
                let path = fx.root.join(link);
                std::os::unix::fs::symlink(fx.root.join("outside.txt"), &path).unwrap();
                fx.made.push(path);
            }
            fx
        }

        /// A server on a port the kernel picks, as `run_loop` would set it up.
        fn serve(&self, qa: bool) -> u16 {
            self.serve_from(qa, vec![self.root.join("assets")])
        }

        fn serve_from(&self, qa: bool, assets: Vec<PathBuf>) -> u16 {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            let site = Site {
                port,
                qa,
                web: self.root.join("web"),
                assets,
                shot_dir: self.root.join("shots"),
            };
            std::thread::spawn(move || accept_loop(listener, universe(), Arc::new(site)));
            port
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            for path in self.made.iter().rev() {
                if path.is_dir() && !path.is_symlink() {
                    let _ = std::fs::remove_dir(path);
                } else {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }

    fn universe() -> Arc<crate::data::Universe> {
        static UNI: OnceLock<Arc<crate::data::Universe>> = OnceLock::new();
        UNI.get_or_init(|| Arc::new(crate::data::Universe::load()))
            .clone()
    }

    struct Reply {
        code: u16,
        head: String,
        body: Vec<u8>,
    }

    impl Reply {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.body).into_owned()
        }
    }

    fn send(port: u16, request: &str) -> Reply {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        let mut raw = Vec::new();
        let _ = s.read_to_end(&mut raw);
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map_or(raw.len(), |i| i + 4);
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let code = head
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        Reply {
            code,
            head,
            body: raw[split..].to_vec(),
        }
    }

    /// A request with the right Host, plus whatever extra headers a test wants.
    fn request(port: u16, method: &str, path: &str, extra: &[&str], body: &str) -> Reply {
        let mut text = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\n",
            body.len()
        );
        for line in extra {
            text.push_str(line);
            text.push_str("\r\n");
        }
        text.push_str("\r\n");
        text.push_str(body);
        send(port, &text)
    }

    fn get(port: u16, path: &str) -> Reply {
        request(port, "GET", path, &[], "")
    }

    fn assert_refused(reply: &Reply, what: &str) {
        assert_eq!(reply.code, 403, "{what}: {}", reply.head);
        assert!(!reply.text().contains(CANARY), "{what} leaked the canary");
    }

    #[test]
    fn normal_same_origin_requests_work() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        let origin = format!("Origin: http://127.0.0.1:{port}");
        for (path, ctype, body) in [
            ("/", "text/html", "<title>page</title>"),
            ("/index.html", "text/html", "<title>page</title>"),
            ("/js/a.js", "application/javascript", "// a"),
            ("/js/scene/b.js", "application/javascript", "// b"),
            ("/lib/three.js", "application/javascript", "// three"),
            ("/css/style.css", "text/css", "/* css */"),
            ("/data/stars.csv", "text/csv", "ra,dec"),
            ("/data/land.geojson", "application/json", "{}"),
            ("/data/textures/earth.jpg", "image/jpeg", "jpeg bytes"),
            ("/data/textures/rings.png", "image/png", "png bytes"),
            ("/fonts/serif.ttf", "font/ttf", "font bytes"),
            ("/js/a.js?v=1", "application/javascript", "// a"),
        ] {
            // what the window sends: module scripts and fonts carry Origin, and
            // same-origin requests carry Sec-Fetch-Site where the engine supports it
            for extra in [
                &[][..],
                &[origin.as_str()][..],
                &["Sec-Fetch-Site: same-origin", origin.as_str()][..],
                &["Sec-Fetch-Site: none"][..],
                &["sec-fetch-site: same-origin"][..],
            ] {
                let r = request(port, "GET", path, extra, "");
                assert_eq!(r.code, 200, "{path} {extra:?}: {}", r.head);
                assert_eq!(r.text(), body, "{path}");
                assert!(r.head.contains(&format!("Content-Type: {ctype}")), "{path}");
            }
        }
        let meta = request(port, "GET", "/api/meta", &[origin.as_str()], "");
        assert_eq!(meta.code, 200);
        assert!(meta.text().contains("\"retrieved\""));
        let states = get(port, "/api/states?jd=2460000.5");
        assert_eq!(states.code, 200);
        assert!(states.text().contains("\"bodies\""));
        assert_eq!(get(port, "/api/tracks").code, 200);
        // localhost is the same server, and its own origin
        let r = send(
            port,
            &format!(
                "GET /js/a.js HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\n\r\n"
            ),
        );
        assert_eq!(r.code, 200, "{}", r.head);
    }

    /// An installed package: downloads in the user's folder, searched first, and the
    /// package's own read-only copy after it.
    #[test]
    fn a_package_serves_downloads_first_and_its_own_files_after() {
        let mut fx = Fixture::new();
        for dir in ["user", "user/data", "user/data/textures"] {
            let path = fx.root.join(dir);
            std::fs::create_dir(&path).unwrap();
            fx.made.push(path);
        }
        for (file, text) in [
            ("user/data/stars.csv", "newer stars"),
            ("user/data/textures/mars.jpg", "mars bytes"),
        ] {
            let path = fx.root.join(file);
            std::fs::write(&path, text).unwrap();
            fx.made.push(path);
        }
        #[cfg(unix)]
        {
            let path = fx.root.join("user/data/escape2.csv");
            std::os::unix::fs::symlink(fx.root.join("outside.txt"), &path).unwrap();
            fx.made.push(path);
        }
        let port = fx.serve_from(false, vec![fx.root.join("user"), fx.root.join("assets")]);
        for (path, body) in [
            ("/data/stars.csv", "newer stars"),
            ("/data/land.geojson", "{}"),
            ("/data/textures/mars.jpg", "mars bytes"),
            ("/fonts/serif.ttf", "font bytes"),
        ] {
            let r = get(port, path);
            assert_eq!((r.code, r.text().as_str()), (200, body), "{path}");
        }
        let meta = get(port, "/api/meta").text();
        assert!(meta.contains("mars.jpg"), "{meta}");
        #[cfg(unix)]
        for path in ["/data/escape2.csv", "/data/escape.csv"] {
            assert_refused(&get(port, path), path);
        }
        for path in [
            "/data/fetch_log.txt",
            "/data/provenance.json",
            "/fonts/OFL.txt",
        ] {
            assert_refused(&get(port, path), path);
        }
    }

    #[test]
    fn replies_carry_no_cors_permission() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        for path in ["/", "/data/stars.csv", "/api/meta", "/nope"] {
            let r = get(port, path);
            assert!(
                !r.head.to_ascii_lowercase().contains("access-control-"),
                "{path}: {}",
                r.head
            );
        }
    }

    #[test]
    fn a_missing_file_in_a_served_folder_is_404() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        for path in [
            "/js/nope.js",
            "/data/nope.csv",
            "/data/textures/nope.jpg",
            "/fonts/nope.ttf",
        ] {
            assert_eq!(get(port, path).code, 404, "{path}");
        }
    }

    #[test]
    fn absolute_paths_are_refused() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        let outside = fx.root.join("outside.txt");
        let outside = outside.display();
        for path in [
            format!("/{outside}"),
            format!("/data/{outside}"),
            format!("/fonts/{outside}"),
            format!("/js/{outside}"),
            format!("/data/textures/{outside}"),
            "/data//etc/passwd".to_string(),
            "/fonts//etc/passwd".to_string(),
            "//etc/passwd".to_string(),
            "/data/C:/Windows/win.ini".to_string(),
        ] {
            assert_refused(&get(port, &path), &path);
        }
    }

    #[test]
    fn parent_directories_are_refused() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        for path in [
            "/../outside.txt",
            "/../../outside.txt",
            "/js/../../outside.txt",
            "/js/../index.html",
            "/data/../../outside.txt",
            "/data/textures/../../../outside.txt",
            "/fonts/../../outside.txt",
            "/data/./stars.csv",
            "/js//a.js",
            "/js/a.js/",
            "/data/..",
        ] {
            assert_refused(&get(port, path), path);
        }
    }

    #[test]
    fn encoded_paths_are_refused() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        for path in [
            "/data/%2e%2e/%2e%2e/outside.txt",
            "/data/..%2f..%2foutside.txt",
            "/data/%2Fetc%2Fpasswd",
            "/data/%2fetc%2fpasswd",
            "/data/.%2e/.%2e/outside.txt",
            "/data/%252e%252e/%252e%252e/outside.txt",
            "/data/..%5c..%5coutside.txt",
            "/data/..\\..\\outside.txt",
            "/js/a%2ejs",
            "/js/a.js%00.csv",
            "/data/stars.csv%00",
        ] {
            assert_refused(&get(port, path), path);
        }
    }

    #[test]
    fn a_target_without_a_leading_slash_is_refused() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        let absolute_form = format!("http://127.0.0.1:{port}/js/a.js");
        for path in ["js/a.js", "*", absolute_form.as_str()] {
            assert_refused(&get(port, path), path);
        }
    }

    #[test]
    fn only_the_folders_and_types_the_page_fetches_are_served() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        for path in [
            "/data/spice/kernel.bsp",
            "/data/spice/kernel.csv",
            "/data/spacecraft/voyager1.txt",
            "/data/fetch_log.txt",
            "/data/provenance.json",
            "/fonts/OFL.txt",
            "/canary.txt",
            "/notes.md",
            "/data/textures/earth.txt",
            "/data/textures",
            "/data",
            "/js",
            "/css/style.js",
            "/js/a.css",
            "/index.css",
            "/README.md",
            "/Cargo.toml",
        ] {
            assert_refused(&get(port, path), path);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_base_is_refused() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        for path in ["/data/escape.csv", "/js/escape.js"] {
            assert_refused(&get(port, path), path);
        }
    }

    #[test]
    fn a_foreign_host_is_refused() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        let wrong_port = format!("Host: 127.0.0.1:{}", port.wrapping_add(1));
        let suffixed = format!("Host: 127.0.0.1:{port}.attacker.example");
        let padded_user = format!("Host: attacker.example@127.0.0.1:{port}");
        let ipv6 = format!("Host: [::1]:{port}");
        let upper = format!("Host: LOCALHOST:{port}");
        for host in [
            "Host: rebind.attacker.example",
            "Host: 127.0.0.1",
            "Host: localhost",
            "Host: ",
            wrong_port.as_str(),
            suffixed.as_str(),
            padded_user.as_str(),
            ipv6.as_str(),
            upper.as_str(),
        ] {
            for path in ["/", "/api/meta", "/data/stars.csv"] {
                let r = send(port, &format!("GET {path} HTTP/1.1\r\n{host}\r\n\r\n"));
                assert_refused(&r, host);
                assert!(!r.text().contains("ra,dec"), "{host} read a catalog");
            }
        }
    }

    #[test]
    fn a_missing_or_repeated_host_is_refused() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        let r = send(port, "GET / HTTP/1.1\r\n\r\n");
        assert_refused(&r, "no Host");
        let r = send(
            port,
            &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nHost: attacker.example\r\n\r\n"),
        );
        assert_refused(&r, "two Hosts");
        let r = send(
            port,
            &format!("GET / HTTP/1.1\r\nHost: attacker.example\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        );
        assert_refused(&r, "two Hosts, ours last");
    }

    #[test]
    fn a_foreign_origin_is_refused() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        let wrong_port = format!("Origin: http://127.0.0.1:{}", port.wrapping_add(1));
        let https = format!("Origin: https://127.0.0.1:{port}");
        // Host is 127.0.0.1, so localhost is a different origin
        let other_name = format!("Origin: http://localhost:{port}");
        for origin in [
            "Origin: http://attacker.example",
            "Origin: null",
            "Origin: ",
            wrong_port.as_str(),
            https.as_str(),
            other_name.as_str(),
        ] {
            for path in ["/", "/api/meta", "/data/stars.csv"] {
                let r = request(port, "GET", path, &[origin], "");
                assert_refused(&r, origin);
            }
        }
        let own = format!("Origin: http://127.0.0.1:{port}");
        let r = request(port, "GET", "/", &[own.as_str(), "Origin: null"], "");
        assert_refused(&r, "two Origins");
    }

    #[test]
    fn a_cross_site_fetch_is_refused() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        for site in ["cross-site", "same-site", "", "somewhere-else"] {
            let header = format!("Sec-Fetch-Site: {site}");
            for path in ["/", "/data/textures/earth.jpg", "/api/meta"] {
                let r = request(port, "GET", path, &[header.as_str()], "");
                assert_refused(&r, &header);
            }
        }
    }

    #[test]
    fn the_shot_hook_is_absent_without_qa_mode() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        let own = format!("Origin: http://127.0.0.1:{port}");
        for extra in [
            &[][..],
            &[own.as_str()][..],
            &["Origin: http://attacker.example"][..],
        ] {
            let r = request(port, "POST", "/api/shot?name=x", extra, "aGVsbG8=");
            assert_ne!(r.code, 200, "{extra:?}: {}", r.head);
        }
        assert_eq!(
            request(port, "POST", "/api/shot?name=x", &[], "aGVsbG8=").code,
            405
        );
        let written = std::fs::read_dir(fx.root.join("shots")).unwrap().count();
        assert_eq!(written, 0, "a shot was written without QA mode");
    }

    #[test]
    fn the_qa_hooks_are_absent_without_qa_mode() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        for path in ["/api/qa/eval", "/api/qa/log", "/api/qa/result?id=1"] {
            assert_eq!(request(port, "POST", path, &[], "1+1").code, 405, "{path}");
        }
        assert_eq!(get(port, "/api/qa/poll").code, 403);
    }

    #[test]
    fn qa_mode_needs_exactly_one() {
        use std::ffi::OsStr;
        assert!(qa_requested(Some(OsStr::new("1"))));
        for value in ["", "0", "2", "true", "yes", "1 ", " 1", "01"] {
            assert!(!qa_requested(Some(OsStr::new(value))), "{value:?}");
        }
        assert!(!qa_requested(None));
    }

    #[test]
    fn qa_mode_keeps_the_envelope_and_still_works_same_origin() {
        let fx = Fixture::new();
        let port = fx.serve(true);
        let own = format!("Origin: http://127.0.0.1:{port}");
        // the eval hook is what a cross-site page would want most
        for extra in [
            "Origin: http://attacker.example",
            "Sec-Fetch-Site: cross-site",
        ] {
            let r = request(port, "POST", "/api/qa/eval?timeout=0.1", &[extra], "1+1");
            assert_refused(&r, extra);
            let r = request(port, "POST", "/api/shot?name=x", &[extra], "aGVsbG8=");
            assert_refused(&r, extra);
        }
        let written = std::fs::read_dir(fx.root.join("shots")).unwrap().count();
        assert_eq!(written, 0, "a refused shot was written");
        let r = request(port, "POST", "/api/qa/log", &[own.as_str()], "from a test");
        assert_eq!(r.code, 200, "{}", r.head);
        let r = request(
            port,
            "POST",
            "/api/shot?name=ok",
            &[own.as_str()],
            "aGVsbG8=",
        );
        assert_eq!(r.code, 200, "{}", r.head);
        let shot = fx.root.join("shots/ok.jpg");
        assert_eq!(std::fs::read(&shot).unwrap(), b"hello");
        std::fs::remove_file(&shot).unwrap();
        // a QA-only CSP may allow eval; the normal one must not
        assert!(r.head.contains("unsafe-eval"));
        let plain = fx.serve(false);
        assert!(!get(plain, "/").head.contains("unsafe-eval"));
    }

    #[test]
    fn an_oversized_hook_body_is_refused_unread() {
        let fx = Fixture::new();
        let port = fx.serve(true);
        let r = send(
            port,
            &format!(
                "POST /api/qa/log HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\n\r\n",
                QA_BODY_MAX + 1
            ),
        );
        assert_eq!(r.code, 413, "{}", r.head);
    }

    #[test]
    fn an_endless_header_gets_no_reply_and_no_memory() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        let long = "x".repeat(2 * HEAD_MAX as usize);
        let r = send(port, &format!("GET / HTTP/1.1\r\nX-Long: {long}\r\n\r\n"));
        assert_eq!(r.code, 0, "{}", r.head);
        assert_eq!(get(port, "/js/a.js").code, 200);
    }

    #[test]
    fn a_large_file_arrives_whole() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        let path = fx.root.join("assets/data/textures/large.jpg");
        let sparse = std::fs::File::create(&path).unwrap();
        let len = 64u64 << 20;
        sparse.set_len(len).unwrap();
        drop(sparse);
        let r = get(port, "/data/textures/large.jpg");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(r.code, 200);
        assert!(r.head.contains(&format!("Content-Length: {len}")));
        assert_eq!(r.body.len() as u64, len);
    }

    #[test]
    fn the_probe_recognizes_a_running_pantheon() {
        let fx = Fixture::new();
        let port = fx.serve(false);
        assert!(existing_pantheon(port));
    }

    #[test]
    fn the_top_of_the_port_range_is_a_range_too() {
        assert_eq!((65535u16..=65535u16.saturating_add(9)).count(), 1);
        // and 65535 itself is either bound or reported as taken, never an empty search
        match bind_or_find(65535) {
            Ok(Bound::Listener(_, port)) => assert_eq!(port, 65535),
            Ok(Bound::Existing(port)) => assert_eq!(port, 65535),
            Err(e) => assert!(e.to_string().contains("no free port"), "{e}"),
        }
    }

    #[test]
    fn a_bound_port_is_never_zero() {
        let Bound::Listener(_listener, port) = bind_or_find(0).unwrap() else {
            panic!("port 0 can't already be in use")
        };
        assert_ne!(port, 0);
    }

    #[test]
    fn plain_names_are_exactly_what_they_look_like() {
        for good in ["index.html", "three.core.js", "8k_earth-map.jpg", "a..b.js"] {
            assert!(plain_name(good), "{good}");
        }
        for bad in [
            "", ".", "..", ".git", "a/b", "a\\b", "a%2eb", "C:", "a b", "a\0b", "é.js", "a?b",
            "a#b",
        ] {
            assert!(!plain_name(bad), "{bad:?}");
        }
    }
}
