#!/usr/bin/env python3
"""Download Pantheon's datasets into assets/data, standard library only.

A fresh clone already runs: the star, exoplanet and coastline catalogs ship in
the repository. This adds what's too large or too fast-changing to keep in git:
spacecraft and planet tracks, JPL kernels (about 3 GB), and surface imagery
(about 60 MB).

  tools/fetch_data.py                everything still missing
  tools/fetch_data.py --list         what would be fetched, with sizes
  tools/fetch_data.py --only maps    one group: catalogs, tracks, kernels, maps
  tools/fetch_data.py --no-kernels   skip the large kernels
  tools/fetch_data.py --force        fetch again even if present

Every file lands under a temporary name and is renamed only when it's complete,
so an interrupted run never leaves a truncated dataset behind.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
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
def manifest() -> list[tuple[str, str, str, float, str | None]]:
    items: list[tuple[str, str, str, float, str | None]] = [
        ("catalogs", "gaia_nearby.csv", tap(GAIA_TAP, REQUEST="doQuery", LANG="ADQL", FORMAT="csv", QUERY=GAIA_NEAR), 1.8, "source_id"),
        ("catalogs", "gaia_bright.csv", tap(GAIA_TAP, REQUEST="doQuery", LANG="ADQL", FORMAT="csv", QUERY=GAIA_BRIGHT), 0.9, "source_id"),
        ("catalogs", "exoplanets.csv", tap(EXO_TAP, query=EXOPLANETS, format="csv"), 1.1, "pl_name"),
        ("catalogs", "ne_110m_coastline.geojson", f"{NE}/ne_110m_coastline.geojson", 0.14, "FeatureCollection"),
        ("catalogs", "ne_110m_land.geojson", f"{NE}/ne_110m_land.geojson", 0.14, "FeatureCollection"),
    ]
    craft = [
        ("voyager1", "-31", "1977-09-06", "2026-07-01", "1mo"),
        ("voyager2", "-32", "1977-08-21", "2026-07-01", "1mo"),
        ("pioneer10", "-23", "1972-03-03", "2026-07-01", "1mo"),
        ("pioneer11", "-24", "1973-04-07", "2026-07-01", "1mo"),
        ("newhorizons", "-98", "2006-01-20", "2026-07-01", "1mo"),
        ("parker", "-96", "2018-08-13", "2026-07-01", "7d"),
        ("cassini", "-82", "1997-10-16", "2017-09-14", "10d"),
    ]
    for name, cmd, start, stop, step in craft:
        items.append(("tracks", f"spacecraft/{name}.txt", horizons(cmd, start, stop, step), 0.12, "$$SOE"))
    # planet and Moon vectors around now: the cross-check shown on each card
    for name, cmd in [
        ("mercury", "199"), ("venus", "299"), ("earth", "399"), ("mars", "499"), ("jupiter", "599"),
        ("saturn", "699"), ("uranus", "799"), ("neptune", "899"), ("pluto", "999"),
    ]:
        items.append(("tracks", f"spacecraft/planet_{name}.txt", horizons(cmd, "2025-07-01", "2027-07-01", "5d"), 0.03, "$$SOE"))
    items.append(("tracks", "spacecraft/moon_geo.txt", horizons("301", "2026-06-01", "2026-09-01", "6h", "500@399"), 0.08, "$$SOE"))
    items.extend(
        [
            ("kernels", "spice/naif0012.tls", f"{NAIF}/lsk/naif0012.tls", 0.01, None),
            ("kernels", "spice/de440s.bsp", f"{NAIF}/spk/planets/de440s.bsp", 32, None),
            ("kernels", "spice/mar099.bsp", f"{NAIF}/spk/satellites/mar099.bsp", 1171, None),
            ("kernels", "spice/jup365.bsp", f"{NAIF}/spk/satellites/jup365.bsp", 1084, None),
            ("kernels", "spice/sat441.bsp", f"{NAIF}/spk/satellites/sat441.bsp", 631, None),
            ("kernels", "spice/nep097.bsp", f"{NAIF}/spk/satellites/nep097.bsp", 100, None),
            ("kernels", "spice/plu060.bsp", f"{NAIF}/spk/satellites/plu060.bsp", 129, None),
        ]
    )
    for name, mb in [
        ("8k_earth_daymap.jpg", 4.4), ("8k_earth_nightmap.jpg", 3.0), ("2k_earth_clouds.jpg", 0.9),
        ("8k_moon.jpg", 14.3), ("8k_mars.jpg", 8.0), ("8k_jupiter.jpg", 2.9), ("8k_saturn.jpg", 1.0),
        ("8k_saturn_ring_alpha.png", 0.1), ("8k_sun.jpg", 3.5), ("4k_venus_atmosphere.jpg", 1.5),
        ("8k_mercury.jpg", 14.3), ("2k_uranus.jpg", 0.1), ("2k_neptune.jpg", 0.2),
    ]:
        items.append(("maps", f"textures/{name}", f"{SSS}/{name}", mb, None))
    items.append(("maps", "textures/eso_milkyway.jpg", ESO_PANORAMA, 7.8, None))
    return items


def download(url: str, dest: Path, expect: str | None) -> tuple[bool, str]:
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_name(dest.name + ".part")
    req = urllib.request.Request(url, headers={"User-Agent": "pantheon-fetch/1 (+tools/fetch_data.py)"})
    try:
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
    except (urllib.error.URLError, TimeoutError, OSError) as e:
        tmp.unlink(missing_ok=True)
        return False, str(e)
    size = tmp.stat().st_size
    # services report errors as short text bodies with a success status
    if size < 200:
        msg = tmp.read_text(errors="replace").strip()[:160]
        tmp.unlink()
        return False, f"too small ({size} bytes): {msg}"
    if expect is not None:
        with open(tmp, "rb") as f:
            head = f.read(1 << 22)
        if expect.encode() not in head:
            tmp.unlink()
            return False, f"response is missing the expected marker {expect!r}"
    tmp.replace(dest)
    return True, f"{size // 1000} kB"


def write_provenance(data: Path) -> None:
    today = dt.date.today().isoformat()
    gaia = {
        "source": "ESA Gaia DR3 (gaiadr3.gaia_source), ESA Gaia Archive TAP",
        "frame": "ICRS epoch J2016.0",
        "status": "measured",
        "citation": "Gaia Collaboration et al. 2023, A&A 674, A1",
        "license": "CC BY-NC 3.0 IGO",
    }
    prov = {
        "retrieved": today,
        "sources": {
            "gaia_nearby.csv": {**gaia, "query": "nearest 12000 sources, parallax>12mas, parallax_over_error>10, G<16"},
            "gaia_bright.csv": {**gaia, "query": "all sources G<6.0"},
            "exoplanets.csv": {
                "source": "NASA Exoplanet Archive, Planetary Systems (ps) table, default_flag=1",
                "frame": "ICRS; orbital elements heterogeneous per publication",
                "status": "measured/derived per-column",
                "citation": "NASA Exoplanet Archive, IPAC/Caltech",
                "license": "not stated; acknowledgment requested",
            },
            "ne_110m_coastline.geojson": {
                "source": "Natural Earth 1:110m physical vectors v5",
                "frame": "WGS84 lon/lat",
                "status": "measured (generalized cartography)",
                "citation": "naturalearthdata.com",
                "license": "public domain",
            },
            "ne_110m_land.geojson": {
                "source": "Natural Earth 1:110m physical vectors v5",
                "frame": "WGS84 lon/lat",
                "status": "measured (generalized cartography)",
                "citation": "naturalearthdata.com",
                "license": "public domain",
            },
            "spacecraft/*.txt": {
                "source": "NASA/JPL Horizons System API",
                "frame": "heliocentric ecliptic J2000 (Moon: geocentric)",
                "status": "measured (reconstructed/predicted trajectory solutions)",
                "citation": "Giorgini et al., JPL Horizons",
                "license": "not found",
            },
            "spice/*": {
                "source": "NASA NAIF generic kernels (DE440s, satellite SPKs, LSK)",
                "frame": "J2000; TDB",
                "status": "measured (ephemeris solutions)",
                "citation": "NAIF; Park et al. 2021, AJ 161:105",
                "license": "not stated for kernels; acknowledgment encouraged",
            },
            "textures/* (except eso_milkyway.jpg)": {
                "source": "Solar System Scope texture pack",
                "frame": "equirectangular",
                "status": "NASA-based maps, colours tuned, gaps filled with fictional terrain",
                "citation": "solarsystemscope.com/textures",
                "license": "CC BY 4.0",
            },
            "textures/eso_milkyway.jpg": {
                "source": "ESO Milky Way panorama, eso0932a",
                "frame": "galactic, centred on the Galactic Centre",
                "status": "photographic mosaic",
                "citation": "ESO/S. Brunier",
                "license": "CC BY 4.0",
            },
        },
    }
    (data / "provenance.json").write_text(json.dumps(prov, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--list", action="store_true", help="show what would be fetched, then stop")
    ap.add_argument("--only", choices=["catalogs", "tracks", "kernels", "maps"], help="fetch one group")
    ap.add_argument("--no-kernels", action="store_true", help="skip the large JPL kernels")
    ap.add_argument("--force", action="store_true", help="fetch again even if the file exists")
    ap.add_argument("--dest", type=Path, help="data directory (default: assets/data in this repository)")
    args = ap.parse_args()

    data = args.dest or Path(__file__).resolve().parent.parent / "assets" / "data"
    items = [
        it for it in manifest()
        if (args.only is None or it[0] == args.only) and not (args.no_kernels and it[0] == "kernels")
    ]
    todo = [it for it in items if args.force or not (data / it[1]).is_file()]
    total_mb = sum(it[3] for it in todo)
    if args.list:
        for group, rel, _url, mb, _expect in items:
            state = "present" if (data / rel).is_file() else "missing"
            print(f"{group:9} {rel:38} {mb:8.1f} MB  {state}")
        print(f"to fetch: {len(todo)} files, about {total_mb:.0f} MB")
        return 0
    if not todo:
        print("Nothing to fetch: everything is already here. Use --force to fetch again.")
        return 0

    data.mkdir(parents=True, exist_ok=True)
    print(f"Fetching {len(todo)} files, about {total_mb:.0f} MB, into {data}")
    log = [f"fetch started {dt.datetime.now().isoformat(timespec='seconds')}"]
    failed: list[str] = []
    for group, rel, url, _mb, expect in todo:
        print(f"  {rel}", flush=True)
        t0 = time.monotonic()
        ok, detail = download(url, data / rel, expect)
        log.append(f"{rel}: ok={ok} {detail} secs={time.monotonic() - t0:.0f}")
        if ok:
            print(f"    {detail}")
        else:
            print(f"    FAILED: {detail}")
            failed.append(rel)
        if group == "tracks":
            time.sleep(0.4)  # Horizons asks for gentle use
    log.append(f"fetch finished {dt.datetime.now().isoformat(timespec='seconds')}")
    (data / "fetch_log.txt").write_text("\n".join(log) + "\n", encoding="utf-8")
    write_provenance(data)
    if failed:
        print(f"\n{len(failed)} of {len(todo)} files failed. Pantheon runs without them; run this again to retry:")
        for rel in failed:
            print(f"  {rel}")
        return 1
    print("\nDone. Check the result with: pantheon-cli --doctor")
    return 0


if __name__ == "__main__":
    sys.exit(main())
