#!/usr/bin/env python3
"""Download Pantheon's datasets into assets/data, standard library only.

A fresh clone already runs: the star, exoplanet and coastline catalogs ship in
the repository. This adds what's too large or too fast-changing to keep in git:
spacecraft and planet tracks, JPL kernels (about 3 GB), and surface imagery
(about 60 MB).

In a checkout or an unpacked release, files go to assets/data beside tools/. An
installed package keeps this script in /usr/share/pantheon/tools, and files go to
~/.local/share/pantheon/assets/data, where the apps look first
($XDG_DATA_HOME/pantheon/assets/data if that's set).

  tools/fetch_data.py                everything still missing
  tools/fetch_data.py --list         what would be fetched, with sizes
  tools/fetch_data.py --only maps    one group: catalogs, tracks, kernels, maps
  tools/fetch_data.py --no-kernels   skip the large kernels
  tools/fetch_data.py --force        fetch again even if present

Every file lands under a temporary name and is renamed only when it's complete:
all the bytes the server promised arrived, it's the size this script expects where
that's known, and it starts the way that kind of file should. A later run checks
what's already here the same way, so running this again replaces a truncated file.
"""

from __future__ import annotations

import argparse
import contextlib
import datetime as dt
import http.client
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

GAIA_TAP = "https://gea.esac.esa.int/tap-server/tap/sync"
EXO_TAP = "https://exoplanetarchive.ipac.caltech.edu/TAP/sync"
NE = "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson"
HORIZONS = "https://ssd.jpl.nasa.gov/api/horizons.api"
NAIF = "https://naif.jpl.nasa.gov/pub/naif/generic_kernels"
SSS = "https://www.solarsystemscope.com/textures/download"
ESO_PANORAMA = "https://cdn.eso.org/images/large/eso0932a.jpg"

GAIA_NEAR = (
    "SELECT TOP 12000 source_id,ra,dec,parallax,parallax_error,pmra,pmdec,radial_velocity,"
    "phot_g_mean_mag,bp_rp FROM gaiadr3.gaia_source WHERE parallax > 12 AND "
    "parallax_over_error > 10 AND phot_g_mean_mag < 16 ORDER BY parallax DESC"
)
GAIA_BRIGHT = (
    "SELECT source_id,ra,dec,parallax,pmra,pmdec,phot_g_mean_mag,bp_rp "
    "FROM gaiadr3.gaia_source WHERE phot_g_mean_mag < 6.0"
)
EXOPLANETS = (
    "select pl_name,hostname,sy_snum,sy_pnum,discoverymethod,disc_year,pl_orbper,pl_orbsmax,"
    "pl_orbeccen,pl_orbincl,pl_rade,pl_bmasse,pl_eqt,st_teff,st_rad,st_mass,st_spectype,ra,dec,"
    "sy_dist,sy_vmag from ps where default_flag=1"
)

# Exact sizes of the files that never change upstream. Catalogs and Horizons replies
# differ with each fetch, so those are checked by their end marker or by what the
# server promised instead.
SIZES = {
    "spice/naif0012.tls": 5_257,
    "spice/de440s.bsp": 32_726_016,
    "spice/mar099.bsp": 1_227_574_272,
    "spice/jup365.bsp": 1_136_581_632,
    "spice/sat441.bsp": 661_592_064,
    "spice/nep097.bsp": 105_262_080,
    "spice/plu060.bsp": 135_207_936,
    "textures/8k_earth_daymap.jpg": 4_565_076,
    "textures/8k_earth_nightmap.jpg": 3_144_468,
    "textures/2k_earth_clouds.jpg": 965_676,
    "textures/8k_moon.jpg": 15_030_356,
    "textures/8k_mars.jpg": 8_396_951,
    "textures/8k_jupiter.jpg": 3_080_980,
    "textures/8k_saturn.jpg": 1_098_204,
    "textures/8k_saturn_ring_alpha.png": 64_790,
    "textures/8k_sun.jpg": 3_696_361,
    "textures/4k_venus_atmosphere.jpg": 1_546_146,
    "textures/8k_mercury.jpg": 15_035_740,
    "textures/2k_uranus.jpg": 77_751,
    "textures/2k_neptune.jpg": 241_580,
    "textures/eso_milkyway.jpg": 8_228_817,
}

JPEG = b"\xff\xd8\xff"
PNG = b"\x89PNG"
KERNEL = b"DAF/SPK "


def tap(base: str, **params: str) -> str:
    return base + "?" + urllib.parse.urlencode(params)


def horizons(command: str, start: str, stop: str, step: str, center: str = "500@10") -> str:
    return tap(
        HORIZONS,
        format="text",
        COMMAND=f"'{command}'",
        OBJ_DATA="'NO'",
        MAKE_EPHEM="'YES'",
        EPHEM_TYPE="VECTORS",
        CENTER=f"'{center}'",
        START_TIME=f"'{start}'",
        STOP_TIME=f"'{stop}'",
        STEP_SIZE=f"'{step}'",
        VEC_TABLE="'2'",
        REF_PLANE="'ECLIPTIC'",
        REF_SYSTEM="'J2000'",
        OUT_UNITS="'AU-D'",
        CSV_FORMAT="'YES'",
    )


# (group, path under assets/data, url, approximate MB, must contain)
def manifest() -> list[tuple[str, str, str, float, bytes | None]]:
    items: list[tuple[str, str, str, float, bytes | None]] = [
        ("catalogs", "gaia_nearby.csv", tap(GAIA_TAP, REQUEST="doQuery", LANG="ADQL", FORMAT="csv", QUERY=GAIA_NEAR), 1.8, b"source_id"),
        ("catalogs", "gaia_bright.csv", tap(GAIA_TAP, REQUEST="doQuery", LANG="ADQL", FORMAT="csv", QUERY=GAIA_BRIGHT), 0.9, b"source_id"),
        ("catalogs", "exoplanets.csv", tap(EXO_TAP, query=EXOPLANETS, format="csv"), 1.1, b"pl_name"),
        ("catalogs", "ne_110m_coastline.geojson", f"{NE}/ne_110m_coastline.geojson", 0.14, b"FeatureCollection"),
        ("catalogs", "ne_110m_land.geojson", f"{NE}/ne_110m_land.geojson", 0.14, b"FeatureCollection"),
    ]
    # Pioneer 10's ephemeris begins 1972-03-03 02:04 TDB, so the day after launch is too early
    craft = [
        ("voyager1", "-31", "1977-09-06", "2027-07-01", "1mo"),
        ("voyager2", "-32", "1977-08-21", "2027-07-01", "1mo"),
        ("pioneer10", "-23", "1972-03-04", "2027-07-01", "1mo"),
        ("pioneer11", "-24", "1973-04-07", "2027-07-01", "1mo"),
        ("newhorizons", "-98", "2006-01-20", "2027-07-01", "1mo"),
        ("parker", "-96", "2018-08-13", "2027-07-01", "7d"),
        ("cassini", "-82", "1997-10-16", "2017-09-14", "10d"),
    ]
    # $$EOE closes a Horizons table, so a reply that was cut off doesn't have it
    for name, cmd, start, stop, step in craft:
        items.append(("tracks", f"spacecraft/{name}.txt", horizons(cmd, start, stop, step), 0.12, b"$$EOE"))
    # planet and Moon vectors around now: the cross-check shown on each card
    for name, cmd in [
        ("mercury", "199"), ("venus", "299"), ("earth", "399"), ("mars", "499"), ("jupiter", "599"),
        ("saturn", "699"), ("uranus", "799"), ("neptune", "899"), ("pluto", "999"),
    ]:
        items.append(("tracks", f"spacecraft/planet_{name}.txt", horizons(cmd, "2025-07-01", "2027-07-01", "5d"), 0.03, b"$$EOE"))
    # the Moon's true path around the day of the fetch: the atlas draws it while the date is in this window
    today = dt.date.today()
    moon_from, moon_to = today - dt.timedelta(days=46), today + dt.timedelta(days=46)
    items.append(("tracks", "spacecraft/moon_geo.txt", horizons("301", moon_from.isoformat(), moon_to.isoformat(), "6h", "500@399"), 0.08, b"$$EOE"))
    items.extend(
        [
            ("kernels", "spice/naif0012.tls", f"{NAIF}/lsk/naif0012.tls", 0.01, b"KPL/LSK"),
            ("kernels", "spice/de440s.bsp", f"{NAIF}/spk/planets/de440s.bsp", 32, KERNEL),
            ("kernels", "spice/mar099.bsp", f"{NAIF}/spk/satellites/mar099.bsp", 1171, KERNEL),
            ("kernels", "spice/jup365.bsp", f"{NAIF}/spk/satellites/jup365.bsp", 1084, KERNEL),
            ("kernels", "spice/sat441.bsp", f"{NAIF}/spk/satellites/sat441.bsp", 631, KERNEL),
            ("kernels", "spice/nep097.bsp", f"{NAIF}/spk/satellites/nep097.bsp", 100, KERNEL),
            ("kernels", "spice/plu060.bsp", f"{NAIF}/spk/satellites/plu060.bsp", 129, KERNEL),
        ]
    )
    for name, mb in [
        ("8k_earth_daymap.jpg", 4.4), ("8k_earth_nightmap.jpg", 3.0), ("2k_earth_clouds.jpg", 0.9),
        ("8k_moon.jpg", 14.3), ("8k_mars.jpg", 8.0), ("8k_jupiter.jpg", 2.9), ("8k_saturn.jpg", 1.0),
        ("8k_saturn_ring_alpha.png", 0.1), ("8k_sun.jpg", 3.5), ("4k_venus_atmosphere.jpg", 1.5),
        ("8k_mercury.jpg", 14.3), ("2k_uranus.jpg", 0.1), ("2k_neptune.jpg", 0.2),
    ]:
        items.append(("maps", f"textures/{name}", f"{SSS}/{name}", mb, PNG if name.endswith(".png") else JPEG))
    items.append(("maps", "textures/eso_milkyway.jpg", ESO_PANORAMA, 7.8, JPEG))
    return items


def xdg_data_home() -> Path | None:
    """$XDG_DATA_HOME, or ~/.local/share if it's unset or relative, as the apps read it."""
    for value, below in ((os.environ.get("XDG_DATA_HOME"), ""), (os.environ.get("HOME"), ".local/share")):
        if value and Path(value).is_absolute():
            return Path(value) / below
    return None


def default_dest() -> tuple[Path, Path | None]:
    """Where files go, and the read-only data folder of the installed package, if this is one.

    A checkout (it has Cargo.toml) or an unpacked archive (it has the pantheon binary) keeps
    its data beside tools/. Anything else is the shared folder a package installed, such as
    /usr/share/pantheon, so files go to the user's data folder, never beside this script.
    """
    root = Path(__file__).resolve().parent.parent
    if any((root / name).exists() for name in ("Cargo.toml", "pantheon", "pantheon.exe")):
        return root / "assets" / "data", None
    home = xdg_data_home()
    if home is None:
        raise SystemExit("Set HOME or XDG_DATA_HOME: an installed Pantheon downloads into your data folder.")
    return home / "pantheon" / "assets" / "data", root / "assets" / "data"


def defect(path: Path, rel: str, expect: bytes | None) -> str | None:
    """Why this file can't be trusted as a complete copy of `rel`, or None if it can."""
    try:
        size = path.stat().st_size
    except OSError:
        return "missing"
    want = SIZES.get(rel)
    if want is not None and size != want:
        return f"{size:,} bytes where {want:,} are expected"
    if expect is not None:
        with open(path, "rb") as f:
            head = f.read(1 << 22)
        if expect not in head:
            return f"it doesn't contain the expected marker {expect!r}"
    return None


def download(url: str, dest: Path, rel: str, expect: bytes | None) -> tuple[bool, str]:
    tmp = dest.with_name(dest.name + ".part")
    req = urllib.request.Request(url, headers={"User-Agent": "pantheon-fetch/1 (+tools/fetch_data.py)"})
    try:
        dest.parent.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(req, timeout=600) as r, open(tmp, "wb") as f:
            total = int(r.headers.get("Content-Length") or 0)
            got = 0
            last = time.monotonic()
            while True:
                chunk = r.read(1 << 20)
                if not chunk:
                    break
                f.write(chunk)
                got += len(chunk)
                if total > 50_000_000 and time.monotonic() - last > 5:
                    print(f"    {got * 100 // total}% of {total // 1_000_000} MB", flush=True)
                    last = time.monotonic()
    except (urllib.error.URLError, http.client.HTTPException, TimeoutError, OSError, ValueError) as e:
        with contextlib.suppress(OSError):  # the folder itself may be what failed
            tmp.unlink(missing_ok=True)
        return False, f"{type(e).__name__}: {e}"
    # A connection that drops mid-file just looks like the end of the data to read()
    if total and got != total:
        tmp.unlink()
        return False, f"the connection dropped: {got:,} of {total:,} bytes arrived"
    size = tmp.stat().st_size
    # services report errors as short text bodies with a success status
    if size < 200 and rel not in SIZES:
        msg = tmp.read_text(errors="replace").strip()[:160]
        tmp.unlink()
        return False, f"too small ({size} bytes): {msg}"
    problem = defect(tmp, rel, expect)
    if problem is not None:
        tmp.unlink()
        return False, f"the reply isn't usable: {problem}"
    tmp.replace(dest)
    return True, f"{size // 1000} kB"


def record_fetched(data: Path, rels: list[str]) -> None:
    """Note today's date for the files this run really fetched, and nothing else.

    This is a separate, untracked file so a fetch never dirties provenance.json, which
    describes each dataset and dates the catalogs that ship in the repository.
    """
    path = data / "fetched.json"
    try:
        dates = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        dates = {}
    if not isinstance(dates, dict):
        dates = {}
    today = dt.date.today().isoformat()
    dates.update({rel: today for rel in rels})
    path.write_text(json.dumps(dates, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--list", action="store_true", help="show what would be fetched, then stop")
    ap.add_argument("--only", choices=["catalogs", "tracks", "kernels", "maps"], help="fetch one group")
    ap.add_argument("--no-kernels", action="store_true", help="skip the large JPL kernels")
    ap.add_argument("--force", action="store_true", help="fetch again even if the file exists")
    ap.add_argument(
        "--dest", type=Path,
        help="data directory (default: assets/data beside tools/, or in ~/.local/share/pantheon for a package)",
    )
    args = ap.parse_args()

    data, shipped = (args.dest, None) if args.dest else default_dest()

    def problem(group: str, rel: str, expect: bytes | None) -> str | None:
        """Why the apps don't have `rel` yet, or None if they do."""
        found = defect(data / rel, rel, expect)
        # The apps read catalogs file by file, so a package's own copy serves while the user
        # has none. A damaged copy of the user's own is read first, so it's fetched again.
        if found == "missing" and shipped and group == "catalogs" and defect(shipped / rel, rel, expect) is None:
            return None
        return found

    items = [
        it for it in manifest()
        if (args.only is None or it[0] == args.only) and not (args.no_kernels and it[0] == "kernels")
    ]
    todo = [it for it in items if args.force or problem(it[0], it[1], it[4]) is not None]
    total_mb = sum(it[3] for it in todo)
    if args.list:
        print(f"data folder: {data}")
        for group, rel, _url, mb, expect in items:
            state = problem(group, rel, expect) or "present"
            print(f"{group:9} {rel:38} {mb:8.1f} MB  {state}")
        print(f"to fetch: {len(todo)} files, about {total_mb:.0f} MB")
        return 0
    if not todo:
        print("Nothing to fetch: everything is already here. Use --force to fetch again.")
        return 0

    try:
        data.mkdir(parents=True, exist_ok=True)
    except OSError as e:
        print(f"Can't create {data}: {e}", file=sys.stderr)
        return 1
    if not os.access(data, os.W_OK):
        print(f"Can't write into {data}. Run this as a user who can, or choose a folder with --dest.", file=sys.stderr)
        return 1
    print(f"Fetching {len(todo)} files, about {total_mb:.0f} MB, into {data}")
    log = [f"fetch started {dt.datetime.now().isoformat(timespec='seconds')}"]
    failed: list[str] = []
    fetched: list[str] = []
    for group, rel, url, _mb, expect in todo:
        print(f"  {rel}", flush=True)
        t0 = time.monotonic()
        ok, detail = download(url, data / rel, rel, expect)
        log.append(f"{rel}: ok={ok} {detail} secs={time.monotonic() - t0:.0f}")
        if ok:
            print(f"    {detail}")
            fetched.append(rel)
        else:
            print(f"    FAILED: {detail}")
            failed.append(rel)
        if group == "tracks":
            time.sleep(0.4)  # Horizons asks for gentle use
    log.append(f"fetch finished {dt.datetime.now().isoformat(timespec='seconds')}")
    (data / "fetch_log.txt").write_text("\n".join(log) + "\n", encoding="utf-8")
    if fetched:
        record_fetched(data, fetched)
    if failed:
        print(f"\n{len(failed)} of {len(todo)} files failed. Pantheon runs without them; run this again to retry:")
        for rel in failed:
            print(f"  {rel}")
        return 1
    if shipped:
        print("\nDone. Restart Pantheon to use the new files.")
    else:
        print("\nDone. Check the result with: pantheon-cli --doctor (target/release/pantheon-cli in a checkout)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
