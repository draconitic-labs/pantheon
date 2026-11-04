// Pantheon 3D: the interface. A log ruler of places on the left, where-am-I at the
// bottom left, time at the bottom right, tools top right, a card when something is
// selected. All DOM is built here; main.js only calls the returned handle.
import { rec, CLS_COLOR } from './evidence.js';

const RATES = [
  { label: 'Real time', r: 1 },
  { label: '1 minute per second', r: 60 },
  { label: '1 hour per second', r: 3600 },
  { label: '1 day per second', r: 86400 },
  { label: '30 days per second', r: 2592000 },
  { label: '1 year per second', r: 31557600 },
];

const HELP = [
  ['Getting around', [
    ['Drag', 'Swing around what you are looking at'],
    ['Scroll or pinch', 'Zoom. Keep going, it reaches the cosmic web and back'],
    ['Click', 'Fly to anything with a name'],
    ['Right-drag', 'Look away without moving; Recenter brings it back'],
    ['Arrow keys', 'Swing, with + and − to zoom'],
  ]],
  ['Places', [
    ['/', 'Search'],
    ['H', 'Home to Earth'],
    ['B or Backspace', 'Back to where you were'],
    ['P', 'Grand tour'],
    ['Esc', 'Stop whatever is happening, or close this'],
  ]],
  ['Free flight', [
    ['F', 'Switch free flight on or off'],
    ['W A S D', 'Fly; Q and E down and up; Shift for speed'],
    ['Drag', 'Look around; Z and C roll'],
    ['Scroll', 'Flight speed'],
    ['L', 'Mouse-look, Esc releases the mouse'],
  ]],
  ['Time and sound', [
    ['Space', 'Pause or run time'],
    ['[ and ]', 'Slower or faster'],
    ['M', 'Ambient sound'],
    ['?', 'This help'],
  ]],
];

function el(tag, cls, parent, text) {
  const d = document.createElement(tag);
  if (cls) d.className = cls;
  if (text != null) d.textContent = text;
  if (parent) parent.appendChild(d);
  return d;
}
function btn(cls, parent, text, fn, title) {
  const b = el('button', cls, parent, text);
  b.type = 'button';
  if (title) b.title = title;
  b.addEventListener('click', (e) => { fn(e); if (e.detail > 0) b.blur(); });
  return b;
}
const root = (id) => document.getElementById(id);

export function initUI(cb) {
  // ---- evidence chips ------------------------------------------------------
  const tip = el('div', 'evtip', document.body);
  let tipFor = null;
  function showTip(chip) {
    const i = chip._info;
    tip.innerHTML = '';
    const add = (k, v) => {
      if (!v) return;
      const r = el('div', 'evtip-row', tip);
      el('span', 'evtip-k', r, k);
      el('span', 'evtip-v', r, String(v));
    };
    add('Source', i.source); add('Citation', i.citation); add('Frame', i.frame); add('Note', i.note); add('Fetched', i.fetched);
    tip.classList.add('show');
    tipFor = chip;
    const r = chip.getBoundingClientRect();
    let x = r.right - tip.offsetWidth, y = r.bottom + 6;
    if (y + tip.offsetHeight > innerHeight - 8) y = r.top - tip.offsetHeight - 6;
    x = Math.max(8, Math.min(x, innerWidth - tip.offsetWidth - 8));
    tip.style.left = `${x}px`; tip.style.top = `${y}px`;
  }
  function hideTip() { tip.classList.remove('show'); tipFor = null; }
  function evChip(key) {
    const info = rec(key);
    const c = el('span', 'evchip');
    el('span', 'evdot', c).style.background = CLS_COLOR[info.cls];
    el('span', 'evcls', c, info.cls);
    c._info = info;
    c.addEventListener('pointerenter', () => showTip(c));
    c.addEventListener('pointerleave', hideTip);
    c.addEventListener('pointerdown', (e) => { e.preventDefault(); e.stopPropagation(); if (tipFor === c) hideTip(); else showTip(c); });
    return c;
  }

  // ---- the ruler of places ---------------------------------------------------
  const rail = root('rail');
  const railLine = el('div', 'rail-line', rail);
  const railMark = el('div', 'rail-mark', railLine);
  const rungs = cb.places.map((p, i) => {
    const b = btn('rung', rail, p.name, () => cb.goPlace(p.id), `Fly to ${p.name}`);
    b.style.order = String(cb.places.length - i);
    if (p.minor) b.dataset.minor = '1';
    return b;
  });
  railLine.style.order = '100';
  let railI = -1;
  let railStops = null;
  const measureRail = () => {
    railStops = [];
    rungs.forEach((r, i) => { if (r.offsetParent !== null) railStops.push([i, r.offsetTop + r.offsetHeight / 2]); });
  };
  window.addEventListener('resize', () => { railStops = null; });
  function setRail(pos) {   // pos: fractional index into places, 0 = home
    if (!railStops) measureRail();
    if (railStops.length > 1) {
      let k = 0;
      while (k < railStops.length - 2 && railStops[k + 1][0] < pos) k++;
      const [ia, ya] = railStops[k], [ib, yb] = railStops[k + 1];
      const f = Math.min(1, Math.max(0, (pos - ia) / (ib - ia)));
      railMark.style.transform = `translateY(${(ya + (yb - ya) * f).toFixed(1)}px)`;
    }
    const i = Math.round(pos);
    const near = Math.abs(pos - i) < 0.34 ? i : -1;
    if (near !== railI) {
      rungs.forEach((r, k) => r.classList.toggle('here', k === near));
      railI = near;
    }
  }

  // ---- where am I ------------------------------------------------------------
  const place = root('place');
  const title = el('div', 'place-title', place);
  const sub = el('div', 'place-sub', place);
  const fact = el('div', 'place-fact', place);
  const actions = el('div', 'place-actions', place);
  const backBtn = btn('act', actions, 'Back', () => cb.goBack(), 'Back to where you were (B)');
  btn('act', actions, 'Home', () => cb.goHome(), 'Home to Earth (H)');
  const recBtn = btn('act', actions, 'Recenter', () => cb.recenter(), 'Face what you are orbiting again');
  const stopBtn = btn('act act-strong', actions, 'Stop', () => cb.stop(), 'Stop the flight (Esc)');
  recBtn.hidden = true; stopBtn.hidden = true;
  const scale = el('div', 'scalebar', place);
  const scaleLine = el('div', 'scale-line', scale);
  const scaleText = el('div', 'scale-text', scale);
  const truthWrap = el('div', 'truth', place);
  const truthBtn = el('button', 'truth-btn', truthWrap);
  truthBtn.type = 'button';
  const truthDot = el('span', 'evdot', truthBtn);
  const truthText = el('span', '', truthBtn);
  const truthList = el('div', 'panel truth-list', truthWrap);
  truthList.hidden = true;
  truthBtn.addEventListener('click', () => { truthList.hidden = !truthList.hidden; lastLayersKey = ''; });
  let lastLayersKey = '';
  const last = {};
  const setText = (node, key, v) => { if (last[key] !== v) { node.textContent = v; last[key] = v; } };

  function setPlace({ name, distance, speed, back, flying, lookedAway }) {
    setText(title, 'name', name);
    setText(sub, 'sub', speed ? `${distance}, moving at ${speed}` : distance);
    backBtn.disabled = !back;
    recBtn.hidden = !lookedAway;
    stopBtn.hidden = !flying;
  }
  let factT = 0;
  function showFact(text, ms = 9000) {
    clearTimeout(factT);
    fact.textContent = text || '';
    fact.classList.toggle('on', !!text);
    title.classList.remove('arrive');
    void title.offsetWidth;
    title.classList.add('arrive');
    if (text) factT = setTimeout(() => fact.classList.remove('on'), ms);
  }
  function setScale(px, label) {
    if (last.sbp !== Math.round(px)) { scaleLine.style.width = `${Math.round(px)}px`; last.sbp = Math.round(px); }
    setText(scaleText, 'sbl', label);
  }
  function setTruth(summary, cls, layers) {
    setText(truthText, 'truth', summary);
    if (last.tcls !== cls) { truthDot.style.background = CLS_COLOR[cls]; last.tcls = cls; }
    const key = layers.map(l => l.ev + l.label).join('|');
    if (truthList.hidden || key === lastLayersKey) return;
    lastLayersKey = key;
    truthList.innerHTML = '';
    for (const l of layers) {
      const r = el('div', 'card-row', truthList);
      el('span', 'card-label', r, l.label);
      r.appendChild(evChip(l.ev));
    }
  }

  // ---- card ------------------------------------------------------------------
  const cardRoot = root('card');
  const liveRows = new Map();
  function showCard({ title: t, kind, blurb, rows = [], note, fly }) {
    hideTip();
    cardRoot.innerHTML = '';
    liveRows.clear();
    const p = el('div', 'panel card', cardRoot);
    const head = el('div', 'card-head', p);
    el('div', 'card-title', head, t);
    if (kind) el('div', 'card-kind', head, kind);
    btn('card-close', head, '×', hideCard, 'Close');
    if (blurb) el('p', 'card-blurb', p, blurb);
    const body = el('div', 'card-rows', p);
    for (const row of rows) {
      const r = el('div', 'card-row', body);
      el('span', 'card-label', r, row.label);
      const v = el('span', 'card-value', r, row.value);
      if (row.live) liveRows.set(row.live, v);
      if (row.ev) r.appendChild(evChip(row.ev));
    }
    if (note) el('div', 'card-note', p, note);
    if (fly) btn('act act-strong card-fly', p, `Fly to ${t}`, fly);
  }
  function hideCard() { hideTip(); cardRoot.innerHTML = ''; liveRows.clear(); cb.cardClosed && cb.cardClosed(); }
  function setLive(key, v) { const n = liveRows.get(key); if (n && n.textContent !== v) n.textContent = v; }

  // ---- labels: keyed, fade rather than pop ------------------------------------
  const labelLayer = root('labels');
  const labelMap = new Map();
  const labelFree = [];
  function renderLabels(items) {
    const seen = new Set();
    for (const it of items) {
      seen.add(it.id);
      let d = labelMap.get(it.id);
      if (!d) {
        d = labelFree.pop() || el('div', 'lbl', labelLayer);
        d.textContent = it.text;
        d.className = `lbl lbl-${it.size || 'm'} lbl-${it.kind || 'body'}`;
        d._gone = 0;
        d.style.display = '';
        labelMap.set(it.id, d);
        d.style.opacity = '0';
        requestAnimationFrame(() => { if (labelMap.get(it.id) === d) d.style.opacity = String(d._a ?? 1); });
      }
      d._gone = 0;
      if (d.textContent !== it.text) d.textContent = it.text;
      const a = it.alpha == null ? 1 : it.alpha;
      if (d._a !== a) { d._a = a; d.style.opacity = String(a); }
      const x = Math.round(it.x), y = Math.round(it.y);
      if (d._x !== x || d._y !== y) {
        d.style.transform = `translate(${x}px,${y}px) translate(-50%,-100%)`;
        d._x = x; d._y = y;
      }
    }
    for (const [id, d] of labelMap) {
      if (seen.has(id)) continue;
      if (!d._gone) { d._gone = performance.now(); d.style.opacity = '0'; d._a = 0; }
      else if (performance.now() - d._gone > 220) {
        d.style.display = 'none';
        labelMap.delete(id);
        labelFree.push(d);
      }
    }
  }

  // ---- toast -----------------------------------------------------------------
  const toastRoot = root('toast');
  let toastT = 0;
  function toast(text, ms = 2600) {
    toastRoot.textContent = text;
    toastRoot.classList.add('show');
    clearTimeout(toastT);
    toastT = setTimeout(() => toastRoot.classList.remove('show'), ms);
  }

  // ---- time ------------------------------------------------------------------
  const bar = root('timebar');
  const play = btn('tbtn tplay', bar, 'Pause', () => cb.togglePause(), 'Pause or run time (Space)');
  const slider = el('input', 'trate', bar);
  slider.type = 'range'; slider.min = '0'; slider.max = String(RATES.length - 1); slider.step = '1';
  slider.title = 'How fast time runs';
  slider.setAttribute('aria-label', 'How fast time runs');
  slider.addEventListener('input', () => cb.setRate(RATES[+slider.value].r));
  slider.addEventListener('pointerup', () => slider.blur());
  const rateText = el('span', 'trate-text', bar);
  const dateOut = el('span', 'tdate', bar);
  btn('tbtn', bar, 'Now', () => cb.goNow(), 'Jump to the present moment');
  function setRateActive(r, paused) {
    let i = RATES.findIndex(x => x.r === r);
    if (i < 0) i = 1;
    slider.value = String(i);
    rateText.textContent = paused ? 'Time paused' : RATES[i].label;
    play.textContent = paused ? 'Run' : 'Pause';
  }
  function setDate(s) { setText(dateOut, 'date', s); }
  function stepRate(r, dir) {
    let i = RATES.findIndex(x => x.r === r);
    if (i < 0) i = 1;
    return RATES[Math.min(RATES.length - 1, Math.max(0, i + dir))].r;
  }

  const creditEl = root('credit');
  function setCredit(t) { setText(creditEl, 'credit', t); }

  // ---- tools -----------------------------------------------------------------
  const tools = root('tools');
  const tourBtn = btn('tool', tools, 'Grand tour', () => cb.tour(), 'A guided trip from the Moon to the cosmic web (P)');
  const freeBtn = btn('tool', tools, 'Free flight', () => cb.freeFlight(), 'Fly yourself with W A S D (F)');
  const soundBtn = btn('tool', tools, 'Sound off', () => cb.audio(), 'Ambient sound (M)');
  const qualBtn = btn('tool', tools, 'Quality', () => { qualPanel.hidden = !qualPanel.hidden; }, 'Rendering quality');
  btn('tool', tools, 'Help', () => setHelp(!helpOpen), 'Controls (?)');
  const qualPanel = el('div', 'panel qual', tools);
  qualPanel.hidden = true;
  const qualRows = new Map();
  for (const [id, name] of [['auto', 'Automatic'], ['ultra', 'Ultra'], ['high', 'High'], ['medium', 'Medium'], ['low', 'Low']]) {
    const b = btn('qual-row', qualPanel, name, () => cb.setQuality(id));
    qualRows.set(id, b);
  }
  const qualRead = el('div', 'qual-read', qualPanel);
  function setQuality(mode, tierLabel) {
    for (const [id, b] of qualRows) b.classList.toggle('on', id === mode);
    qualBtn.textContent = mode === 'auto' ? `Quality: ${tierLabel}` : `Quality: ${tierLabel}, fixed`;
  }
  function setFrameRead(s) { if (!qualPanel.hidden) setText(qualRead, 'qr', s); }
  function setFreeOn(on) {
    if (last.free === on) return;
    last.free = on;
    freeBtn.classList.toggle('on', on);
    freeBtn.textContent = on ? 'Leave free flight' : 'Free flight';
  }
  function setTourOn(on) { tourBtn.classList.toggle('on', on); tourBtn.textContent = on ? 'End tour' : 'Grand tour'; }
  function setSound(on) { soundBtn.classList.toggle('on', on); soundBtn.textContent = on ? 'Sound on' : 'Sound off'; }

  // ---- tour bar, banners, hint, notice ------------------------------------------
  const tourBar = root('tourbar');
  const tourText = el('span', 'tour-text', tourBar);
  btn('tbtn', tourBar, 'Skip', () => cb.tourSkip(), 'Next stop');
  btn('tbtn', tourBar, 'End tour', () => cb.tour(), 'End the tour (Esc)');
  function setTourStep(s) { tourBar.classList.toggle('show', !!s); if (s) tourText.textContent = s; }
  const banner = root('banner');
  function setBanner(s) { banner.classList.toggle('show', !!s); if (s) banner.textContent = s; }

  const hint = root('hint');
  let hintDone = false;
  try { hintDone = localStorage.getItem('pantheon.hint') === 'done'; } catch { /* private mode */ }
  function showHint() {
    if (hintDone) return;
    hint.textContent = 'Drag to swing around Earth. Scroll to zoom all the way out. Click anything named to fly there.';
    hint.classList.add('show');
    setTimeout(dismissHint, 16000);
  }
  function dismissHint() {
    if (!hint.classList.contains('show') && hintDone) return;
    hint.classList.remove('show');
    hintDone = true;
    try { localStorage.setItem('pantheon.hint', 'done'); } catch { /* private mode */ }
  }

  const noticeRoot = root('notice');
  function setNotice(lines) {
    noticeRoot.innerHTML = '';
    if (!lines.length) return;
    const n = el('div', 'panel notice', noticeRoot);
    el('div', 'notice-title', n, 'Running with less data than usual');
    for (const l of lines) el('p', 'notice-line', n, l);
    btn('tbtn', n, 'Got it', () => { noticeRoot.innerHTML = ''; });
  }

  // ---- loader -----------------------------------------------------------------
  const loader = root('loader');
  function setLoading(text) { const n = loader.querySelector('.load-text'); if (n) n.textContent = text; }
  function hideLoader() { loader.classList.add('gone'); setTimeout(() => loader.remove(), 1600); }

  // ---- search -----------------------------------------------------------------
  const sbox = el('div', 'panel search', root('searchwrap'));
  const input = el('input', 'sinput', sbox);
  input.type = 'text';
  input.placeholder = 'Search planets, stars, galaxies';
  input.setAttribute('aria-label', 'Search');
  input.autocomplete = 'off';
  input.spellcheck = false;
  const list = el('div', 'sresults', sbox);
  list.hidden = true;
  let results = [], sel = -1;
  function paintSel() {
    const rows = list.children;
    for (let i = 0; i < rows.length; i++) rows[i].classList.toggle('sel', i === sel);
  }
  function searchResults(items, empty) {
    results = (items || []).slice(0, 10);
    sel = results.length ? 0 : -1;
    list.innerHTML = '';
    results.forEach((res, i) => {
      const row = el('div', `srow${i === sel ? ' sel' : ''}`, list);
      el('span', 'sname', row, res.name);
      if (res.detail) el('span', 'sdetail', row, res.detail);
      row.addEventListener('pointerdown', (e) => { e.preventDefault(); pickResult(res); });
      row.addEventListener('pointerenter', () => { sel = i; paintSel(); });
    });
    if (!results.length && empty) el('div', 'sempty', list, empty);
    list.hidden = !results.length && !empty;
  }
  function pickResult(res) { cb.pick(res); hideSearch(); }
  function openSearch() {
    if (document.exitPointerLock) document.exitPointerLock();
    input.focus();
    input.select();
  }
  function hideSearch() { input.value = ''; searchResults([]); input.blur(); }
  const isTyping = () => document.activeElement === input;
  const runSearch = () => {
    const q = input.value.trim();
    searchResults(q ? cb.search(q) : [], q ? `Nothing called “${q}” in the catalogs` : null);
  };
  input.addEventListener('input', runSearch);
  input.addEventListener('focus', runSearch);
  input.addEventListener('blur', () => { list.hidden = true; });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.stopPropagation(); hideSearch(); }
    else if (e.key === 'Enter') { e.stopPropagation(); if (sel >= 0) pickResult(results[sel]); }
    else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault(); e.stopPropagation();
      if (results.length) {
        sel = (sel + (e.key === 'ArrowDown' ? 1 : results.length - 1)) % results.length;
        paintSel();
      }
    }
  });

  // ---- hover tip + selection ring ------------------------------------------------
  const hoverTip = el('div', 'hovertip', labelLayer);
  hoverTip.style.display = 'none';
  function setHover(h) {
    if (!h) { if (hoverTip.style.display !== 'none') hoverTip.style.display = 'none'; return; }
    hoverTip.style.display = '';
    if (hoverTip.textContent !== h.text) hoverTip.textContent = h.text;
    hoverTip.style.transform = `translate(${h.x + 14}px,${h.y + 12}px)`;
  }
  const reticle = el('div', 'reticle', labelLayer);
  reticle.style.display = 'none';
  function setReticle(rt) {
    if (!rt) { if (reticle.style.display !== 'none') reticle.style.display = 'none'; return; }
    reticle.style.display = '';
    const r = Math.round(rt.r);
    reticle.style.transform = `translate(${Math.round(rt.x - r)}px,${Math.round(rt.y - r)}px)`;
    if (reticle._r !== r) { reticle.style.width = reticle.style.height = `${r * 2}px`; reticle._r = r; }
  }

  // ---- help -----------------------------------------------------------------------
  const helpRoot = root('help');
  const hp = el('div', 'panel help-panel', helpRoot);
  el('h2', 'help-title', hp, 'How to get around');
  const cols = el('div', 'help-cols', hp);
  for (const [name, rows] of HELP) {
    const sec = el('section', 'help-sec', cols);
    el('h3', 'help-h', sec, name);
    const g = el('div', 'help-grid', sec);
    for (const [k, v] of rows) { el('span', 'help-k', g, k); el('span', 'help-v', g, v); }
  }
  el('p', 'help-credits', hp, 'Sky photograph: ESO/S. Brunier, CC BY 4.0. Surface and ring maps: Solar System Scope, from NASA data, CC BY 4.0. Stars: ESA/Gaia/DPAC, CC BY-NC 3.0 IGO. Exoplanets: NASA Exoplanet Archive. Positions: NASA/JPL. See NOTICE.');
  btn('act act-strong help-close', hp, 'Close', () => setHelp(false));
  let helpOpen = false;
  function setHelp(open) {
    helpOpen = !!open;
    helpRoot.classList.toggle('open', helpOpen);
  }
  helpRoot.addEventListener('pointerdown', (e) => { if (e.target === helpRoot) setHelp(false); });

  // ---- chrome rests when you stop touching it --------------------------------------
  let idleT = 0;
  const wake = () => {
    document.body.classList.remove('idle');
    clearTimeout(idleT);
    idleT = setTimeout(() => document.body.classList.add('idle'), 7000);
  };
  for (const ev of ['pointermove', 'pointerdown', 'keydown', 'wheel']) window.addEventListener(ev, wake, { passive: true });
  wake();

  // ---- global keys that belong to the UI ---------------------------------------------
  window.addEventListener('keydown', (e) => {
    if (isTyping() || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === '/') { e.preventDefault(); setHelp(false); openSearch(); }
    else if (e.key === '?') { e.preventDefault(); setHelp(!helpOpen); }
  });

  return {
    setPlace, showFact, setScale, setTruth, setRail,
    showCard, hideCard, setLive, renderLabels, toast,
    setRateActive, setDate, setCredit, stepRate, setQuality, setFrameRead, setFreeOn, setTourOn, setSound,
    setTourStep, setBanner, showHint, dismissHint, setNotice, setLoading, hideLoader,
    isTyping, hideSearch, helpOpen: () => helpOpen, setHelp, closePanels: () => {
      const open = !qualPanel.hidden || !truthList.hidden;
      qualPanel.hidden = true; truthList.hidden = true;
      return open;
    },
    setHover, setReticle,
  };
}
