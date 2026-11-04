# KNOWN_ISSUES — validated, prioritized remainder of the audit backlog

Each entry: audit ID(s) · severity · state. These are real, acknowledged, and
sequenced; nothing here is hidden behind product claims (README is written to
current behavior).

## Architecture (audit Phase 5–6)
- **ARCH-001/002/006** [High] `ObjectRef` is a closed enum; no general frame
  graph / entity registry / capability model. Current special cases work but do
  not compose (binary stars, barycenters, surface frames, instruments).
- **FND-006/007** [High] One global f64 coordinate space: local detail at
  arbitrary cosmic origins needs per-frame local origins + camera rebasing.
  (Exo pockets now have world positions but still live in the root frame.)
- **ARCH-005** [Med] `focus` still doubles as gravity-pair selector and
  awaken target (selection vs scene now split via `scene_exo`; the rest
  remains).

## Science (audit Phase 2–3, 7)
- **SCI-041** [Med] Without the kernels (a fresh clone), planetary states fall back
  to Standish elements (about 1e4 km inner, 2e6 km Saturn). The app says so.
- **SCI-024** [Low] IAU rotation omits small periodic terms; moons other than ours
  use a mean tidal lock with no libration.
- **SCI-027** [High] Non-lunar moon planes = parent equator approximation;
  Triton's true inclined retrograde plane not modeled.
- **SIM-002/004** [Med] Island initialization does not zero total momentum;
  no energy/momentum diagnostics; step policy heuristic (GMs now correct).
- **SCI-040** [Med] Gravity contour field is evaluated in the screen plane —
  exact only face-on (L-point markers ARE 3D-correct now).
- **SCI-004** [Med] Cosmic distances are Euclidean heliocentric; no cosmology.
  Labeled illustrative in truth mode, kept.
- **SCI-011** [Med] Proper motion is linear; no validity model at kyr/Myr rates.

## Provenance & data (audit Phase 4)
- **FND-008/PROV** [High] Evidence is still an enum applied at call sites, not
  field-level records with uncertainty/citation/lineage. Cards hardcode source
  prose. (Parallax σ now kept for star distance — first uncertainty display.)
- **DATA-003** [Med] Handwritten CSV parser (quoted-comma aware, BOM-blind for
  non-provenance files); `csv` crate migration pending.
- **DATA-007/037** [Med] No raw/normalized/derived stages, hashes, or release
  manifests; fetch script is not atomic-promote.
- **DATA-012/013** [Med] Named-star crossmatch is a 0.57° cone; synthesized
  rows mix V/G photometry.

## Rendering (audit Phase 8)
- **REND-008** [Low] Full buffer rebuild per frame (fine at current loads).
- **REND-034** [Low] Moons don't cast shadows on their planets, and rings don't
  shadow moons. Saturn's rings do shadow Saturn and Saturn shadows its rings.
- **REND-035** [Low] Below about 5 km per pixel a surface map runs out of
  resolution and goes soft. There is no streaming or finer data.
- **REND-036** [Med] Bright stars sit on the sky by true direction, through a
  nominal lens (the atlas camera is orthographic). Directions are right, angular
  scale is not meaningful.

## UX / capture (audit Phase 11)
- **UI-019/PATH-001** [Med] Path schema unversioned, partial state snapshot.
- **UI-031** [Low] `T`, `G`, `Y` and other single-key shortcuts assume a Latin
  layout, and `?` is read from text input so it does work across layouts.

## Fixed in the Linux pass (2026-11-04)
See UX_AUDIT.md for the field-test list. Closed here: FND-009 (depth + HDR were
already in; the entry was stale), REND-003 (HDR, AgX), FND-015 (canvas in physical
pixels), REND-005/006 (adaptive tessellation), UI-008 (search), UI-011 (layout is
computed from the window size and tested not to collide), and the SPICE and IAU
entries above, which predate the kernels and orientation work: planet states use
DE440s when kernels are present and IAU 2015 rotation elements always.

## Fixed in the follow-up session (2026-07-11)
- UI-007 help overlay; UI-009 case-insensitive lookup; UI-020 path time/tilt glide;
  UI-026 timestamped capture saves w/ error surfacing; SIM-002/004 island momentum
  zeroing + energy-drift diagnostic (card row, ppm); DATA-012 crossmatch tightened
  (0.11 deg, distance + magnitude gates); REND-006 partial (4 px chords, 1024-seg cap);
  REND-024 partial (char-count label widths). Plus: window/taskbar/shortcut icon
  (assets/icon.ico via tools/make_icon.ps1) and a dependency-free generative
  ambience engine (src/audio.rs, winmm waveOut; GUI-only, rail toggle + volume).
