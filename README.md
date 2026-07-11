# PANTHEON
### An Infinite Vector Atlas of the Living Universe

A GPU-first desktop atlas that renders the universe as one continuous, infinitely
zoomable vector cosmogram — from the coastlines of Earth to the cosmic web — built
on real astronomical data with per-object provenance.

Zoom out from Earth and the view becomes an orbital-mechanics engraving; further,
a data-backed Gaia star atlas; further, the luminous fog of the Milky Way; further,
the Local Group and the filamentary cosmic web. Zoom *into* any gold portal ring in
the star field and a real exoplanet system unfolds — worlds within worlds. All of it
is one camera, one continuous scale, no cuts.

## Running

```
pantheon                          launch the interactive atlas
pantheon --list-shots             list the preset scenes
pantheon --shot all               render every preset headlessly to exports/shots
pantheon --shot saturn --size 1920x1080 --out exports/shots
pantheon --path exports/paths/grand_tour.json --fps 30 --out exports/grand_tour
```

The `--path` renderer plays a cinematic keyframe file (same format the in-app
Capture panel saves) and writes numbered PNG frames plus an ffmpeg one-liner to
assemble a video.

## Controls

| input | action |
|---|---|
| mouse wheel | infinite zoom, anchored at the cursor |
| left drag | pan |
| right drag (vertical) | tilt the atlas plane |
| click | select object (opens the inspector card) |
| double-click | dive to the object |
| `L` | Living Orbits (time animation) |
| `T` | Truth mode (ink by evidence) |
| `G` | Gravity view (Lagrange points, Hill spheres, potential contours) |
| `Y` | System View (log-spaced schematic) |
| `H` | home (Earth) |
| `Esc` | release focus |

## The modes

- **Free Infinite Zoom** — the default state; ~19 orders of magnitude of scale.
- **Scale Ladder** — the log₁₀ rail on the right; click a rung to fly there.
- **Object Awakening** — "awaken" on a planet card spins up a local N-body
  island (Sun + planet + moons, leapfrog integrator) that visibly drifts from
  the Keplerian engraving while the rest of the universe stays analytic.
- **Living Orbits / Time Machine** — play or scrub time (1600–2500); planetary
  elements are flagged as extrapolated outside 1800–2050; stars drift by real
  Gaia proper motions at high time rates.
- **Ephemeris Trail** — measured JPL Horizons arcs drawn dashed-blue against
  the modeled gold orbits; every planet card shows the live Kepler-vs-JPL
  residual in km.
- **Gravity View** — CR3BP effective-potential contours engraved on the canvas,
  L1–L5, Hill spheres, fluid Roche limits for the focused pair.
- **Truth Mode** — every stroke re-inked by evidence class: measured / modeled /
  inferred / procedural / illustrative / unknown, with retrieval dates.
- **System View** — logarithmic orbit spacing for compact resonant systems.
- **Cinematic Capture** — record keyframes in-app, save as JSON, replay live or
  render headless.

## Data & provenance

| dataset | source | status |
|---|---|---|
| 12,000 nearest stars + naked-eye sky (G<6) | ESA Gaia DR3, Archive TAP | measured |
| 6,300 confirmed exoplanets (default params) | NASA Exoplanet Archive `ps` | measured |
| planetary orbits | JPL approximate elements (Standish), 1800–2050 | modeled |
| planet/Moon ground truth, residuals | JPL Horizons vectors, cached | measured |
| spacecraft trails (Voyagers, Pioneers, New Horizons, Parker, Cassini) | JPL Horizons | measured |
| Earth coastlines | Natural Earth 1:110m | measured |
| moons | JPL mean elements (circular-inclined approx.) | modeled |
| Milky Way fog, cosmic filaments | procedural | procedural |
| named star / Local Group / attractor tables | compiled catalogs | measured |

Raw query results live in `assets/data/` with `provenance.json` recording source,
query, frame, and retrieval date (2026-07-11). Re-run `tools/fetch_data.ps1` to
refresh.

## Architecture (src/)

- `frames.rs` — f64 transform ladder; one orthographic camera whose view basis
  slerps ecliptic → galactic → supergalactic as scale crosses bands.
- `engine/` — wgpu 25 renderer: four pipelines (fullscreen band shader, AA glow
  strokes, instanced star sprites, instanced procedural-surface discs), MSAA,
  premultiplied luminous-ink blending; identical path drives the window and the
  headless PNG exporter (full egui UI included in exports).
- `sim.rs` — JD clock, Kepler propagation, precessing lunar model, N-body
  leapfrog islands, Lagrange/Hill/Roche mechanics.
- `data.rs` — dataset parsers + the provenance model.
- `atlas.rs` — the scale-band layer compositor (the heart).
- `app.rs` — shared GUI/headless shell, cards, ladder, capture.

## Building (this machine)

```
$env:Path = "$env:USERPROFILE\.cargo\bin;<llvm-mingw>\bin;$env:Path"
$env:RUSTFLAGS = "-C target-feature=+crt-static"
cargo +stable-x86_64-pc-windows-gnullvm build --release
```
