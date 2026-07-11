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
- **SPICE** [High] No SPICE kernels; planetary states are Standish elements
  (±~1e4 km inner, ±~2e6 km Saturn — displayed honestly). DE44x + LSK is the
  planned foundation.
- **SCI-024/025/026** [High] Body orientations are node-line obliquity tilts,
  not IAU pole/prime-meridian models; Earth longitude uses an approximate
  alignment constant. Surface orientation is decorative at close zoom.
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
- **FND-009** [High] Still no depth buffer/HDR: disc sort + ring split are
  painter-order mitigations. Points (stars/markers) still draw over discs.
- **REND-003** [Med] LDR sRGB target; bulge handled by shader tonemap only.
- **FND-015** [Med] GUI canvas sized in logical points (blurry at ppp>1;
  this machine runs ppp=1). Headless is exact.
- **REND-005/006** [Med] No joins/caps; fixed-count tessellation (256-segment
  circles can facet at extreme radii).
- **REND-008** [Low] Full buffer rebuild per frame (fine at current loads).

## UX / capture (audit Phase 11)
- **UI-008** [Med] No search/command palette.
- **UI-020** [Med] Path keys jump time/tilt rather than interpolating.
- **UI-019/PATH-001** [Med] Path schema unversioned, partial state snapshot.
- **UI-011** [Low] Fixed panel anchors can collide at small window sizes.
