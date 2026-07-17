// Pantheon 3D — UI layer: HUD chip, info card, label pool, time bar, search, help, toast.
// All DOM lives under the empty containers in index.html; DOM is only touched
// inside initUI() so the module can be imported headlessly.
import { rec, CLS_COLOR } from './evidence.js';

const RATES = [
  { label: '❚❚', r: 0 },
  { label: 'real',    r: 1 },
  { label: '1 min/s', r: 60 },
  { label: '1 hr/s',  r: 3600 },
  { label: '1 day/s', r: 86400 },
  { label: '30 d/s',  r: 2592000 },
  { label: '1 yr/s',  r: 31557600 },   // 365.25 d
];

const HELP_ROWS = [
  ['click canvas', 'mouse-look (pointer lock)'],
  ['W A S D',      'fly'],
  ['R / F',        'up / down'],
  ['Shift',        'boost ×8'],
  ['scroll',       'speed'],
  ['click',        'select'],
  ['double-click', 'travel to'],
  ['G',            'travel to selection'],
  ['H',            'home'],
  ['/',            'search'],
  ['Backspace',    'back'],
  ['T',            'pause time'],
  ['X',            'stop riding the focused body'],
  ['P',            'grand tour'],
  ['M',            'ambience'],
  ['?',            'help'],
  ['Esc',          'release mouse'],
];

const LABEL_MAX = 64;
const CHIP_KEYS = ['focus', 'dist', 'speed', 'date', 'src'];

function el(tag, cls, parent, text) {
  const d = document.createElement(tag);
  if (cls) d.className = cls;
  if (text != null) d.textContent = text;
  if (parent) parent.appendChild(d);
  return d;
}

export function initUI(cb) {
  // ---- HUD chip -------------------------------------------------------
  const chip = el('div', 'panel chip', document.getElementById('hud'));
  chip.style.display = 'none';
  const segs = {};
  for (const k of CHIP_KEYS) segs[k] = el('span', 'chip-seg chip-' + k, chip);

  function setChip(s) {
    let any = false;
    for (const k of CHIP_KEYS) {
      const v = s[k];
      const seg = segs[k];
      if (v) {
        any = true;
        if (seg._v !== v) { seg.textContent = v; seg._v = v; }
        if (seg._h) { seg.style.display = ''; seg._h = false; }
      } else if (!seg._h) { seg.style.display = 'none'; seg._h = true; }
    }
    chip.style.display = any ? '' : 'none';
    if (s.date && dateOut._v !== s.date) { dateOut.textContent = s.date; dateOut._v = s.date; }
  }

  // ---- evidence tooltip (shared, fixed-position so card scroll can't clip it)
  const tip = el('div', 'evtip', document.body);
  let tipFor = null;
  function showTip(chipEl) {
    const i = chipEl._info;
    tip.innerHTML = '';
    const add = (k, v) => {
      if (!v) return;
      const r = el('div', 'evtip-row', tip);
      el('span', 'evtip-k', r, k);
      el('span', 'evtip-v', r, String(v));
    };
    if (i) {
      add('source', i.source); add('citation', i.citation); add('frame', i.frame);
      add('note', i.note); add('fetched', i.fetched);
    } else add('key', chipEl._key);
    tip.classList.add('show');
    tipFor = chipEl;
    const r = chipEl.getBoundingClientRect();
    let x = r.right - tip.offsetWidth, y = r.bottom + 6;
    if (y + tip.offsetHeight > innerHeight - 8) y = r.top - tip.offsetHeight - 6;
    x = Math.max(8, Math.min(x, innerWidth - tip.offsetWidth - 8));
    tip.style.left = x + 'px';
    tip.style.top = y + 'px';
  }
  function hideTip() { tip.classList.remove('show'); tipFor = null; }

  function evChip(key) {
    let info = null;
    try { info = rec(key); } catch { /* unknown key: plain chip */ }
    const cls = (info && info.cls) || 'modeled';
    const c = el('span', 'evchip');
    el('span', 'evdot', c).style.background = CLS_COLOR[cls] || CLS_COLOR.modeled;
    el('span', 'evcls', c, cls);
    c._info = info;
    c._key = key;
    c.addEventListener('pointerenter', () => showTip(c));
    c.addEventListener('pointerleave', hideTip);
    c.addEventListener('pointerdown', e => {          // press = toggle (touch)
      e.preventDefault(); e.stopPropagation();
      tipFor === c ? hideTip() : showTip(c);
    });
    return c;
  }

  // ---- card -----------------------------------------------------------
  const cardRoot = document.getElementById('card');
  function showCard({ title, kind, rows = [], note }) {
    hideTip();
    cardRoot.innerHTML = '';
    const p = el('div', 'panel card', cardRoot);
    const head = el('div', 'card-head', p);
    el('div', 'card-title', head, title);
    if (kind) el('div', 'card-kind', head, kind);
    el('button', 'card-close', head, '×').onclick = hideCard;
    const body = el('div', 'card-rows', p);
    for (const row of rows) {
      const r = el('div', 'card-row', body);
      el('span', 'card-label', r, row.label);
      el('span', 'card-value', r, row.value);
      if (row.ev) r.appendChild(evChip(row.ev));
    }
    if (note) el('div', 'card-note', p, note);
  }
  function hideCard() { hideTip(); cardRoot.innerHTML = ''; }

  // ---- label pool (max LABEL_MAX divs, reused every frame) -------------
  const labelLayer = document.getElementById('labels');
  const pool = [];
  function renderLabels(items) {
    const n = Math.min(items.length, LABEL_MAX);
    while (pool.length < n) {
      const d = el('div', 'lbl', labelLayer);
      d._t = ''; d._c = ''; d._h = false;
      pool.push(d);
    }
    for (let i = 0; i < n; i++) {
      const it = items[i], d = pool[i];
      if (d._t !== it.text) { d.textContent = it.text; d._t = it.text; }
      const c = 'lbl lbl-' + (it.size || 'm') + ' lbl-' + (it.kind || 'body');
      if (d._c !== c) { d.className = c; d._c = c; }
      d.style.transform = 'translate3d(' + it.x + 'px,' + it.y + 'px,0) translate(-50%,-130%)';
      d.style.opacity = it.alpha == null ? 1 : it.alpha;
      if (d._h) { d.style.display = ''; d._h = false; }
    }
    for (let i = n; i < pool.length; i++) {
      if (!pool[i]._h) { pool[i].style.display = 'none'; pool[i]._h = true; }
    }
  }

  // ---- toast ------------------------------------------------------------
  const toastRoot = document.getElementById('toast');
  const toastBox = el('div', 'panel toast', toastRoot);
  let toastT = 0;
  function toast(text) {
    toastBox.textContent = text;
    toastRoot.classList.add('show');
    clearTimeout(toastT);
    toastT = setTimeout(() => toastRoot.classList.remove('show'), 2500);
  }

  // ---- time bar ---------------------------------------------------------
  const bar = el('div', 'panel timebar', document.getElementById('timebar'));
  const rateBtns = new Map();
  for (const { label, r } of RATES) {
    const b = el('button', 'tbtn rate', bar, label);
    b.addEventListener('click', e => {
      cb.setRate(r); setRateActive(r);
      if (e.detail > 0) b.blur();      // mouse click: release focus so keys keep flying
    });
    rateBtns.set(r, b);
  }
  el('span', 'tsep', bar);
  const mkBtn = (label, fn) => {
    const b = el('button', 'tbtn', bar, label);
    b.addEventListener('click', e => { fn(); if (e.detail > 0) b.blur(); });
  };
  mkBtn('− day', () => cb.stepDays(-1));
  mkBtn('+ day', () => cb.stepDays(1));
  mkBtn('now', () => cb.goNow());
  if (cb.tour) mkBtn('⟡ tour', () => cb.tour());
  if (cb.audio) mkBtn('♪', () => cb.audio());
  el('span', 'tsep', bar);
  const dateOut = el('span', 'tdate', bar);

  function setRateActive(r) {
    for (const [k, b] of rateBtns) b.classList.toggle('active', k === r);
  }

  // ---- search -----------------------------------------------------------
  const sbox = el('div', 'panel search', document.getElementById('searchwrap'));
  const input = el('input', 'sinput', sbox);
  input.type = 'text';
  input.placeholder = '/ search the atlas';
  input.autocomplete = 'off';
  input.spellcheck = false;
  const list = el('div', 'sresults', sbox);
  list.style.display = 'none';
  let results = [], sel = -1;

  function paintSel() {
    const rows = list.children;
    for (let i = 0; i < rows.length; i++) rows[i].classList.toggle('sel', i === sel);
  }
  function searchResults(items) {
    results = (items || []).slice(0, 12);
    sel = results.length ? 0 : -1;
    list.innerHTML = '';
    results.forEach((res, i) => {
      const row = el('div', 'srow' + (i === sel ? ' sel' : ''), list);
      el('span', 'sname', row, res.name);
      if (res.kind) el('span', 'skind', row, res.kind);
      if (res.detail) el('span', 'sdetail', row, res.detail);
      row.addEventListener('pointerdown', e => { e.preventDefault(); pickResult(res); });
      row.addEventListener('pointerenter', () => { sel = i; paintSel(); });
    });
    list.style.display = results.length ? '' : 'none';
  }
  function pickResult(res) { cb.pick(res); hideSearch(); }
  function openSearch() {
    if (document.exitPointerLock) document.exitPointerLock();
    input.focus();
    input.select();
  }
  function hideSearch() {
    input.value = '';
    searchResults([]);
    input.blur();
  }
  const isTyping = () => document.activeElement === input;

  input.addEventListener('input', () => {
    const q = input.value.trim();
    searchResults(q ? cb.search(q) : []);
  });
  input.addEventListener('focus', () => {
    const q = input.value.trim();
    if (q) searchResults(cb.search(q));
  });
  input.addEventListener('blur', () => { list.style.display = 'none'; });
  // Owned keys only — everything else falls through so typing is never swallowed.
  input.addEventListener('keydown', e => {
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

  // ---- help overlay -------------------------------------------------------
  const helpRoot = document.getElementById('help');
  const hp = el('div', 'panel help-panel', helpRoot);
  el('div', 'help-title', hp, 'Controls');
  const grid = el('div', 'help-grid', hp);
  for (const [k, v] of HELP_ROWS) {
    el('span', 'help-k', grid, k);
    el('span', 'help-v', grid, v);
  }
  el('div', 'help-hint', hp, 'press ? to close');
  let helpOpen = false;
  function setHelp(open) {
    helpOpen = !!open;
    helpRoot.classList.toggle('open', helpOpen);
  }
  helpRoot.addEventListener('pointerdown', e => { if (e.target === helpRoot) setHelp(false); });

  // ---- global keys (never act while the search input is focused) ----------
  window.addEventListener('keydown', e => {
    if (isTyping()) return;
    if (e.key === '/') { e.preventDefault(); setHelp(false); openSearch(); }
    else if (e.key === '?') { e.preventDefault(); setHelp(!helpOpen); }
    else if (e.key === 'Escape' && helpOpen) setHelp(false);
  });

  return {
    setChip, showCard, hideCard, renderLabels, toast, setRateActive, setHelp,
    isTyping, searchResults, hideSearch,
  };
}
