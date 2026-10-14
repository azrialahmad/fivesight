// Forwards events from Meet tabs to the fivesight app on this PC (127.0.0.1 only).
const ENDPOINT = 'http://127.0.0.1:47285';
const VERSION = chrome.runtime.getManifest().version;

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

// Tells the app this extension is installed and paired, with or without a Meet tab open.
const hello = () => post('/events', { events: [{ type: 'hello', version: VERSION }] });

// Chrome wakes the worker for alarms; 30 s is the shortest period it allows.
chrome.alarms.create('hello', { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener((a) => a.name === 'hello' && hello());
chrome.runtime.onStartup.addListener(hello);
chrome.runtime.onInstalled.addListener(hello);

chrome.runtime.onMessage.addListener((msg, _sender, reply) => {
  if (msg && Array.isArray(msg.events)) {
    // A hello from a Meet tab also says that tab is alive, which is what "in a call" relies on.
    const events = msg.events.map((e) => (e.type === 'hello' ? { ...e, version: VERSION, tab: true } : e));
    post('/events', { events }).then(reply);
    return true;
  }
  if (msg && msg.ping) {
    hello().then(reply);
    return true;
  }
  return false;
});
