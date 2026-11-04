// Pantheon 3D: GLSL for solid bodies. Everything is shaded in the body's object
// space with the planet radius as the unit (object +Y = pole, +X = prime
// meridian, -Z = 90 deg east), which keeps f32 precision from the coastline to
// the far side of the system. Sun and camera arrive as uniforms in that space.

export const NOISE = /* glsl */`
  float hash1(vec3 p) {
    p = fract(p * 0.3183099 + vec3(0.1, 0.17, 0.13));
    p *= 17.0;
    return fract(p.x * p.y * p.z * (p.x + p.y + p.z));
  }
  float vnoise(vec3 p) {
    vec3 i = floor(p), f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(
      mix(mix(hash1(i), hash1(i + vec3(1,0,0)), f.x),
          mix(hash1(i + vec3(0,1,0)), hash1(i + vec3(1,1,0)), f.x), f.y),
      mix(mix(hash1(i + vec3(0,0,1)), hash1(i + vec3(1,0,1)), f.x),
          mix(hash1(i + vec3(0,1,1)), hash1(i + vec3(1,1,1)), f.x), f.y), f.z);
  }
  float fbm(vec3 p) {
    float a = 0.5, s = 0.0;
    for (int i = 0; i < 5; i++) { s += a * vnoise(p); p *= 2.03; a *= 0.5; }
    return s;
  }
  float ign(vec2 p) { return fract(52.9829189 * fract(dot(p, vec2(0.06711056, 0.00583715)))); }
`;

// Equirect lookup with Tarini's seam fix: two u parameterizations, whichever has
// the smaller derivative, so mip selection never jumps at lon 180.
export const LATLON = /* glsl */`
  vec2 latlonUv(vec3 n) {
    float x = atan(-n.z, n.x) * 0.15915494;
    float uA = fract(x + 0.5), uB = fract(x + 1.0) - 0.5;
    float u = fwidth(uA) <= fwidth(uB) + 1e-5 ? uA : uB;
    return vec2(u, asin(clamp(n.y, -1.0, 1.0)) * 0.31830989 + 0.5);
  }
`;

export const BODY_VSH = /* glsl */`
  varying vec3 vObj;
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vObj = position;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
    #include <logdepthbuf_vertex>
  }
`;

// uKind: 1 rocky, 2 gas, 3 ice, 4 earth.  uLight: 0 lambert, 1 regolith, 2 cloud-top.
export const BODY_FSH = /* glsl */`
  varying vec3 vObj;
  uniform int uKind, uLight, uNOcc;
  uniform vec3 uColA, uColB, uSunObj, uCamObj, uSunTint;
  uniform vec4 uParam;     // x polar cap, y detail, z band freq, w storm
  uniform float uTime, uSeed, uClose, uSunI, uAmb, uFlat, uBump, uCloudLon, uSunAng;
  uniform sampler2D uMap, uNight, uClouds, uRingMap;
  uniform float uHasMap, uHasNight, uHasClouds, uHasRing, uRingIn, uRingOut;
  uniform vec2 uMapTexel;
  uniform vec4 uOcc[4];    // eclipsers: xyz object space, w radius
  #include <common>
  #include <logdepthbuf_pars_fragment>
  ${NOISE}
  ${LATLON}
  float lum(vec3 c) { return dot(c, vec3(0.2126, 0.7152, 0.0722)); }
  float cloudAt(vec3 n, float lod) {
    float x = atan(-n.z, n.x) * 0.15915494 + uCloudLon;
    return textureLod(uClouds, vec2(fract(x + 0.5), asin(clamp(n.y, -1.0, 1.0)) * 0.31830989 + 0.5), lod).r;
  }
  void main() {
    #include <logdepthbuf_fragment>
    vec3 n = normalize(vObj);
    vec3 P = vObj * vec3(1.0, uFlat, 1.0);
    vec3 N = normalize(vObj * vec3(1.0, 1.0 / uFlat, 1.0));
    vec3 V = normalize(uCamObj - P);
    vec3 L = uSunObj;
    vec2 tuv = latlonUv(n);
    float lat = asin(clamp(n.y, -1.0, 1.0));
    float lon = atan(-n.z, n.x);
    vec3 p = n * uParam.y + vec3(uSeed);
    vec3 alb;
    float water = 0.0;
    if (uHasMap > 0.5) {
      alb = texture2D(uMap, tuv).rgb;
      if (uKind == 4) water = smoothstep(1.25, 2.4, alb.b / (max(alb.r, alb.g) + 0.004));
    } else if (uKind == 2) {
      float turb = fbm(vec3(lon * 1.5, lat * 3.0, uSeed)) - 0.5;
      float band = sin(lat * uParam.z + turb * 3.4) * 0.5 + 0.5;
      float fine = fbm(vec3(lon * 4.0, lat * uParam.z * 1.7, uSeed + 4.0));
      alb = mix(uColA, uColB, band) * (0.82 + 0.36 * fine);
      if (uParam.w > 0.0) {
        vec2 d = vec2((lon - 0.6) * cos(lat) / 0.26, (lat + 0.38) / 0.11);
        alb = mix(alb, vec3(0.78, 0.42, 0.28), exp(-dot(d, d)) * uParam.w);
      }
    } else if (uKind == 3) {
      float band = sin(lat * uParam.z) * 0.5 + 0.5;
      alb = mix(uColA, uColB, lat * 0.5 + 0.5) * (0.9 + 0.12 * band + 0.08 * fbm(p * 0.8));
    } else {
      float a1 = fbm(p * 3.0);
      alb = mix(uColA, uColB, a1) * (0.72 + 0.55 * fbm(p * 9.0 + 7.0));
      if (uParam.x > 0.0) {
        float cap = smoothstep(uParam.x, uParam.x + 0.07, abs(n.y) + 0.05 * (a1 - 0.5));
        alb = mix(alb, vec3(0.93, 0.94, 0.96), cap);
      }
    }
    // synthetic close-range detail (labeled in the card): only near the ground
    if (uClose > 0.002 && uKind != 2 && uKind != 3) {
      float f1 = fbm(p * 40.0 + 11.0), f2 = fbm(p * 170.0 + 23.0);
      alb *= 1.0 + uClose * (1.0 - 0.7 * water) * (0.34 * (f1 - 0.5) + 0.26 * (f2 - 0.5));
    } else if (uClose > 0.002) {
      alb *= 1.0 + uClose * 0.18 * (fbm(vec3(lon * 26.0, lat * 90.0, uSeed + 7.0)) - 0.5);
    }
    // relief from the imagery itself: mild, and absent over open water
    vec3 Ns = N;
    if (uBump > 0.0 && uHasMap > 0.5) {
      vec3 east = normalize(vec3(-n.z, 0.0, -n.x) + vec3(1e-5));
      vec3 north = cross(n, east);
      float h0 = lum(alb);
      float hE = lum(texture2D(uMap, tuv + vec2(uMapTexel.x, 0.0)).rgb);
      float hN = lum(texture2D(uMap, tuv + vec2(0.0, uMapTexel.y)).rgb);
      Ns = normalize(N - (east * (hE - h0) + north * (hN - h0)) * uBump * (1.0 - water));
    }
    float NL = dot(Ns, L), NV = max(dot(N, V), 0.0);
    float nl = max(NL, 0.0);
    float lit;
    if (uLight == 1) lit = 1.75 * nl / (nl + NV + 0.12) * smoothstep(-0.01, 0.05, NL);
    else if (uLight == 2) lit = pow(nl, 0.86) * pow(max(NV, 0.02), 0.14) * smoothstep(-0.02, 0.06, NL);
    else lit = nl * smoothstep(-0.03, 0.05, NL);
    // Saturn's rings shadow the cloud tops; radial profile from the ring strip
    if (uHasRing > 0.5 && abs(L.y) > 1e-4) {
      float t = -P.y / L.y;
      if (t > 0.0) {
        vec3 h = P + L * t;
        float r = length(h.xz);
        if (r > uRingIn && r < uRingOut)
          lit *= 1.0 - 0.85 * texture2D(uRingMap, vec2((r - uRingIn) / (uRingOut - uRingIn), 0.5)).a;
      }
    }
    // eclipses: real geometry from the ephemeris, penumbra from the Sun's size
    for (int i = 0; i < uNOcc; i++) {
      vec3 oc = uOcc[i].xyz - P;
      float tc = dot(oc, L);
      if (tc > 0.0) {
        float dd = length(oc - L * tc), r = uOcc[i].w;
        float pen = max(r * 0.1, tc * uSunAng);
        lit *= smoothstep(r - pen, r + pen, dd);
      }
    }
    float cl = 0.0;
    vec3 night = vec3(0.0);
    vec3 spec = vec3(0.0);
    if (uHasClouds > 0.5) {
      // the shell above draws the clouds; here they cast their shadow
      cl = cloudAt(normalize(n + L * 0.008), 1.5);
      lit *= 1.0 - 0.55 * smoothstep(0.15, 0.9, cl);
    }
    if (uHasNight > 0.5) {
      float dark = 1.0 - smoothstep(-0.14, 0.06, NL);
      vec3 li = texture2D(uNight, tuv).rgb;
      night = li * vec3(1.0, 0.86, 0.62) * dark * 3.2 * (1.0 - 0.75 * cloudAt(n, 1.0));
    }
    if (uKind == 4 && water > 0.0 && NL > 0.0) {
      // sun glint and a sky-tinted fresnel sheen on open water
      vec3 H = normalize(L + V);
      vec3 Nw = normalize(N + (vec3(vnoise(n * 420.0), vnoise(n * 420.0 + 5.0), vnoise(n * 420.0 + 9.0)) - 0.5) * 0.035);
      float nh = max(dot(Nw, H), 0.0);
      float fres = 0.02 + 0.98 * pow(1.0 - max(dot(H, V), 0.0), 5.0);
      spec = vec3(1.0, 0.94, 0.82) * water * (pow(nh, 420.0) * 1.1 + pow(nh, 70.0) * 0.05) * (0.3 + fres) * smoothstep(0.0, 0.12, NL);
      alb += vec3(0.02, 0.05, 0.1) * water * pow(1.0 - NV, 4.0);
    }
    vec3 sun = uSunI * mix(uSunTint, vec3(1.0), smoothstep(-0.02, 0.32, nl));
    // engraved graticule marks surfaces without imagery as synthetic
    float grat = 0.0;
    if (uHasMap < 0.5) {
      float gLat = abs(fract(lat * 57.29578 / 15.0 + 0.5) - 0.5);
      float gLon = abs(fract(lon * 57.29578 / 15.0 + 0.5) - 0.5) * cos(lat);
      grat = (1.0 - smoothstep(0.0, 0.02, min(gLat, gLon))) * 0.018;
    }
    vec3 col = alb * (lit * sun + uAmb) + night + spec * sun * (1.0 - smoothstep(0.1, 0.7, cl))
      + vec3(0.85, 0.70, 0.42) * grat * (0.25 + nl);
    gl_FragColor = vec4(col, 1.0);
  }
`;

// Single-scattering atmosphere, marched per pixel in units of the body radius.
// Output is premultiplied: in-scattered light plus mean transmittance in alpha.
export const ATMO_FSH = /* glsl */`
  varying vec3 vObj;
  uniform vec3 uCamObj, uSunObj, uBetaR, uBetaO, uBetaM;
  uniform float uHr, uHm, uG, uTop, uSunE;
  uniform int uN, uNL;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  ${NOISE}
  vec2 rs(vec3 o, vec3 d, float r) {
    float b = dot(o, d), c = dot(o, o) - r * r, h = b * b - c;
    if (h < 0.0) return vec2(1e9, -1e9);
    h = sqrt(h);
    return vec2(-b - h, -b + h);
  }
  void main() {
    #include <logdepthbuf_fragment>
    vec3 o = uCamObj;
    vec3 d = normalize(vObj * uTop - o);
    vec3 L = uSunObj;
    vec2 ta = rs(o, d, uTop);
    float t0 = max(ta.x, 0.0), t1 = ta.y;
    if (t1 <= t0) discard;
    // the mesh is faceted: stop just under the true sphere so no bright sliver of
    // air shows between the two at the limb
    vec2 tp = rs(o, d, 0.9992);
    if (tp.x > 0.0) t1 = min(t1, tp.x);
    float dt = (t1 - t0) / float(uN);
    float j = ign(gl_FragCoord.xy);
    float mu = dot(d, L);
    float pr = 0.0596831 * (1.0 + mu * mu);
    float g2 = uG * uG;
    float pm = 0.1193662 * (1.0 - g2) * (1.0 + mu * mu) / ((2.0 + g2) * pow(max(1.0 + g2 - 2.0 * uG * mu, 1e-4), 1.5));
    vec3 ir = vec3(0.0), im = vec3(0.0);
    float odR = 0.0, odM = 0.0;
    for (int i = 0; i < uN; i++) {
      vec3 q = o + d * (t0 + dt * (float(i) + j));
      float h = max(length(q) - 1.0, 0.0);
      float rR = exp(-h / uHr) * dt, rM = exp(-h / uHm) * dt;
      odR += rR; odM += rM;
      if (rs(q, L, 1.0).x > 0.0) continue;   // this parcel is in the planet's shadow
      float tl = rs(q, L, uTop).y, dl = tl / float(uNL);
      float lR = 0.0, lM = 0.0;
      for (int k = 0; k < uNL; k++) {
        float hl = max(length(q + L * dl * (float(k) + 0.5)) - 1.0, 0.0);
        lR += exp(-hl / uHr); lM += exp(-hl / uHm);
      }
      vec3 tau = uBetaR * (odR + lR * dl) + uBetaO * (odR + lR * dl) + uBetaM * 1.1 * (odM + lM * dl);
      vec3 T = exp(-tau);
      ir += T * rR; im += T * rM;
    }
    vec3 col = uSunE * (ir * uBetaR * pr + im * uBetaM * pm);
    vec3 Tv = exp(-(uBetaR + uBetaO) * odR - uBetaM * 1.1 * odM);
    gl_FragColor = vec4(col, 1.0 - dot(Tv, vec3(0.3333)));
  }
`;

// Earth's cloud shell, a separate layer so it parallaxes and clears the limb.
export const CLOUD_FSH = /* glsl */`
  varying vec3 vObj;
  uniform vec3 uSunObj;
  uniform sampler2D uClouds;
  uniform float uCloudLon, uSunI, uClose;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  ${NOISE}
  void main() {
    #include <logdepthbuf_fragment>
    vec3 n = normalize(vObj);
    float x = atan(-n.z, n.x) * 0.15915494 + uCloudLon;
    float uA = fract(x + 0.5), uB = fract(x + 1.0) - 0.5;
    float u = fwidth(uA) <= fwidth(uB) + 1e-5 ? uA : uB;
    float c = texture2D(uClouds, vec2(u, asin(clamp(n.y, -1.0, 1.0)) * 0.31830989 + 0.5)).r;
    if (uClose > 0.02) c *= 1.0 + uClose * (0.9 * (fbm(n * 90.0) - 0.5) + 1.1 * (fbm(n * 620.0) - 0.5));
    float a = smoothstep(0.1, 0.82, c);
    if (a < 0.004) discard;
    float NL = dot(n, uSunObj);
    float w = clamp((NL + 0.14) / 1.14, 0.0, 1.0);
    vec3 tint = mix(vec3(1.0, 0.5, 0.28), vec3(1.0), smoothstep(0.0, 0.3, NL));
    gl_FragColor = vec4(vec3(0.92, 0.93, 0.95) * (w * w * uSunI * tint + 0.004) * a, a);
  }
`;

export const RING_VSH = /* glsl */`
  varying vec3 vP;
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vP = position;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
    #include <logdepthbuf_vertex>
  }
`;
export const RING_FSH = /* glsl */`
  varying vec3 vP;
  uniform vec3 uSunObj, uCamObj;
  uniform sampler2D uMap;
  uniform float uInner, uOuter, uSunI, uFlat, uClose;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  ${NOISE}
  void main() {
    #include <logdepthbuf_fragment>
    float r = length(vP.xy);
    vec4 t = texture2D(uMap, vec2((r - uInner) / (uOuter - uInner), 0.5));
    float a = t.a;
    // fine grain only where the screen can resolve it, or it beats into moire
    float grainW = fwidth(r) * 900.0;
    if (uClose > 0.001 && grainW < 1.0) a *= 1.0 + uClose * (1.0 - smoothstep(0.25, 1.0, grainW)) * (vnoise(vec3(r * 900.0, 0.0, 0.0)) - 0.5) * 0.5;
    if (a <= 0.004 || r < uInner || r > uOuter) discard;
    vec3 L = uSunObj;
    vec3 V = normalize(uCamObj - vec3(vP.xy, 0.0));
    // the planet's shadow across the rings (an ellipsoid seen from the Sun)
    vec3 P = vec3(vP.xy, 0.0);
    float b = dot(P, L);
    float sh = 1.0;
    if (b < 0.0) {
      float sq = length(P - L * b);
      float limb = mix(1.0, uFlat, abs(L.z));
      sh = smoothstep(limb * 0.975, limb * 1.015, sq);
    }
    // lit face vs. backlit: thin rings glow when the Sun is behind them
    bool litSide = (L.z > 0.0) == (uCamObj.z > 0.0);
    float inc = abs(L.z);
    float surface = litSide ? (0.9 + 1.6 * inc) : (1.0 - a) * a * 5.0;
    float fwd = 1.0 + 2.2 * pow(max(dot(-V, L), 0.0), 6.0);
    vec3 col = t.rgb * surface * fwd * uSunI * sh;
    gl_FragColor = vec4(col * a + vec3(0.0015) * a, a);
  }
`;

// The Sun, and any star up close (uHasMap = 0, tinted by temperature).
export const STAR_FSH = /* glsl */`
  varying vec3 vObj;
  uniform vec3 uCamObj, uTint;
  uniform sampler2D uMap;
  uniform float uHasMap, uTime, uClose, uI;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  ${NOISE}
  ${LATLON}
  // granulation: 2x2x2 cellular noise, bright cells with dark intergranular lanes
  float cells(vec3 p) {
    vec3 i = floor(p), f = p - i;
    float d1 = 8.0, d2 = 8.0;
    for (int z = 0; z < 2; z++) for (int y = 0; y < 2; y++) for (int x = 0; x < 2; x++) {
      vec3 c = vec3(float(x), float(y), float(z));
      vec3 o = vec3(hash1(i + c), hash1(i + c + 11.3), hash1(i + c + 23.7));
      vec3 r = c + o - f;
      float d = dot(r, r);
      if (d < d1) { d2 = d1; d1 = d; } else if (d < d2) d2 = d;
    }
    return smoothstep(0.0, 0.42, sqrt(d2) - sqrt(d1));
  }
  void main() {
    #include <logdepthbuf_fragment>
    vec3 n = normalize(vObj);
    vec3 V = normalize(uCamObj - n);
    float mu = max(dot(n, V), 0.0);
    float t = uTime * 0.012;
    vec3 w = vec3(fbm(n * 5.0 + vec3(0.0, t, 0.0)), fbm(n * 5.0 + vec3(3.1, -t, 1.7)), 0.0) - 0.5;
    float T;
    if (uHasMap > 0.5) {
      vec2 uv = latlonUv(normalize(n + w * 0.012));
      T = dot(texture2D(uMap, uv).rgb, vec3(0.3, 0.6, 0.1));
      T = 0.34 + 0.42 * smoothstep(0.05, 0.85, T);
    } else {
      T = fbm(n * 7.0 + w * 2.0) * 1.15;
    }
    float g1 = fbm(n * 32.0 + w * 3.0 + vec3(t * 2.0));
    T = T * (0.72 + 0.56 * g1);
    if (uClose > 0.01) {
      float g2 = cells(n * 70.0 + w * 2.0 + vec3(0.0, 0.0, t * 6.0));
      float g3 = cells(n * 240.0 + vec3(0.0, 0.0, t * 18.0));
      T *= 1.0 + uClose * (0.34 * (g2 - 0.55) + 0.42 * (g3 - 0.55));
    }
    // hot-to-cool ramp keyed to the star's own color
    // a G star runs orange to pale gold; a hot star stays near its own blue-white
    float h = smoothstep(0.72, 1.25, uTint.b / uTint.r);
    vec3 cool = uTint * mix(vec3(1.0, 0.34, 0.08), vec3(0.8, 0.84, 0.92), h);
    vec3 warm = uTint * mix(vec3(1.0, 0.62, 0.24), vec3(0.94, 0.96, 1.0), h);
    vec3 hot = mix(uTint, vec3(1.0), 0.5) * mix(vec3(1.0, 0.88, 0.62), vec3(1.0), h);
    vec3 col = mix(cool, warm, smoothstep(0.15, 0.55, T));
    col = mix(col, hot, smoothstep(0.55, 0.95, T));
    float I = uI * (0.4 + 1.2 * T);
    // limb darkening I(mu) with a redder limb
    float limb = 1.0 - 0.72 * (1.0 - mu) - 0.16 * (1.0 - mu * mu);
    col = mix(cool * 0.9, col, smoothstep(0.0, 0.7, mu));
    gl_FragColor = vec4(col * I * max(limb, 0.08), 1.0);
  }
`;

// Camera-facing corona: streamers keyed to 3D direction so they stay attached
// to the star instead of swinging with the view. Illustrative, and labeled so.
export const CORONA_VSH = /* glsl */`
  varying vec2 vP;
  uniform float uSpan;
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vP = uv * 2.0 - 1.0;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
    #include <logdepthbuf_vertex>
  }
`;
export const CORONA_FSH = /* glsl */`
  varying vec2 vP;
  uniform vec3 uRight, uUp, uTint;
  uniform float uSpan, uTime, uI, uDisc;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  ${NOISE}
  void main() {
    #include <logdepthbuf_fragment>
    vec2 p = vP * uSpan;
    float r = length(p);
    if (r >= uSpan || r < 0.99) discard;
    vec3 dir = normalize(uRight * p.x + uUp * p.y);
    float t = uTime * 0.02;
    float s1 = fbm(dir * 2.6 + vec3(t * 0.3));
    float s2 = fbm(dir * 7.5 - vec3(t * 0.5) + r * 0.09);
    float streamer = pow(clamp(s1 * 1.3 - 0.12, 0.0, 1.0), 2.0) * (0.6 + 0.8 * s2);
    float x = max(r - 1.0, 0.0);
    float inner = 0.95 / pow(1.0 + x * 6.0, 3.4);
    float outer = (0.03 + 0.34 * streamer) / pow(1.0 + x * 0.9, 2.7);
    vec3 col = mix(vec3(1.0, 0.86, 0.68), vec3(0.8, 0.88, 1.0), smoothstep(0.1, 3.5, x)) * (inner + outer);
    // chromosphere and prominences hugging the limb
    float fl = fbm(vec3(dir * 22.0 + vec3(0.0, t * 1.4, 0.0)));
    float h = 0.006 + 0.075 * pow(clamp(fl * 1.6 - 0.5, 0.0, 1.0), 2.2);
    float prom = smoothstep(h, h * 0.15, x);
    col += uTint * vec3(1.0, 0.3, 0.18) * prom * 2.2;
    col *= smoothstep(uSpan, uSpan * 0.55, r) * uI;
    gl_FragColor = vec4(col * uDisc, 1.0);
  }
`;

// Screen-sized glare around a bright star when its disc is small on screen.
export const GLARE_FSH = /* glsl */`
  varying vec2 vP;
  uniform vec3 uTint;
  uniform float uI;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  void main() {
    #include <logdepthbuf_fragment>
    float d = length(vP);
    if (d >= 1.0) discard;
    float g = exp(-d * 15.0) * 1.8 + exp(-d * 5.0) * 0.24 + exp(-d * 2.2) * 0.04;
    float sp = exp(-abs(vP.x) * 70.0) * exp(-abs(vP.y) * 5.0) + exp(-abs(vP.y) * 70.0) * exp(-abs(vP.x) * 5.0);
    g += sp * 0.22;
    g *= smoothstep(1.0, 0.7, d);
    gl_FragColor = vec4(mix(uTint, vec3(1.0), 0.2) * g * uI, 1.0);
  }
`;

export const ORBIT_VSH = /* glsl */`
  attribute float t;
  varying float vT;
  varying float vNear;
  uniform float uNear;
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vT = t;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    vNear = smoothstep(0.0, 1.0, length(mv.xyz / uNear));
    gl_Position = projectionMatrix * mv;
    #include <logdepthbuf_vertex>
  }
`;
export const ORBIT_FSH = /* glsl */`
  varying float vT;
  varying float vNear;
  uniform vec3 uCol;
  uniform float uHead, uGain;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  void main() {
    #include <logdepthbuf_fragment>
    float d = fract(uHead - vT);
    float a = 0.1 + 0.62 * pow(1.0 - d, 3.0);
    gl_FragColor = vec4(uCol * a * uGain * vNear, 1.0);
  }
`;
