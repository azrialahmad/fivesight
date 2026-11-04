// Runs on meet.google.com. Watches for a call, for who is speaking and for chat messages, and
// tells the background worker, which forwards it to the fivesight app on this PC. It adds nothing
// to the page.
(function () {
  const S = self.FSSelectors;
  const T = self.FSTracker;
  const TICK_MS = 200;
  const HELLO_MS = 15000;
  const CHAT_MS = 1000;
  const CHAT_BATCH = 20;

  let debug = false;
  try {
    chrome.storage.local.get('debug', (v) => (debug = !!(v && v.debug)));
    chrome.storage.onChanged.addListener((c) => c.debug && (debug = !!c.debug.newValue));
  } catch (_) {}
  const log = (...a) => debug && console.log('[fivesight]', ...a);

  const speaking = T.createSpeakingTracker();
  const call = T.createCallTracker();
  const chat = T.createChatTracker();
  let ownName = null;
  let timers = [];
  let dead = false;

  function send(events) {
    if (dead || events.length === 0) return;
    try {
      chrome.runtime.sendMessage({ events }, () => void chrome.runtime.lastError);
    } catch (_) {
      shutdown(); // the extension was reloaded; this copy of the script is orphaned
    }
  }

  function shutdown() {
    dead = true;
    timers.forEach(clearInterval);
    observer.disconnect();
  }

  // Count changes inside each participant tile. A speaker's tile animates its audio indicator;
  // the video itself never touches the DOM, so quiet tiles stay quiet.
  const observer = new MutationObserver((records) => {
    const now = Date.now();
    for (const r of records) {
      if (r.type !== 'attributes') continue;
      const el = r.target;
      if (!(el instanceof Element) || el.closest('video')) continue;
      const tile = el.closest('[data-participant-id]');
      if (tile) speaking.note(S.tileId(tile), now);
    }
  });
  observer.observe(document.documentElement, { subtree: true, attributes: true });

  function tick() {
    const now = Date.now();
    const inCall = S.inCall(document, location.pathname);
    const change = call.tick(inCall);
    if (change === 'joined') {
      const ev = { type: 'call', state: 'joined', title: S.callTitle(document), code: S.meetCode(location.pathname) };
      log('joined', ev);
      send([ev]);
    } else if (change === 'left') {
      log('left');
      send([{ type: 'call', state: 'left' }]);
      send(speaking.reset(now).map(toEvent));
      chat.reset();
    }
    if (!call.inCall) return;

    ownName = S.ownName(document) || ownName;
    const present = new Map();
    for (const tile of S.tiles(document)) {
      const name = S.tileName(tile);
      if (!name || name === ownName || tile.querySelector('[data-self-name]')) continue; // your own voice is the mic stream
      present.set(S.tileId(tile), name);
    }
    const edges = speaking.tick(now, present);
    if (edges.length) {
      log('speaking', edges, 'tiles:', Array.from(present.values()));
      send(edges.map(toEvent));
    }
  }

  const toEvent = (e) => ({ type: 'speaking', name: e.name, speaking: e.speaking, at: e.at });

  // Chat messages, while the chat panel is open (Meet doesn't draw them otherwise).
  function readChat() {
    if (!call.inCall) return;
    const now = Date.now();
    const found = S.chatMessages(document).map((m) => ({
      ...m,
      at: S.parseChatTime(m.time, now),
      me: S.isOwnSender(m.sender, ownName)
    }));
    const fresh = chat.fresh(now, found);
    if (!fresh.length) return;
    log('chat', fresh);
    const events = fresh.map((m) => ({
      type: 'chat',
      id: m.id,
      sender: m.me ? ownName : m.sender,
      text: m.text,
      at: m.at,
      me: m.me
    }));
    for (let i = 0; i < events.length; i += CHAT_BATCH) send(events.slice(i, i + CHAT_BATCH));
  }

  timers.push(setInterval(tick, TICK_MS));
  timers.push(setInterval(readChat, CHAT_MS));
  // Hellos carry your Meet name once it's known, so the app can suggest it as yours.
  const hello = () => send([{ type: 'hello', me: ownName }]);
  timers.push(setInterval(hello, HELLO_MS));
  hello();

  addEventListener('pagehide', () => {
    if (call.leaveNow()) send([{ type: 'call', state: 'left' }]);
  });
})();
