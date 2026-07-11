# Pantheon dataset fetcher — caches real astronomical data with provenance.
# Sources: ESA Gaia DR3 TAP, NASA Exoplanet Archive TAP, NASA/JPL Horizons API,
# Natural Earth (via github raw). Run from anywhere; writes into assets/data.
$ErrorActionPreference = "Continue"
$root = Split-Path $PSScriptRoot -Parent
$data = Join-Path $root "assets\data"
New-Item -ItemType Directory -Force $data, (Join-Path $data "spacecraft") | Out-Null
$today = Get-Date -Format "yyyy-MM-dd"
$log = Join-Path $data "fetch_log.txt"
"fetch started $today $(Get-Date -Format HH:mm:ss)" | Out-File $log -Encoding utf8

function Fetch($name, $url, $outfile, $extra) {
    $t0 = Get-Date
    $args = @("-s", "--fail", "-m", "600", "-o", $outfile) + $extra + @($url)
    & curl.exe @args
    $ok = ($LASTEXITCODE -eq 0) -and (Test-Path $outfile) -and ((Get-Item $outfile).Length -gt 200)
    $len = if (Test-Path $outfile) { (Get-Item $outfile).Length } else { 0 }
    $dt = [int]((Get-Date) - $t0).TotalSeconds
    "$name : ok=$ok bytes=$len secs=$dt" | Out-File $log -Append -Encoding utf8
    return $ok
}

# ---- Gaia DR3: nearest stars with reliable astrometry (sync TAP) ----
$gaiaNear = "SELECT TOP 12000 source_id,ra,dec,parallax,parallax_error,pmra,pmdec,radial_velocity,phot_g_mean_mag,bp_rp FROM gaiadr3.gaia_source WHERE parallax > 12 AND parallax_over_error > 10 AND phot_g_mean_mag < 16 ORDER BY parallax DESC"
Fetch "gaia_nearby" "https://gea.esac.esa.int/tap-server/tap/sync" (Join-Path $data "gaia_nearby.csv") @("-G", "--data-urlencode", "REQUEST=doQuery", "--data-urlencode", "LANG=ADQL", "--data-urlencode", "FORMAT=csv", "--data-urlencode", "QUERY=$gaiaNear")

# ---- Gaia DR3: naked-eye sky (G < 6.0) ----
$gaiaBright = "SELECT source_id,ra,dec,parallax,pmra,pmdec,phot_g_mean_mag,bp_rp FROM gaiadr3.gaia_source WHERE phot_g_mean_mag < 6.0"
Fetch "gaia_bright" "https://gea.esac.esa.int/tap-server/tap/sync" (Join-Path $data "gaia_bright.csv") @("-G", "--data-urlencode", "REQUEST=doQuery", "--data-urlencode", "LANG=ADQL", "--data-urlencode", "FORMAT=csv", "--data-urlencode", "QUERY=$gaiaBright")

# ---- NASA Exoplanet Archive: all confirmed planets (default parameter set) ----
$exoQ = "select pl_name,hostname,sy_snum,sy_pnum,discoverymethod,disc_year,pl_orbper,pl_orbsmax,pl_orbeccen,pl_orbincl,pl_rade,pl_bmasse,pl_eqt,st_teff,st_rad,st_mass,st_spectype,ra,dec,sy_dist,sy_vmag from ps where default_flag=1"
Fetch "exoplanets" "https://exoplanetarchive.ipac.caltech.edu/TAP/sync" (Join-Path $data "exoplanets.csv") @("-G", "--data-urlencode", "query=$exoQ", "--data-urlencode", "format=csv")

# ---- Natural Earth 110m coastline + land polygons (vector Earth) ----
Fetch "coastline" "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/ne_110m_coastline.geojson" (Join-Path $data "ne_110m_coastline.geojson") @()
Fetch "land" "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/ne_110m_land.geojson" (Join-Path $data "ne_110m_land.geojson") @()

# ---- JPL Horizons: heliocentric state vectors ----
function Horizons($name, $cmd, $start, $stop, $step, $center) {
    $out = Join-Path $data "spacecraft\$name.txt"
    Fetch "horizons_$name" "https://ssd.jpl.nasa.gov/api/horizons.api" $out @(
        "-G",
        "--data-urlencode", "format=text",
        "--data-urlencode", "COMMAND='$cmd'",
        "--data-urlencode", "OBJ_DATA='NO'",
        "--data-urlencode", "MAKE_EPHEM='YES'",
        "--data-urlencode", "EPHEM_TYPE=VECTORS",
        "--data-urlencode", "CENTER='$center'",
        "--data-urlencode", "START_TIME='$start'",
        "--data-urlencode", "STOP_TIME='$stop'",
        "--data-urlencode", "STEP_SIZE='$step'",
        "--data-urlencode", "VEC_TABLE='2'",
        "--data-urlencode", "REF_PLANE='ECLIPTIC'",
        "--data-urlencode", "REF_SYSTEM='J2000'",
        "--data-urlencode", "OUT_UNITS='AU-D'",
        "--data-urlencode", "CSV_FORMAT='YES'"
    )
    Start-Sleep -Milliseconds 400
}

# Spacecraft trails (heliocentric)
Horizons "voyager1"    "-31" "1977-09-06" "2026-07-01" "1mo" "500@10"
Horizons "voyager2"    "-32" "1977-08-21" "2026-07-01" "1mo" "500@10"
Horizons "pioneer10"   "-23" "1972-03-03" "2026-07-01" "1mo" "500@10"
Horizons "pioneer11"   "-24" "1973-04-07" "2026-07-01" "1mo" "500@10"
Horizons "newhorizons" "-98" "2006-01-20" "2026-07-01" "1mo" "500@10"
Horizons "parker"      "-96" "2018-08-13" "2026-07-01" "7d"  "500@10"
Horizons "cassini"     "-82" "1997-10-16" "2017-09-14" "10d" "500@10"

# Planet + Moon state vectors around now (validation + ephemeris-trail ground truth)
$pl = @{ mercury="199"; venus="299"; earth="399"; mars="499"; jupiter="599"; saturn="699"; uranus="799"; neptune="899"; pluto="999" }
foreach ($k in $pl.Keys) {
    Horizons "planet_$k" $pl[$k] "2025-07-01" "2027-07-01" "5d" "500@10"
}
Horizons "moon_geo" "301" "2026-06-01" "2026-09-01" "6h" "500@399"

"fetch finished $(Get-Date -Format HH:mm:ss)" | Out-File $log -Append -Encoding utf8

# ---- provenance manifest ----
$prov = @"
{
  "retrieved": "$today",
  "sources": {
    "gaia_nearby.csv":  { "source": "ESA Gaia DR3 (gaiadr3.gaia_source), ESA Gaia Archive TAP", "query": "nearest 12000 sources, parallax>12mas, parallax_over_error>10, G<16", "frame": "ICRS epoch J2016.0", "status": "measured", "citation": "Gaia Collaboration et al. 2023, A&A 674, A1" },
    "gaia_bright.csv":  { "source": "ESA Gaia DR3 (gaiadr3.gaia_source), ESA Gaia Archive TAP", "query": "all sources G<6.0", "frame": "ICRS epoch J2016.0", "status": "measured", "citation": "Gaia Collaboration et al. 2023, A&A 674, A1" },
    "exoplanets.csv":   { "source": "NASA Exoplanet Archive, Planetary Systems (ps) table, default_flag=1", "frame": "ICRS; orbital elements heterogeneous per publication", "status": "measured/derived per-column", "citation": "NASA Exoplanet Archive, IPAC/Caltech" },
    "ne_110m_coastline.geojson": { "source": "Natural Earth 1:110m physical vectors v5", "frame": "WGS84 lon/lat", "status": "measured (generalized cartography)", "citation": "naturalearthdata.com, public domain" },
    "spacecraft/*.txt": { "source": "NASA/JPL Horizons System API", "frame": "heliocentric ecliptic J2000 (Moon: geocentric)", "status": "measured (reconstructed/predicted trajectory solutions)", "citation": "Giorgini et al., JPL Horizons" }
  }
}
"@
$prov | Out-File (Join-Path $data "provenance.json") -Encoding utf8
"provenance written" | Out-File $log -Append -Encoding utf8

# ---- NAIF SPICE kernels: DE440s planetary ephemeris + leap seconds ----
New-Item -ItemType Directory -Force (Join-Path $data "spice") | Out-Null
Fetch "spice_de440s" "https://naif.jpl.nasa.gov/pub/naif/generic_kernels/spk/planets/de440s.bsp" (Join-Path $data "spice\de440s.bsp") @()
Fetch "spice_lsk" "https://naif.jpl.nasa.gov/pub/naif/generic_kernels/lsk/naif0012.tls" (Join-Path $data "spice\naif0012.tls") @()
