// Forwards events from Meet tabs to the fivesight app on this PC (127.0.0.1 only).
const ENDPOINT = 'http://127.0.0.1:47285';

async function token() {
  const v = await chrome.storage.local.get('token');
  return (v.token || '').trim();
}

async function post(path, body) {
  const t = await token();
  if (!t) return { ok: false, reason: 'no-token' };
  try {
    const res = await fetch(ENDPOINT + path, {
      method: 'POST',
      // text/plain keeps this a "simple" request, so there is no preflight to answer.
      headers: { Authorization: 'Bearer ' + t, 'Content-Type': 'text/plain' },
      body: body ? JSON.stringify(body) : ''
    });
    if (res.status === 401) return { ok: false, reason: 'wrong-token' };
    return res.ok ? { ok: true } : { ok: false, reason: 'http-' + res.status };
  } catch (_) {
    return { ok: false, reason: 'not-running' };
  }
}

chrome.runtime.onMessage.addListener((msg, _sender, reply) => {
  if (msg && Array.isArray(msg.events)) {
    post('/events', { events: msg.events }).then(reply);
    return true;
  }
  if (msg && msg.ping) {
    post('/ping').then(reply);
    return true;
  }
  return false;
});
