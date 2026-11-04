# Known issues

What is true of Pantheon today and isn't what you might assume. Nothing here stops the
apps running. The on-screen labels already say most of this where it applies.

## Positions and science

- **Fresh clone.** Without the kernels, planets use JPL's approximate orbital elements, and
  moons use mean elements. The app says so.
- **Our Moon without a kernel.** A precessing ellipse with no periodic terms. From 1950 to
  2050 its direction is about 1° from DE440s on average and 3.2° at worst, which is up to about
  20,000 km, or a few hours of its motion, so a full or new moon can look hours early or late.
  Outside that it is worse: about 2° on average and 4.6° at worst by 1850 and 2150, because its
  rates are of-date and drift about 1.4° a century.
- **The moons of Uranus.** `tools/fetch_data.py` downloads no Uranian kernel, so Miranda,
  Ariel, Umbriel, Titania and Oberon always use mean elements, and where each sits along its
  orbit is chosen for legibility rather than real. Their cards say this.
- **Moons without a kernel.** The orbit lies in a plane tilted a fixed amount from the planet's
  equator, and its node doesn't precess. Moons other than ours are tidally locked with no
  libration.
- **Rotation.** IAU rotation elements, with the small periodic terms left out.
- **Gravity view.** Contours are computed in the screen plane, so they are exact only when
  the system is seen face-on. The Lagrange point markers are correct in 3D.
- **Cosmic distances.** Straight lines from the Sun in a flat space. There is no cosmology.
- **Star motion.** Proper motion is linear, with no perspective acceleration or binary motion,
  and it stops being meaningful over thousands of years.
- **N-body islands.** The substep is a fixed fraction of the shortest moon period, not an
  adaptive scheme. When time runs too fast for it, the simulation ends and says so.

## Data

- **Evidence is per source, not per value.** Only star distances carry an uncertainty.
- **Named stars.** The crossmatch to Gaia uses a 0.11° cone with distance and magnitude gates.
  Stars it can't match come from an embedded table with V magnitudes, so the star field mixes
  V and G photometry.
- **Downloads.** Kernels and maps are checked against their exact size. Catalogs and Horizons
  tables change with each fetch, so they are checked by what the server promised and by their
  markers, and a catalog truncated some other way wouldn't be noticed on a later run. There are
  no checksums, so a kernel of the right size with a damaged body looks fine to the script. The
  apps catch that one when they load it and say to delete it and fetch again. Natural Earth
  comes from an unpinned `master` branch.
- **Spacecraft tracks end on 2027-07-01** (Cassini in 2017). Past the end of a track, the atlas
  hides that spacecraft (Cassini stays as a dim marker), and in 3D its marker holds at the end
  and the card says so. Newer dates
  need an edit to `tools/fetch_data.py`.
- **Catalog parser.** A small handwritten CSV reader. It handles quoted commas, but not a
  byte-order mark, and the catalogs it reads have none.

## The atlas

- **Scene model.** Positions run through a frame graph, but the kinds of object are a closed
  enum (`ObjectRef`), so binary stars, barycenters and surface frames aren't expressible.
- **Precision.** Outside an exoplanet system, everything lives in one heliocentric 64-bit space,
  so fine local detail at cosmic distances isn't possible.
- **Selection.** `focus` also picks the pair for the gravity view and the target for a simulation.
- **Shadows.** Saturn's rings shadow Saturn and Saturn shadows its rings. Moons don't cast
  shadows on planets, and rings don't shadow moons.
- **Map resolution.** Below about 5 km per pixel on an Earth-sized body, a surface map goes soft.
  There is no streaming or finer data.
- **The sky.** Bright stars and the sky photograph are correct in direction, but the camera is
  orthographic, so their angular scale is nominal.
- **Frame cost.** Vertex buffers are rebuilt every frame, which is fine at current loads.
- **Recorded paths.** The file format has no version number, and a key stores only part of the
  app's state.
- **Keyboard layouts.** `T`, `G`, `Y` and other single-key shortcuts assume a Latin layout. `?`
  is read from text input, so it works on any layout.

## Pantheon 3D

- **Tested on one setup.** Linux, WebKitGTK 2.54, RTX 4060. Other drivers keep WebKit's
  default frame handover, and that path hasn't been tested.
- **Frame rate.** At 1600×950 on that RTX 4060, on a busy machine, the window held 55-62 fps
  at High on both Wayland and XWayland, using the shared-memory frame handover the app selects on
  NVIDIA. With `WEBKIT_DISABLE_DMABUF_RENDERER=1`, Wayland ran at 40-48 fps. Larger windows have
  not been measured.
- **Frame readout.** WebKitGTK has no GPU timer and masks the adapter name. The render figure
  is timed through `glFinish` and barely changed between quality tiers when measured, so treat
  it as rough. The frame interval is the reliable number.
- **Automatic quality** can step down under heavy load from other programs although the GPU has
  headroom, because it can't tell a busy machine from a slow GPU.
- **First approach to an 8K map** uploads over about 16 frames. The largest single frame measured
  was about 80 ms, so a small hitch remains during that flight.
- **Procedural looks.** All Local Group spirals share one illustrative structure, oriented from
  the literature. The Sun's own map doesn't rotate, and star surfaces, corona and prominences are
  generic. Cloud shapes are one saved snapshot, and their drift is illustrative.
- **Moving ports.** If port 8090 is taken, the window takes the next free one, and WebKit treats
  that as a new site, so the first-run hint returns and the quality setting resets.
- **Spacecraft in 3D** move linearly between Horizons samples. The atlas uses Hermite
  interpolation.

## Windows

Pantheon was first built on Windows, and the code paths remain (kernel memory mapping,
WebView2 through Tauri), but the current tree is developed and tested only on Linux. The atlas
passes `cargo check --target x86_64-pc-windows-gnullvm`, which only shows that it type-checks.
Nobody has linked or run this version on Windows, and the 3D window hasn't even been
type-checked for it, because its build script needs a Windows resource compiler.
