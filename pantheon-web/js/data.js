// Pantheon 3D — data.js: catalog loaders (Gaia nearby/bright CSV, NASA
// Exoplanet Archive CSV, Natural Earth coastline GeoJSON, Horizons tracks).
// All world positions heliocentric ecliptic J2000, meters.

import * as THREE from 'three';

// Constants used locally (eph.js owns the exported copies).
const PC = 3.0856775814913673e16; // m
const OBLIQUITY_DEG = 23.4392911;

const DEG = Math.PI / 180;
const COS_E = Math.cos(OBLIQUITY_DEG * DEG);
const SIN_E = Math.sin(OBLIQUITY_DEG * DEG);

// u_ecl = M * u_icrs — this exact rotation, NOT its transpose.
export function icrsToEcl(raDeg, decDeg) {
  const a = raDeg * DEG, d = decDeg * DEG, cd = Math.cos(d);
  const x = cd * Math.cos(a), y = cd * Math.sin(a), z = Math.sin(d);
  return new THREE.Vector3(x, COS_E * y + SIN_E * z, -SIN_E * y + COS_E * z);
}

// ------------------------------------------------------------------ CSV core

// Char-scan CSV: quoted fields (embedded commas/newlines, "" escapes),
// '#' comment lines (NASA archive preamble), CR/LF/CRLF line ends.
function parseCsv(text) {
  const rows = [];
  let field = '', row = [], inQ = false, atLineStart = true, inComment = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (inComment) {
      if (c === '\n') { inComment = false; atLineStart = true; }
      continue;
    }
    if (inQ) {
      if (c === '"') {
        if (text[i + 1] === '"') { field += '"'; i++; } else inQ = false;
      } else field += c;
      continue;
    }
    if (atLineStart && c === '#') { inComment = true; continue; }
    atLineStart = false;
    if (c === '"') inQ = true;
    else if (c === ',') { row.push(field); field = ''; }
    else if (c === '\n' || c === '\r') {
      if (c === '\r' && text[i + 1] === '\n') i++;
      if (field.length || row.length) { row.push(field); rows.push(row); }
      field = ''; row = []; atLineStart = true;
    } else field += c;
  }
  if (field.length || row.length) { row.push(field); rows.push(row); }
  return rows;
}

function headerIndex(row) {
  const idx = Object.create(null);
  for (let i = 0; i < row.length; i++) idx[row[i].trim()] = i;
  return idx;
}

function num(row, i) {
  if (i == null) return null;
  const t = (row[i] || '').trim();
  if (!t) return null;
  const v = Number(t);
  return Number.isFinite(v) ? v : null;
}

function str(row, i) {
  if (i == null) return null;
  const t = (row[i] || '').trim();
  return t || null;
}

async function fetchText(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`fetch ${url}: HTTP ${r.status}`);
  return r.text();
}

// ---------------------------------------------------------------- star color

// bp_rp -> linear RGB ramp, ported from src/data.rs star_rgb (used by atlas.rs):
// blue-white through ivory and gold to deep orange-red.
const STAR_STOPS = [
  [-0.4, [0.60, 0.71, 1.00]],
  [0.3, [0.79, 0.87, 1.00]],
  [0.7, [1.00, 0.98, 0.94]],
  [1.2, [1.00, 0.90, 0.76]],
  [2.0, [1.00, 0.77, 0.54]],
  [3.6, [1.00, 0.60, 0.40]],
];

export function starRgb(bpRp) {
  const x = Math.min(3.6, Math.max(-0.4, bpRp));
  for (let i = 0; i < STAR_STOPS.length - 1; i++) {
    const [x0, a] = STAR_STOPS[i], [x1, b] = STAR_STOPS[i + 1];
    if (x <= x1) {
      const t = (x - x0) / (x1 - x0);
      return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
    }
  }
  return STAR_STOPS[STAR_STOPS.length - 1][1].slice();
}

// ---------------------------------------------------------------- Gaia stars

function buildStarSet(text, bright) {
  const rows = parseCsv(text);
  const col = rows.length ? headerIndex(rows[0]) : {};
  const pos = [], colr = [], size = [], meta = [];
  for (let r = 1; r < rows.length; r++) {
    const row = rows[r];
    const ra = num(row, col.ra), dec = num(row, col.dec), g = num(row, col.phot_g_mean_mag);
    if (ra == null || dec == null || g == null) continue;
    const par = num(row, col.parallax);
    let distPc;
    if (par != null && par > 0) {
      distPc = 1000 / par;
      if (bright) distPc = Math.min(2000, Math.max(1, distPc)); // G<6 parallaxes unreliable
    } else if (bright) {
      distPc = 500; // parallax missing/negative in the bright backdrop
    } else {
      continue; // nearby set: skip bad parallax
    }
    // Catalog epoch J2016 position only — no per-frame proper motion.
    const u = icrsToEcl(ra, dec), d = distPc * PC;
    pos.push(u.x * d, u.y * d, u.z * d);
    const bprp = num(row, col.bp_rp) ?? 0.8;
    const rgb = starRgb(bprp);
    // apparent-mag luminance keeps colors in the 0..~1.6 linear range
    const lum = Math.min(1.6, Math.max(0.45, 1.6 - 0.09 * (g - 1)));
    colr.push(rgb[0] * lum, rgb[1] * lum, rgb[2] * lum);
    const Mg = g - 5 * Math.log10(distPc / 10);
    size.push(Math.min(3.2, Math.max(0.35, 2.2 - 0.13 * Mg)));
    const perr = num(row, col.parallax_error);
    const distErrPc = perr != null && par > 0 ? Math.abs(perr / par) * distPc : 0;
    meta.push({ id: str(row, col.source_id) || '', gmag: g, bprp, distPc, distErrPc, ra, dec });
  }
  return {
    count: meta.length,
    positions: new Float32Array(pos),
    colors: new Float32Array(colr),
    sizes: new Float32Array(size),
    meta,
  };
}

export async function loadStars(url = '/data/gaia_nearby.csv') {
  return buildStarSet(await fetchText(url), false);
}

export async function loadBrightStars(url = '/data/gaia_bright.csv') {
  return buildStarSet(await fetchText(url), true);
}

// --------------------------------------------------------------- named stars

// Ported from src/data.rs NAMED_STARS: name, RA deg, Dec deg, dist pc,
// V mag, B−V-ish color index (bp_rp-ish).
export const NAMED_STARS = [
  ['Sirius', 101.2871, -16.7161, 2.64, -1.46, 0.0],
  ['Canopus', 95.9880, -52.6957, 95.0, -0.74, 0.19],
  ['α Centauri', 219.9021, -60.8340, 1.34, -0.27, 0.9],
  ['Arcturus', 213.9153, 19.1824, 11.3, -0.05, 1.5],
  ['Vega', 279.2347, 38.7837, 7.68, 0.03, 0.0],
  ['Capella', 79.1723, 45.9980, 13.1, 0.08, 1.0],
  ['Rigel', 78.6345, -8.2016, 260.0, 0.13, -0.03],
  ['Procyon', 114.8255, 5.2250, 3.51, 0.34, 0.53],
  ['Achernar', 24.4285, -57.2368, 43.0, 0.46, -0.16],
  ['Betelgeuse', 88.7929, 7.4071, 170.0, 0.50, 2.3],
  ['Hadar', 210.9559, -60.3730, 120.0, 0.61, -0.23],
  ['Altair', 297.6958, 8.8683, 5.13, 0.76, 0.28],
  ['Acrux', 186.6496, -63.0991, 99.0, 0.76, -0.24],
  ['Aldebaran', 68.9802, 16.5093, 20.4, 0.86, 1.9],
  ['Antares', 247.3519, -26.4320, 170.0, 0.96, 2.4],
  ['Spica', 201.2983, -11.1613, 77.0, 0.97, -0.23],
  ['Pollux', 116.3289, 28.0262, 10.4, 1.14, 1.3],
  ['Fomalhaut', 344.4127, -29.6222, 7.70, 1.16, 0.14],
  ['Deneb', 310.3580, 45.2803, 800.0, 1.25, 0.15],
  ['Mimosa', 191.9303, -59.6888, 85.0, 1.25, -0.23],
  ['Regulus', 152.0930, 11.9672, 24.3, 1.39, -0.11],
  ['Adhara', 104.6564, -28.9721, 132.0, 1.50, -0.21],
  ['Castor', 113.6494, 31.8883, 15.6, 1.62, 0.1],
  ['Polaris', 37.9546, 89.2641, 133.0, 1.98, 0.75],
  ['Proxima Centauri', 217.4289, -62.6795, 1.30, 11.13, 3.6],
  ["Barnard's Star", 269.4521, 4.6934, 1.83, 9.54, 2.8],
  ['Wolf 359', 164.120, 7.014, 2.41, 13.54, 3.9],
  ['Lalande 21185', 165.834, 35.970, 2.55, 7.52, 2.3],
  ['Tau Ceti', 26.017, -15.937, 3.65, 3.50, 0.9],
  ['ε Eridani', 53.233, -9.458, 3.22, 3.73, 1.1],
  ['61 Cygni', 316.745, 38.749, 3.50, 5.21, 1.5],
  ['ε Indi', 330.840, -56.786, 3.64, 4.69, 1.3],
  ['Ross 128', 176.935, 0.799, 3.37, 11.13, 3.4],
  ["Kapteyn's Star", 77.919, -45.018, 3.93, 8.85, 2.1],
  ['Groombridge 34', 4.595, 44.023, 3.56, 8.08, 2.9],
  ["Luyten's Star", 111.853, 5.226, 3.80, 9.87, 2.7],
].map(([name, ra, dec, distPc, vmag, ci]) => ({ name, ra, dec, distPc, vmag, ci }));

// Attach names to catalog stars by angular proximity (<0.05°).
// Fills starSet.names = Map(index -> name); also returns the Map.
export function nameStars(starSet) {
  const meta = starSet.meta, n = meta.length;
  const ux = new Float64Array(n), uy = new Float64Array(n), uz = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    const a = meta[i].ra * DEG, d = meta[i].dec * DEG, cd = Math.cos(d);
    ux[i] = cd * Math.cos(a); uy[i] = cd * Math.sin(a); uz[i] = Math.sin(d);
  }
  const cosTol = Math.cos(0.05 * DEG);
  const names = new Map();
  const bestDotAt = new Map(); // keep the closer name if two claim one star
  for (const ns of NAMED_STARS) {
    const a = ns.ra * DEG, d = ns.dec * DEG, cd = Math.cos(d);
    const vx = cd * Math.cos(a), vy = cd * Math.sin(a), vz = Math.sin(d);
    let best = -1, bestDot = cosTol;
    for (let i = 0; i < n; i++) {
      const dot = ux[i] * vx + uy[i] * vy + uz[i] * vz;
      if (dot > bestDot) { bestDot = dot; best = i; }
    }
    if (best >= 0 && !(bestDotAt.get(best) > bestDot)) {
      names.set(best, ns.name);
      bestDotAt.set(best, bestDot);
    }
  }
  starSet.names = names;
  return names;
}

// ---------------------------------------------------------------- exoplanets

// NASA Exoplanet Archive PS table (default_flag=1), grouped by host.
// Rows without sy_dist are skipped; missing values become null.
export async function loadExo(url = '/data/exoplanets.csv') {
  const rows = parseCsv(await fetchText(url));
  const col = rows.length ? headerIndex(rows[0]) : {};
  const hosts = new Map();
  for (let r = 1; r < rows.length; r++) {
    const row = rows[r];
    // sy_dist may be absent in the default parameter set (e.g. TRAPPIST-1);
    // keep the system with distPc=null — the app resolves it via Gaia later
    const distPc = num(row, col.sy_dist);
    const host = str(row, col.hostname);
    const name = str(row, col.pl_name);
    if (!host || !name) continue;
    let h = hosts.get(host);
    if (!h) {
      h = {
        host,
        ra: num(row, col.ra), dec: num(row, col.dec), distPc,
        teff: null, stRad: null, stMass: null, spec: null,
        planets: [],
      };
      hosts.set(host, h);
    }
    if (h.teff == null) h.teff = num(row, col.st_teff);
    if (h.stRad == null) h.stRad = num(row, col.st_rad);
    if (h.stMass == null) h.stMass = num(row, col.st_mass);
    if (h.spec == null) h.spec = str(row, col.st_spectype);
    h.planets.push({
      name,
      perD: num(row, col.pl_orbper),
      aAU: num(row, col.pl_orbsmax),
      ecc: num(row, col.pl_orbeccen),
      radE: num(row, col.pl_rade),
      massE: num(row, col.pl_bmasse),
      eqt: num(row, col.pl_eqt),
      discYear: num(row, col.disc_year),
      method: str(row, col.discoverymethod),
    });
  }
  return [...hosts.values()];
}

// ----------------------------------------------------------------- coastline

// Natural Earth 1:110m coastline -> [[ [lonDeg,latDeg], ... ], ...]
export async function loadCoast(url = '/data/ne_110m_coastline.geojson') {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`fetch ${url}: HTTP ${r.status}`);
  const gj = await r.json();
  const lines = [];
  for (const f of gj.features || []) {
    const g = f && f.geometry;
    if (!g) continue;
    if (g.type === 'LineString') lines.push(g.coordinates);
    else if (g.type === 'MultiLineString') for (const c of g.coordinates) lines.push(c);
  }
  return lines;
}

// ------------------------------------------------------------------- tracks

// Server emits [{name, samples:[[jd,x,y,z,vx,vy,vz]...]}]; we keep [jd,x,y,z]
// per the module contract. Returns [] on any failure.
export async function loadTracks(url = '/api/tracks') {
  try {
    const r = await fetch(url);
    if (!r.ok) return [];
    const arr = await r.json();
    if (!Array.isArray(arr)) return [];
    return arr.map(t => ({
      name: t.name,
      samples: (t.samples || []).map(s => [s[0], s[1], s[2], s[3]]),
    }));
  } catch {
    return [];
  }
}
