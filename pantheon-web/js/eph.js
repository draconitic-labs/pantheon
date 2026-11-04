// eph.js — ephemeris + IAU orientation. Port of src/sim.rs (analytic paths).
// Frames: heliocentric ecliptic J2000, meters, velocities m/day, time jd TDB.
// SPICE-grade truth arrives from /api/states; this module is the always-on
// model (Standish 1992 mean elements, valid 1800–2050) and extrapolation basis.

import * as THREE from 'three';

// ------------------------------------------------------------- constants

export const AU = 1.495978707e11;          // m
export const PC = 3.0856775814913673e16;   // m
export const KPC = 1e3 * PC;
export const DAY_S = 86400.0;
export const J2000_JD = 2451545.0;
export const OBLIQUITY_DEG = 23.4392911;
export const SUN_RADIUS_M = 695700.0e3;

const DEG = Math.PI / 180;
const TAU = Math.PI * 2;

// u_ecl = M * u_icrs, rows [1,0,0],[0,cos e,sin e],[0,-sin e,cos e] — Rx(−ε).
// The transposed version is a known past bug; do not "fix" this.
function icrsToEclVec(x, y, z) {
  const c = Math.cos(OBLIQUITY_DEG * DEG), s = Math.sin(OBLIQUITY_DEG * DEG);
  return new THREE.Vector3(x, c * y + s * z, -s * y + c * z);
}

// ------------------------------------------------------------ jd helpers

// TT−UTC = 32.184 s + 37 leap seconds (constant since 2017); TDB−TT < 2 ms neglected.
const TT_MINUS_UTC_S = 69.184;

export function jdUtcNow() { return 2440587.5 + Date.now() / 86400000.0; }
export function jdTdb(jdUtc) { return jdUtc + TT_MINUS_UTC_S / 86400.0; }
export function jdUtcFromTdb(jd) { return jd - TT_MINUS_UTC_S / 86400.0; }

// Gregorian calendar -> JD (Fliegel–Van Flandern); d may be fractional.
export function calToJd(y, m, d) {
  let yy = y, mm = m;
  if (m <= 2) { yy = y - 1; mm = m + 12; }
  const a = Math.trunc(yy / 100);
  const b = 2 - a + Math.trunc(a / 4);
  return Math.floor(365.25 * (yy + 4716)) + Math.floor(30.6001 * (mm + 1)) + d + b - 1524.5;
}

// JD -> [year, month, day, hour, minute]
export function jdToCal(jd) {
  const z = Math.floor(jd + 0.5);
  const f = jd + 0.5 - z;
  let a = z;
  if (z >= 2299161.0) {
    const alpha = Math.floor((z - 1867216.25) / 36524.25);
    a = z + 1 + alpha - Math.floor(alpha / 4);
  }
  const b = a + 1524.0;
  const c = Math.floor((b - 122.1) / 365.25);
  const d = Math.floor(365.25 * c);
  const e = Math.floor((b - d) / 30.6001);
  const day = b - d - Math.floor(30.6001 * e) + f;
  const month = e < 14 ? e - 1 : e - 13;
  const year = month > 2 ? c - 4716 : c - 4715;
  const dd = Math.floor(day);
  const hh = (day - dd) * 24.0;
  const h = Math.floor(hh);
  const mi = Math.floor((hh - h) * 60.0);
  return [year, month, dd, h, mi];
}

const MON = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

// Format a TDB Julian date as a UTC calendar string.
export function fmtJd(jdTdbVal) {
  const [y, m, d, h, mi] = jdToCal(jdUtcFromTdb(jdTdbVal));
  const p2 = (n) => String(n).padStart(2, '0');
  return `${y} ${MON[Math.min(Math.max(m, 1) - 1, 11)]} ${p2(d)}  ${p2(h)}:${p2(mi)} UTC`;
}

// --------------------------------------------------------------- planets

const hex = (c) => (Math.round(c[0] * 255) << 16) | (Math.round(c[1] * 255) << 8) | Math.round(c[2] * 255);

const P = (name, el, rate, radiusKm, gm, obliquityDeg, rotHours, color, color2, rim, kind, flatten) => ({
  name, el, rate, radius: radiusKm * 1e3, gm, obliquityDeg, rotHours,
  color, color2, rim, kind, flatten, a: el[0], colorHex: hex(color),
});

// JPL SSD "Approximate Positions of the Planets" Table 1 (Standish), 1800–2050.
// el: a(AU), e, I(deg), L(deg), long.peri(deg), long.node(deg); rate: per century.
// The "Earth" row is the Earth–Moon barycenter.
export const PLANETS = [
  P('Mercury',
    [0.38709927, 0.20563593, 7.00497902, 252.25032350, 77.45779628, 48.33076593],
    [0.00000037, 0.00001906, -0.00594749, 149472.67411175, 0.16047689, -0.12534081],
    2439.7, 2.2032e13, 0.03, 1407.6,
    [0.62, 0.58, 0.54], [0.38, 0.35, 0.33], [0.0, 0.0, 0.0], 1.0, 0.0),
  P('Venus',
    [0.72333566, 0.00677672, 3.39467605, 181.97909950, 131.60246718, 76.67984255],
    [0.00000390, -0.00004107, -0.00078890, 58517.81538729, 0.00268329, -0.27769418],
    6051.8, 3.24859e14, 177.4, -5832.5,
    [0.93, 0.85, 0.66], [0.80, 0.70, 0.52], [0.95, 0.85, 0.6], 2.0, 0.0),
  P('Earth',
    [1.00000261, 0.01671123, -0.00001531, 100.46457166, 102.93768193, 0.0],
    [0.00000562, -0.00004392, -0.01294668, 35999.37244981, 0.32327364, 0.0],
    6371.0, 3.986004418e14, 23.44, 23.9345,
    [0.09, 0.22, 0.45], [0.2, 0.32, 0.2], [0.45, 0.65, 1.0], 4.0, 0.0034),
  P('Mars',
    [1.52371034, 0.09339410, 1.84969142, -4.55343205, -23.94362959, 49.55953891],
    [0.00001847, 0.00007882, -0.00813131, 19140.30268499, 0.44441088, -0.29257343],
    3389.5, 4.2828375e13, 25.19, 24.6229,
    [0.78, 0.42, 0.26], [0.48, 0.26, 0.18], [0.9, 0.55, 0.35], 1.0, 0.0059),
  P('Jupiter',
    [5.20288700, 0.04838624, 1.30439695, 34.39644051, 14.72847983, 100.47390909],
    [-0.00011607, -0.00013253, -0.00183714, 3034.74612775, 0.21252668, 0.20469106],
    69911.0, 1.26686534e17, 3.13, 9.925,
    [0.83, 0.72, 0.55], [0.62, 0.45, 0.34], [0.9, 0.8, 0.6], 2.0, 0.065),
  P('Saturn',
    [9.53667594, 0.05386179, 2.48599187, 49.95424423, 92.59887831, 113.66242448],
    [-0.00125060, -0.00050991, 0.00193609, 1222.49362201, -0.41897216, -0.28867794],
    58232.0, 3.7931187e16, 26.73, 10.656,
    [0.91, 0.83, 0.63], [0.75, 0.66, 0.47], [0.95, 0.88, 0.65], 2.0, 0.098),
  P('Uranus',
    [19.18916464, 0.04725744, 0.77263783, 313.23810451, 170.95427630, 74.01692503],
    [-0.00196176, -0.00004397, -0.00242939, 428.48202785, 0.40805281, 0.04240589],
    25362.0, 5.793939e15, 97.8, -17.24,
    [0.62, 0.85, 0.88], [0.5, 0.72, 0.78], [0.7, 0.95, 1.0], 2.0, 0.023),
  P('Neptune',
    [30.06992276, 0.00859048, 1.77004347, -55.12002969, 44.96476227, 131.78422574],
    [0.00026291, 0.00005105, 0.00035372, 218.45945325, -0.32241464, -0.00508664],
    24622.0, 6.836529e15, 28.3, 16.11,
    [0.32, 0.45, 0.92], [0.2, 0.3, 0.7], [0.5, 0.65, 1.0], 2.0, 0.017),
  P('Pluto',
    [39.48211675, 0.24882730, 17.14001206, 238.92903833, 224.06891629, 110.30393684],
    [-0.00031596, 0.00005170, 0.00004818, 145.20780515, -0.04062942, -0.01183482],
    1188.3, 8.71e11, 122.5, -153.29,
    [0.78, 0.68, 0.58], [0.55, 0.46, 0.4], [0.0, 0.0, 0.0], 5.0, 0.0),
];

// -------------------------------------------------------- kepler + orbits

// Newton iteration; start at M (or π when e ≥ 0.8). M may be negative.
export function keplerE(M, e) {
  let ea = e < 0.8 ? M : Math.PI;
  for (let k = 0; k < 12; k++) {
    ea -= (ea - e * Math.sin(ea) - M) / (1.0 - e * Math.cos(ea));
  }
  return ea;
}

// Rz(node)·Rx(inc)·Rz(argp) applied to perifocal (x, y, 0) — radians.
function perifocalToEcl(node, inc, argp, x, y) {
  const ca = Math.cos(argp), sa = Math.sin(argp);
  const px = ca * x - sa * y, py = sa * x + ca * y;
  const ci = Math.cos(inc), si = Math.sin(inc);
  const qy = ci * py, qz = si * py;
  const cn = Math.cos(node), sn = Math.sin(node);
  return new THREE.Vector3(cn * px - sn * qy, sn * px + cn * qy, qz);
}

// An orbit at jd. The linear rates are fitted to 1800-2050 (T -2 to 0.5); outside that
// they hold at the nearest limit, so an orbit stays a real ellipse at any date. Only the
// mean longitude keeps running. Kept identical to orbit_shape in src/sim.rs.
function orbitShape(p, jd) {
  const t = Math.min(Math.max((jd - J2000_JD) / 36525.0, -2.0), 0.5);
  return {
    a: (p.el[0] + p.rate[0] * t) * AU,
    e: p.el[1] + p.rate[1] * t,
    inc: (p.el[2] + p.rate[2] * t) * DEG,
    lp: (p.el[4] + p.rate[4] * t) * DEG,
    node: (p.el[5] + p.rate[5] * t) * DEG,
  };
}

// Heliocentric position (m) of PLANETS[i] from the mean elements at jd.
// For Earth this is the EMB; use earthCenter/planetState for the true center.
export function planetPos(i, jd) {
  const p = PLANETS[i];
  const t = (jd - J2000_JD) / 36525.0;
  const { a, e, inc, lp, node } = orbitShape(p, jd);
  const l = (p.el[3] + p.rate[3] * t) * DEG;
  const m = (l - lp) % TAU;
  const ea = keplerE(m, e);
  return perifocalToEcl(node, inc, lp - node,
    a * (Math.cos(ea) - e), a * Math.sqrt(1.0 - e * e) * Math.sin(ea));
}

// GM_moon / (GM_earth + GM_moon): Earth-center offset fraction from the EMB.
const MU_MOON_FRAC = 4.9028e12 / (3.986004418e14 + 4.9028e12);

// Earth center = EMB minus the mass-weighted geocentric Moon vector.
export function earthCenter(jd) {
  return planetPos(2, jd).addScaledVector(lunaRel(jd), -MU_MOON_FRAC);
}

function analyticPos(i, jd) {
  return i === 2 ? earthCenter(jd) : planetPos(i, jd);
}

// {pos: m helio-ecl, vel: m/day} — central difference h = 0.01 d.
export function planetState(i, jd) {
  const h = 0.01;
  const vel = analyticPos(i, jd + h).sub(analyticPos(i, jd - h)).multiplyScalar(1.0 / (2.0 * h));
  return { pos: analyticPos(i, jd), vel };
}

export function sunState() {
  return { pos: new THREE.Vector3(0, 0, 0), vel: new THREE.Vector3(0, 0, 0) };
}

// Sampled full orbit ellipse (root frame) at the osculating epoch, n points.
export function planetOrbit(i, jd, n = 256) {
  const p = PLANETS[i];
  const t = (jd - J2000_JD) / 36525.0;
  const { a, e, inc, lp, node } = orbitShape(p, jd);
  const out = [];
  for (let k = 0; k < n; k++) {
    const ea = (k / n) * TAU;
    out.push(perifocalToEcl(node, inc, lp - node,
      a * (Math.cos(ea) - e), a * Math.sqrt(1.0 - e * e) * Math.sin(ea)));
  }
  return out;
}

// ----------------------------------------------------------------- moons

// Inclination of each moon's orbit to its parent's equator (deg), aligned with
// MOONS. Node longitudes approximate (0 in the equator frame). Triton 156.9°
// retrograde is the one that matters most.
export const MOON_INCL = [
  0.0, 1.1, 1.8, 0.04, 0.47, 0.2, 0.2, 1.57, 0.01, 1.1, 0.03, 0.35, 0.35, 15.47, 4.34, 0.04,
  0.13, 0.08, 0.07, 156.9, 0.08,
];

const Mn = (parent, name, aKm, period, radiusKm, e, phase, color, color2, kind) => ({
  parent, name, a: aKm * 1e3, period, radius: radiusKm * 1e3, e, phase,
  color, color2, kind, colorHex: hex(color), node: 0.0,
});

// JPL planetary satellite mean elements, circular-inclined approximation
// (orbital plane = parent equator + incl; phases illustrative except the Moon).
// a in m, period in days, incl deg to parent equator, phase = M0 deg at J2000.
// Triton's retrograde sense lives ONLY in the 156.9° inclination —
// its period stays positive; never encode retrograde twice.
export const MOONS = [
  Mn(2, 'Moon', 384400.0, 27.321661, 1737.4, 0.0549, 135.27, [0.62, 0.62, 0.6], [0.42, 0.42, 0.42], 5.0),
  Mn(3, 'Phobos', 9376.0, 0.3189, 11.3, 0.0151, 30.0, [0.55, 0.5, 0.46], [0.35, 0.32, 0.3], 5.0),
  Mn(3, 'Deimos', 23463.0, 1.2624, 6.2, 0.0002, 200.0, [0.58, 0.53, 0.48], [0.38, 0.35, 0.32], 5.0),
  Mn(4, 'Io', 421800.0, 1.769138, 1821.6, 0.0041, 20.0, [0.92, 0.82, 0.45], [0.7, 0.55, 0.3], 1.0),
  Mn(4, 'Europa', 671100.0, 3.551181, 1560.8, 0.0094, 140.0, [0.85, 0.82, 0.75], [0.65, 0.6, 0.55], 5.0),
  Mn(4, 'Ganymede', 1070400.0, 7.154553, 2634.1, 0.0013, 260.0, [0.62, 0.58, 0.52], [0.45, 0.42, 0.38], 5.0),
  Mn(4, 'Callisto', 1882700.0, 16.689017, 2410.3, 0.0074, 80.0, [0.5, 0.46, 0.42], [0.32, 0.3, 0.28], 5.0),
  Mn(5, 'Mimas', 185540.0, 0.942422, 198.2, 0.0196, 10.0, [0.7, 0.7, 0.68], [0.5, 0.5, 0.48], 5.0),
  Mn(5, 'Enceladus', 238040.0, 1.370218, 252.1, 0.0047, 90.0, [0.92, 0.94, 0.96], [0.75, 0.78, 0.82], 5.0),
  Mn(5, 'Tethys', 294670.0, 1.887802, 531.1, 0.0001, 170.0, [0.78, 0.78, 0.76], [0.58, 0.58, 0.56], 5.0),
  Mn(5, 'Dione', 377420.0, 2.736915, 561.4, 0.0022, 250.0, [0.75, 0.75, 0.73], [0.55, 0.55, 0.53], 5.0),
  Mn(5, 'Rhea', 527070.0, 4.5175, 763.8, 0.001, 330.0, [0.72, 0.72, 0.7], [0.52, 0.52, 0.5], 5.0),
  Mn(5, 'Titan', 1221870.0, 15.945421, 2574.7, 0.0288, 60.0, [0.88, 0.68, 0.4], [0.7, 0.5, 0.3], 2.0),
  Mn(5, 'Iapetus', 3560840.0, 79.330183, 734.5, 0.0293, 120.0, [0.66, 0.62, 0.55], [0.25, 0.22, 0.2], 5.0),
  Mn(6, 'Miranda', 129900.0, 1.413479, 235.8, 0.0013, 15.0, [0.68, 0.7, 0.72], [0.5, 0.52, 0.54], 5.0),
  Mn(6, 'Ariel', 190900.0, 2.520379, 578.9, 0.0012, 100.0, [0.7, 0.72, 0.74], [0.52, 0.54, 0.56], 5.0),
  Mn(6, 'Umbriel', 266000.0, 4.144177, 584.7, 0.0039, 190.0, [0.5, 0.52, 0.55], [0.35, 0.37, 0.4], 5.0),
  Mn(6, 'Titania', 436300.0, 8.705872, 788.9, 0.0011, 280.0, [0.65, 0.67, 0.7], [0.48, 0.5, 0.52], 5.0),
  Mn(6, 'Oberon', 583500.0, 13.463239, 761.4, 0.0014, 340.0, [0.6, 0.6, 0.62], [0.44, 0.44, 0.46], 5.0),
  Mn(7, 'Triton', 354759.0, 5.876854, 1353.4, 0.00002, 50.0, [0.78, 0.8, 0.85], [0.6, 0.62, 0.68], 5.0),
  Mn(8, 'Charon', 19591.0, 6.38723, 606.0, 0.0002, 220.0, [0.6, 0.58, 0.56], [0.42, 0.4, 0.38], 5.0),
];
// Uranus's IAU pole points against its spin, and its moons orbit with the spin, so their
// orbits are nearly 180° from that pole. Kept identical to moon_plane_tilt_deg in src/sim.rs.
const URANUS = 6;
MOONS.forEach((m, i) => { m.incl = m.parent === URANUS ? 180.0 - MOON_INCL[i] : MOON_INCL[i]; });

// Earth's Moon with precessing node (18.6 yr) and perigee (8.85 yr) — the one moon
// whose place on its orbit isn't illustrative: about 1° from DE440s on average, about
// 3° at worst, from 1950 to 2050. Kept identical to luna_rel in src/sim.rs, whose tests
// hold it to that.
export function lunaRel(jd) {
  const d = jd - J2000_JD;
  const a = 384400.0e3;
  const e = 0.0549;
  const inc = 5.145 * DEG;
  const node = (125.08 - 0.0529538 * d) * DEG;
  const argp = (318.15 + 0.1643573 * d) * DEG;
  const ma = (135.27 + 13.064993 * d) * DEG;
  const ea = keplerE(ma % TAU, e);
  return perifocalToEcl(node, inc, argp,
    a * (Math.cos(ea) - e), a * Math.sqrt(1.0 - e * e) * Math.sin(ea));
}

// Moon position relative to its planet (m, root frame): Kepler ellipse in the
// parent's IAU equator, tilted by the moon's own inclination about the node.
export function moonRel(mi, jd) {
  const m = MOONS[mi];
  if (m.name === 'Moon') return lunaRel(jd);
  const ma = m.phase * DEG + TAU * (jd - J2000_JD) / m.period;
  const ea = keplerE(ma % TAU, m.e);
  const lx = m.a * (Math.cos(ea) - m.e);
  const ly = m.a * Math.sqrt(1.0 - m.e * m.e) * Math.sin(ea);
  const ci = Math.cos(m.incl * DEG), si = Math.sin(m.incl * DEG);
  return basisApply(bodyEquator(m.parent, jd), lx, ci * ly, si * ly);
}

// Sampled moon orbit relative to planet; the Moon samples the same precessing
// model as its position (line matches marker).
export function moonOrbit(mi, jd, n = 128) {
  const m = MOONS[mi];
  if (m.name === 'Moon') {
    const out = [];
    for (let k = 0; k < n; k++) out.push(lunaRel(jd + m.period * k / n));
    return out;
  }
  const eq = bodyEquator(m.parent, jd);
  const ci = Math.cos(m.incl * DEG), si = Math.sin(m.incl * DEG);
  const out = [];
  for (let k = 0; k < n; k++) {
    const ea = (k / n) * TAU;
    const lx = m.a * (Math.cos(ea) - m.e);
    const ly = m.a * Math.sqrt(1.0 - m.e * m.e) * Math.sin(ea);
    out.push(basisApply(eq, lx, ci * ly, si * ly));
  }
  return out;
}

// Tidally-locked moon basis: z = orbit normal from actual motion (so Triton's
// retrograde sense survives), x = toward the parent. Approximation, noted.
export function moonBasis(mi, jd) {
  const r0 = moonRel(mi, jd);
  const r1 = moonRel(mi, jd + 0.01);
  const z = new THREE.Vector3().crossVectors(r0, r1).normalize();
  const toParent = r0.multiplyScalar(-1).normalize();
  const y = new THREE.Vector3().crossVectors(z, toParent).normalize();
  const x = new THREE.Vector3().crossVectors(y, z);
  return { x, y, z };
}

// ----------------------------------------------------------- IAU rotation

// IAU 2015 rotation elements aligned with PLANETS: pole α0 (deg), α̇ (deg/cy),
// δ0 (deg), δ̇ (deg/cy), W0 (deg), Ẇ (deg/day). Small trigonometric series
// (Neptune pole nutation, lunar libration) omitted — documented approximation.
export const IAU_ROT = [
  [281.0103, -0.0328, 61.4155, -0.0049, 329.5988, 6.1385108],        // Mercury
  [272.76, 0.0, 67.16, 0.0, 160.20, -1.4813688],                     // Venus (retrograde)
  [0.0, -0.641, 90.0, -0.557, 190.147, 360.9856235],                 // Earth
  [317.269202, -0.109275, 54.432516, -0.058271, 176.049863, 350.891982443], // Mars
  [268.056595, -0.006499, 64.495303, 0.002413, 284.95, 870.536],     // Jupiter (Sys III)
  [40.589, -0.036, 83.537, -0.004, 38.90, 810.7939024],              // Saturn
  [257.311, 0.0, -15.175, 0.0, 203.81, -501.1600928],                // Uranus (retrograde)
  [299.36, 0.0, 43.46, 0.0, 249.978, 541.1397757],                   // Neptune
  [132.993, 0.0, -6.163, 0.0, 302.695, 56.3625225],                  // Pluto
];

function basisApply(b, vx, vy, vz) {
  return new THREE.Vector3(
    b.x.x * vx + b.y.x * vy + b.z.x * vz,
    b.x.y * vx + b.y.y * vy + b.z.y * vz,
    b.x.z * vx + b.y.z * vy + b.z.z * vz,
  );
}

// Body equatorial basis in the root frame: x = node direction Q (body equator
// × ICRS equator), y = Q rotated 90° in the equator, z = IAU pole. Moon orbits
// and rings live in this plane.
export function bodyEquator(pi, jd) {
  const r = IAU_ROT[pi];
  const t = (jd - J2000_JD) / 36525.0;
  const alpha = (r[0] + r[1] * t) * DEG;
  const delta = (r[2] + r[3] * t) * DEG;
  const cd = Math.cos(delta);
  const pole = icrsToEclVec(cd * Math.cos(alpha), cd * Math.sin(alpha), Math.sin(delta));
  const zIcrs = icrsToEclVec(0, 0, 1);
  const q = new THREE.Vector3().crossVectors(zIcrs, pole);
  if (q.lengthSq() < 1e-12) q.set(1, 0, 0);
  q.normalize();
  return { x: q, y: new THREE.Vector3().crossVectors(pole, q), z: pole };
}

// IAU prime-meridian angle W (radians) at jd.
export function bodyW(pi, jd) {
  const r = IAU_ROT[pi];
  return (r[4] + r[5] * (jd - J2000_JD)) * DEG;
}

// Full body-fixed basis: x = prime meridian direction (node rotated by W along
// the equator), z = pole. Surfaces (coastlines) use this.
export function bodyBasis(pi, jd) {
  const eq = bodyEquator(pi, jd);
  const w = bodyW(pi, jd);
  const x = eq.x.multiplyScalar(Math.cos(w)).addScaledVector(eq.y, Math.sin(w));
  return { x, y: new THREE.Vector3().crossVectors(eq.z, x), z: eq.z };
}

// ----------------------------------------------------------- ring systems

// zones: inner/outer edge (m from planet center), relative density 0..1.
export const RINGS = [
  {
    planet: 5, // Saturn
    zones: [
      { rIn: 74658.0e3, rOut: 92000.0e3, density: 0.35 },   // C
      { rIn: 92000.0e3, rOut: 117580.0e3, density: 1.0 },   // B
      { rIn: 122170.0e3, rOut: 136775.0e3, density: 0.7 },  // A (Cassini division gap before)
      { rIn: 140180.0e3, rOut: 140680.0e3, density: 0.25 }, // F
    ],
  },
  {
    planet: 6, // Uranus
    zones: [{ rIn: 51149.0e3, rOut: 51650.0e3, density: 0.3 }], // ε ring
  },
];

// -------------------------------------------------------------- self-check

{
  const jd = jdTdb(jdUtcNow());
  const iE = PLANETS.findIndex((p) => p.name === 'Earth');
  const iJ = PLANETS.findIndex((p) => p.name === 'Jupiter');
  const iM = MOONS.findIndex((m) => m.name === 'Moon');
  const rE = planetState(iE, jd).pos.length() / AU;
  console.assert(rE >= 0.9835 && rE <= 1.0170, `eph: Earth |pos| = ${rE.toFixed(4)} AU`);
  const rM = moonRel(iM, jd).length() / 1e3;
  console.assert(rM >= 356e3 && rM <= 407e3, `eph: Moon |rel| = ${rM.toFixed(0)} km`);
  const rJ = planetState(iJ, jd).pos.length() / AU;
  console.assert(rJ >= 4.9 && rJ <= 5.5, `eph: Jupiter |pos| = ${rJ.toFixed(3)} AU`);
  const tilt = bodyBasis(iE, jd).z.angleTo(new THREE.Vector3(0, 0, 1)) / DEG;
  console.assert(Math.abs(tilt - 23.44) < 0.1, `eph: Earth axial tilt = ${tilt.toFixed(3)} deg`);
}
