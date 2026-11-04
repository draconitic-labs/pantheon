# Pantheon

An atlas of the universe as one continuous map. You start at Earth, where it is
right now, and zoom out without a single cut: past the Moon's orbit, the planets,
the edge of the solar wind, the nearest stars, the Milky Way, the Local Group, to
the cosmic web. That is about 19 orders of magnitude on one camera.

It's a native Rust application (eframe, egui, wgpu). It uses real data wherever
real data exists, and says so on screen when it doesn't.

![Earth, with the real sky behind it](docs/screenshots/earth.jpg)

| | |
|---|---|
| ![Saturn](docs/screenshots/saturn.jpg) | ![The Sun](docs/screenshots/sun.jpg) |
| ![The nearest stars](docs/screenshots/stars.jpg) | ![The Milky Way](docs/screenshots/milkyway.jpg) |
| ![The Local Group](docs/screenshots/localgroup.jpg) | ![The cosmic web](docs/screenshots/cosmic.jpg) |

## Run it on Linux

You need Rust ([rustup](https://rustup.rs) installs the pinned toolchain by
itself), a Vulkan driver, and these system packages:

```sh
# Fedora
sudo dnf install alsa-lib-devel libxkbcommon-devel wayland-devel libX11-devel

# Debian, Ubuntu
sudo apt install libasound2-dev libxkbcommon-dev libwayland-dev libx11-dev
```

Then:

```sh
cargo build --release --bin pantheon --bin pantheon-cli
./target/release/pantheon
```

That already works on a fresh clone. The star, exoplanet and coastline catalogs
ship in the repository. Without the large downloads, planets have procedural
surfaces, positions come from approximate orbital elements, and spacecraft are
absent. The app tells you when that's the case, in its view menu.

To add the rest:

```sh
tools/fetch_data.py               # everything: about 3 GB, mostly JPL kernels
tools/fetch_data.py --no-kernels  # planet maps and spacecraft tracks: about 60 MB
tools/fetch_data.py --list        # see what's here and what isn't
```

Python 3.9 or newer and nothing else is required. `pantheon-cli --doctor` checks
the result.

To add Pantheon to your application menu, under `~/.local` only:

```sh
scripts/install-desktop.sh              # build, then add menu entries and icons
scripts/install-desktop.sh --uninstall  # take them away again
```

No sound is built in if you'd rather skip ALSA: build with
`--no-default-features`. With sound compiled in, ambience stays off until you turn
it on.

## Moving around

| Input | What it does |
|---|---|
| Scroll, or pinch | Zoom at the pointer. Hold Shift for fine control |
| Drag | Move. A flick keeps drifting |
| Right-drag, or Shift-drag | Tilt and turn the view |
| Click | Select, and open the card. Names are clickable too |
| Double-click | Fly there. On empty space, zoom in |
| `/` or `Ctrl K` | Search for anything, then arrow keys and Enter |
| The scale on the right | Click a name to go there, drag to scrub through all of it |
| `H` | Fly home to Earth |
| `Backspace` | Back to the previous view |
| `+` `-`, arrows or `W A S D` | Zoom and move from the keyboard |
| `F` | Fly to the selection |
| `0` | Level the view |
| `Space` | Run or pause time |
| `[` `]` | Slower, faster |
| `N` | Back to now |
| `T` | Colour everything by evidence |
| `G` | Gravity field of the selected body |
| `Y` | Schematic spacing for exoplanet systems |
| `M` | Ambience |
| `Esc` | Close, or clear the selection |
| `?` | All of this, in the app |

The top left always says where you are, how wide the view is, how long light takes
to cross it, and what the picture is made of. When something you selected, or the
Sun, is off screen, an arrow at the edge points to it. Click the arrow to go there.

## What's real and what isn't

The project labels evidence, and the labels are on screen: the line under the
place name states what the current view is made of. Pressing `T` re-inks every
stroke by evidence class, and the legend lists only what's actually drawn. The
card's "Where this comes from" gives source, citation and frame for each value.

| You see | It is | Class |
|---|---|---|
| Planet and Moon positions | JPL DE440s and satellite kernels, with Standish elements outside coverage or without kernels | measured / modeled |
| Spacecraft paths | JPL Horizons vectors, Hermite-interpolated, never extrapolated | measured |
| Nearest 12,000 stars and the naked-eye sky | ESA Gaia DR3 | measured |
| Exoplanet periods, sizes and hosts | NASA Exoplanet Archive | measured |
| Orientation of exoplanet orbits, their phases | A convention. Most systems have no known orientation | illustrative |
| Body orientation | IAU 2015 rotation elements, small terms omitted | modeled |
| Planet, Moon and Sun surfaces | Solar System Scope maps, based on NASA data. The publisher tunes the colour and fills unmapped gaps with fictional terrain | inferred |
| Clouds | One snapshot in the map set, not today's weather | inferred |
| Moons other than ours, and Pluto | Procedural: no map | procedural |
| Rings | Saturn's radial profile from a map, Uranus's from embedded zones. Shadows use the real sun direction | modeled |
| The sky behind the solar system | ESO's all-sky photograph, correct in direction, seen through a nominal lens because the atlas camera has no true field of view | measured |
| Atmosphere glow, the Sun's corona and motion | Shading choices, exaggerated to be visible | illustrative |
| Relief on rocky bodies | Derived from image brightness, not elevation | illustrative |
| Asteroid, Kuiper and Oort samples, the heliosphere's shape | Show where these are, not the rocks themselves. The Kirkwood gaps are in the right places | illustrative |
| The Milky Way | An illustration. Arms are stylized to pass near their real distances along the Sun's azimuth | procedural |
| Stars beyond 83 pc, distant galaxies | Illustrative dust, kept out of the sphere where this atlas has real Gaia data. A ring marks the edge | procedural |
| Local Group and cluster positions | Compiled literature values, approximate sizes | measured |
| Their appearance, halos, the cosmic web's filaments | Procedural, not a survey map | illustrative |

Time is TDB internally and UTC on display. See `AUDIT_STATUS.md` and
`KNOWN_ISSUES.md` for the verified state and the honest remainder.

## Graphics

An HDR canvas (16-bit float, 4x MSAA) with a real depth buffer, a mip-chain bloom,
and an AgX tonemap. It renders in physical pixels at any display scale. Orbit lines
are tessellated adaptively in 64-bit and clipped before they become 32-bit, so they
stay smooth at any zoom. Surface maps decode on worker threads and upload in slices
so nothing stalls a frame. Measured headless on an RTX 4060 at 2560x1440, a full frame costs about
1.5 to 5.5 ms depending on the scene. `pantheon --bench` measures your machine.

## Command line

```
pantheon                                 the interactive atlas
pantheon --list-shots                    preset scenes
pantheon --shot NAME[,NAME] [--size WxH] [--out DIR]    render without a window
pantheon --sweep [--size WxH] [--out DIR]   one frame per half decade, Earth to the web
pantheon --bench [--size WxH] [--scenes a,b]   frame cost on this GPU
pantheon --path FILE.json [--fps N] [--size WxH] [--out DIR]   render a recorded flight
pantheon --doctor                        GPU, kernels, datasets
pantheon-cli ...                         the same, always with console output
```

Record a flight from the view menu, save it, and render it with `--path`. Set
`PANTHEON_ASSETS` to use a data directory elsewhere, and `PANTHEON_TEX_MAX` to cap
map resolution on a small GPU.

## Pantheon 3D

A second edition with true 3D free flight, in a native Tauri window. It lives in
`src-tauri/` and `pantheon-web/`.

```sh
sudo dnf install webkit2gtk4.1-devel        # Debian, Ubuntu: libwebkit2gtk-4.1-dev
cd src-tauri && CARGO_TARGET_DIR=../target-3d cargo build --release
../target-3d/release/pantheon-3d
```

`scripts/install-desktop.sh` builds it too and adds its own menu entry.

## Layout of the code

- `src/frames.rs`: coordinate frames, the camera, the zoom-and-pan flight path.
- `src/atlas.rs`: every scale band, composed into one frame.
- `src/engine/`: the wgpu renderer and the surface map bank. `shaders/` holds WGSL.
- `src/app.rs`, `src/app/`: state, input, the UI layout, search, settings.
- `src/sim.rs`: time, Kepler propagation, N-body islands, gravity mechanics.
- `src/spice.rs`: a dependency-free reader for NAIF kernels.
- `src/data.rs`, `src/evidence.rs`: datasets, and the record behind each evidence label.
- `tools/`: dataset and icon scripts. `scripts/`: the desktop installer.

`cargo test --lib --bins` runs the checks, including golden frame and time tests
and a test that panels never collide at any window size.

## License

The source code is under the MIT license (see `LICENSE`). Datasets, imagery and
fonts keep their own terms, listed in `NOTICE`. Gaia data, which ships in this
repository, is CC BY-NC 3.0 IGO: non-commercial.
