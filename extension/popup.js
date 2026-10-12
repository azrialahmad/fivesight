const $ = (id) => document.getElementById(id);
const status = $('status');

function show(text, kind) {
  status.className = kind || '';
  status.textContent = '';
  if (kind === 'ok') {
    const dot = document.createElement('span');
    dot.className = 'dot';
    status.appendChild(dot);
  }
  status.appendChild(document.createTextNode(text));
}

const messages = {
  'no-token': ['Paste the pairing code from fivesight first.', 'bad'],
  'wrong-token': ['That code was not accepted. Copy it again from fivesight.', 'bad'],
  'not-running': ['fivesight is not running. Open the app, then press Connect.', 'bad']
};

function ping() {
  chrome.runtime.sendMessage({ ping: true }, (r) => {
    if (chrome.runtime.lastError || !r) return show('Could not reach the extension worker.', 'bad');
    if (r.ok) return show('Connected to fivesight', 'ok');
    const m = messages[r.reason];
    show(m ? m[0] : 'fivesight answered with an error (' + r.reason + ').', 'bad');
  });
}

chrome.storage.local.get(['token', 'debug'], (v) => {
  $('token').value = v.token || '';
  $('debug').checked = !!v.debug;
  if (v.token) ping();
});

$('connect').addEventListener('click', () => {
  const token = $('token').value.trim();
  chrome.storage.local.set({ token }, ping);
});

$('debug').addEventListener('change', (e) => chrome.storage.local.set({ debug: e.target.checked }));
