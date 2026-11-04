//! Strict CLI: validated arguments, --help/--version, and `doctor`.

use std::path::PathBuf;

use crate::app;

const USAGE: &str = "\
pantheon — An Infinite Vector Atlas of the Living Universe

USAGE:
  pantheon                                  launch the interactive atlas
  pantheon --list-shots                     list preset scenes
  pantheon --shot <name|all> [--size WxH] [--out DIR]
  pantheon --path <file.json> [--fps N] [--size WxH] [--out DIR]
  pantheon --sweep [--size WxH] [--out DIR]  one frame per half decade, Earth to the cosmic web
  pantheon --bench [--size WxH] [--scenes a,b] frame cost per scene on this GPU, no window
  pantheon --serve [--port N]               run the Pantheon 3D backend (default port 8090);
                                            its window is the pantheon-3d binary (src-tauri)
  pantheon --doctor                         environment, GPU, and data health
  pantheon --help | --version

Sizes are WIDTHxHEIGHT pixels, both in 16..=8192 (even values required for
video-friendly frames). FPS must be in 1..=120.";

pub fn entry() {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        if let Err(e) = app::run_gui() {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }
    match run(&args) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
    }
}

fn parse_size(s: &str, require_even: bool) -> anyhow::Result<(u32, u32)> {
    let mut it = s.split(['x', 'X']);
    let (Some(ws), Some(hs), None) = (it.next(), it.next(), it.next()) else {
        anyhow::bail!("--size must be WIDTHxHEIGHT, got '{s}'");
    };
    let w: u32 = ws
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("bad width '{ws}'"))?;
    let h: u32 = hs
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("bad height '{hs}'"))?;
    anyhow::ensure!(
        (16..=8192).contains(&w) && (16..=8192).contains(&h),
        "size {w}x{h} outside 16..=8192"
    );
    if require_even {
        anyhow::ensure!(
            w % 2 == 0 && h % 2 == 0,
            "video frames need even dimensions, got {w}x{h}"
        );
    }
    Ok((w, h))
}

/// Extract `--flag value` pairs; error on unknown or valueless flags.
struct Flags {
    pairs: Vec<(String, String)>,
}

impl Flags {
    fn parse(args: &[String], allowed: &[&str]) -> anyhow::Result<Flags> {
        let mut pairs = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            anyhow::ensure!(a.starts_with("--"), "unexpected argument '{a}'");
            anyhow::ensure!(allowed.contains(&a.as_str()), "unknown flag '{a}'");
            let v = args
                .get(i + 1)
                .filter(|v| !v.starts_with("--"))
                .ok_or_else(|| anyhow::anyhow!("flag '{a}' needs a value"))?;
            anyhow::ensure!(!pairs.iter().any(|(k, _)| k == a), "duplicate flag '{a}'");
            pairs.push((a.clone(), v.clone()));
            i += 2;
        }
        Ok(Flags { pairs })
    }
    fn get(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

fn run(args: &[String]) -> anyhow::Result<()> {
    match args[0].as_str() {
        "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        "--version" | "-V" => {
            println!("pantheon {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "--list-shots" => {
            anyhow::ensure!(args.len() == 1, "--list-shots takes no arguments");
            for s in app::SHOTS {
                println!("{:16} {}", s.name, s.desc);
            }
            Ok(())
        }
        "--doctor" => doctor(),
        "--serve" => {
            let flags = Flags::parse(&args[1..], &["--port"])?;
            let port: u16 = match flags.get("--port") {
                Some(p) => p.parse().map_err(|_| anyhow::anyhow!("bad port '{p}'"))?,
                None => 8090,
            };
            let r = crate::serve::serve(port);
            if let Err(e) = &r {
                // launched from a double-clicked shortcut, the console vanishes
                // with the process — keep the error on screen long enough to read
                eprintln!("error: {e:#}");
                eprintln!("(window closes in 15 s)");
                std::thread::sleep(std::time::Duration::from_secs(15));
                std::process::exit(1);
            }
            r
        }
        "--shot" => {
            let which = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .map(String::as_str)
                .unwrap_or("all");
            let skip = if args.get(1).map(|a| a.starts_with("--")).unwrap_or(true) {
                1
            } else {
                2
            };
            let flags = Flags::parse(&args[skip..], &["--size", "--out"])?;
            let (w, h) = match flags.get("--size") {
                Some(s) => parse_size(s, false)?,
                None => (1600, 950),
            };
            let out = PathBuf::from(flags.get("--out").unwrap_or("exports/shots"));
            app::run_shots(which, w, h, &out)
        }
        "--sweep" => {
            let flags = Flags::parse(&args[1..], &["--size", "--out"])?;
            let (w, h) = match flags.get("--size") {
                Some(s) => parse_size(s, false)?,
                None => (1280, 720),
            };
            let out = PathBuf::from(flags.get("--out").unwrap_or("exports/sweep"));
            app::run_sweep(w, h, &out)
        }
        "--bench" => {
            let flags = Flags::parse(&args[1..], &["--size", "--scenes"])?;
            let (w, h) = match flags.get("--size") {
                Some(s) => parse_size(s, false)?,
                None => (2560, 1440),
            };
            app::run_bench(w, h, flags.get("--scenes").unwrap_or("all"))
        }
        "--path" => {
            let file = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .ok_or_else(|| anyhow::anyhow!("--path needs a keyframe file"))?;
            let file = PathBuf::from(file);
            anyhow::ensure!(file.exists(), "path file not found: {}", file.display());
            let flags = Flags::parse(&args[2..], &["--size", "--out", "--fps"])?;
            let (w, h) = match flags.get("--size") {
                Some(s) => parse_size(s, true)?,
                None => (1280, 760),
            };
            let fps: u32 = match flags.get("--fps") {
                Some(s) => s.parse().map_err(|_| anyhow::anyhow!("bad fps '{s}'"))?,
                None => 30,
            };
            anyhow::ensure!((1..=120).contains(&fps), "fps must be 1..=120, got {fps}");
            let out = PathBuf::from(flags.get("--out").unwrap_or("exports/frames"));
            app::run_path(&file, w, h, fps, &out)
        }
        other => anyhow::bail!("unknown command '{other}'\n\n{USAGE}"),
    }
}

fn doctor() -> anyhow::Result<()> {
    println!("pantheon {} — doctor", env!("CARGO_PKG_VERSION"));
    // GPU
    match crate::engine::gpu::adapter_info() {
        Ok(info) => println!("gpu:      {info}"),
        Err(e) => println!("gpu:      UNAVAILABLE — {e}"),
    }
    // SPICE kernels
    match crate::spice::spice() {
        Some(s) => {
            let (y0, y1) = (
                2000.0 + (s.jd0 - 2_451_545.0) / 365.25,
                2000.0 + (s.jd1 - 2_451_545.0) / 365.25,
            );
            println!(
                "spice:    OK — {} · planetary {y0:.0}–{y1:.0} · leap seconds {}",
                s.kernel_summary(),
                s.lsk
                    .as_ref()
                    .map(|l| l.entries.len().to_string())
                    .unwrap_or("MISSING".into())
            );
        }
        None => println!("spice:    not loaded — analytic elements only (optional)"),
    }
    for problem in crate::spice::problems() {
        println!("  {problem} tools/fetch_data.py replaces it.");
    }
    // layer registry
    println!(
        "layers:   {} registered — {}",
        crate::atlas::LAYERS.len(),
        crate::atlas::LAYERS
            .iter()
            .map(|l| l.name)
            .collect::<Vec<_>>()
            .join(", ")
    );
    // ffmpeg
    let ff = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output();
    match ff {
        Ok(o) if o.status.success() => {
            let first = String::from_utf8_lossy(&o.stdout);
            println!("ffmpeg:   {}", first.lines().next().unwrap_or("present"));
        }
        _ => println!("ffmpeg:   not found (video assembly unavailable)"),
    }
    // data health. The large downloads are optional: the atlas runs without
    // them and says so. Only the catalogs that ship in the repo are required.
    let (u, health) = crate::data::Universe::load_with_health();
    let mut bad = 0;
    let mut optional = 0;
    println!("assets:   root = {}", crate::data::asset_root().display());
    for line in &health.lines {
        println!("  {line}");
        if line.contains("MISSING") || line.contains("INVALID") {
            if line.starts_with("craft ") || line.starts_with("truth ") || line.starts_with("land/")
            {
                optional += 1;
            } else {
                bad += 1;
            }
        }
    }
    let tex = crate::data::asset_root().join("data/textures");
    let maps = crate::engine::textures::ALL_TEX
        .iter()
        .filter(|t| tex.join(t.file()).is_file())
        .count();
    println!(
        "imagery:  {maps} of {} surface maps on disk{}",
        crate::engine::textures::ALL_TEX.len(),
        if maps == 0 {
            " (bodies will be procedural)"
        } else {
            ""
        }
    );
    if optional > 0
        || maps == 0
        || crate::spice::spice().is_none()
        || !crate::spice::problems().is_empty()
    {
        println!("optional data is missing or damaged; run tools/fetch_data.py to download it");
    }
    println!(
        "loaded:   {} stars, {} exo systems, {} craft, coast {} lines",
        u.stars.len(),
        u.exos.len(),
        u.craft.len(),
        u.coast.len(),
    );
    for (dataset, date) in &u.retrieved {
        println!("retrieved: {dataset} {date}");
    }
    let json = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "lines": health.lines,
        "stars": u.stars.len(),
        "exo_systems": u.exos.len(),
        "craft": u.craft.len(),
    });
    std::fs::create_dir_all("exports").ok();
    std::fs::write(
        "exports/DATA_HEALTH.json",
        serde_json::to_string_pretty(&json)?,
    )?;
    println!("wrote exports/DATA_HEALTH.json");
    anyhow::ensure!(bad == 0, "{bad} required dataset problem(s) — see above");
    Ok(())
}
