// Pantheon 3D test hooks, active only with ?qa in the URL. Page errors go to the
// backend's log, and the window runs scripts the backend hands it (PANTHEON_QA=1),
// so tests can read camera state and frame times from inside the real window.
(() => {
  if (!new URLSearchParams(location.search).has('qa')) return;
  const send = (s) => fetch('/api/qa/log', { method: 'POST', body: s }).catch(() => {});
  for (const k of ['error', 'warn', 'log']) {
    const orig = console[k];
    console[k] = (...a) => {
      send(`${k}: ${a.map(x => (typeof x === 'string' ? x : (() => { try { return JSON.stringify(x); } catch { return String(x); } })())).join(' ')}`);
      orig.apply(console, a);
    };
  }
  addEventListener('error', e => send(`uncaught: ${e.message} @ ${e.filename}:${e.lineno}`));
  addEventListener('unhandledrejection', e => send(`rejection: ${e.reason && (e.reason.stack || e.reason.message || e.reason)}`));
  const Async = (async () => {}).constructor;
  const run = async (js) => {
    let fn;
    try { fn = new Async(`return (${js}\n)`); } catch { fn = new Async(js); }
    return fn();
  };
  (async () => {
    for (;;) {
      try {
        const j = await (await fetch('/api/qa/poll')).json();
        if (j.js === undefined) continue;
        let out;
        try { out = { ok: await run(j.js) }; } catch (e) { out = { err: String(e && (e.stack || e)) }; }
        let body;
        try { body = JSON.stringify(out); } catch { body = JSON.stringify({ ok: String(out.ok) }); }
        await fetch(`/api/qa/result?id=${j.id}`, { method: 'POST', body: body || '{}' });
      } catch { await new Promise(r => setTimeout(r, 500)); }
    }
  })();
})();
