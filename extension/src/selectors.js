// Everything that knows what Google Meet's page looks like lives in this file.
// Meet changes its markup without notice; when speaker names stop appearing, fix it here.
// Turn on "Debug logging" in the extension popup to see what these functions find.
(function (root) {
  // Meet serves calls at /abc-defg-hij and also under /meet/abc-defg-hij.
  const CODE = /^(?:\/meet)?\/([a-z]{3}-[a-z]{4}-[a-z]{3})(?:$|[/?])/;

  /** The meeting code from the address, or null on Meet's home page and other screens. */
  function meetCode(pathname) {
    const m = CODE.exec(pathname || '');
    return m ? m[1] : null;
  }

  /** One tile per participant in the grid or filmstrip. Meet tags each with an id attribute. */
  function tiles(doc) {
    return Array.from(doc.querySelectorAll('[data-participant-id]'));
  }

  function tileId(tile) {
    return tile.getAttribute('data-participant-id');
  }

  /** Your own display name, which Meet exposes on your own tile. */
  function ownName(doc) {
    const el = doc.querySelector('[data-self-name]');
    return (el && el.getAttribute('data-self-name')) || null;
  }

  function isIconText(el) {
    // Meet draws icons with a font; the "text" is the icon's name (mic_off, push_pin, ...).
    const fam = (el.ownerDocument.defaultView.getComputedStyle(el).fontFamily || '').toLowerCase();
    return fam.includes('material') || fam.includes('icon');
  }

  /** Notices Meet shows on tiles ("Others might still see your full video.") read like sentences; names don't. */
  function looksLikeName(text) {
    return !/[.!?]$/.test(text) && text.split(/\s+/).length <= 6;
  }

  /** The display name shown on a tile: the first plain text label that isn't a button, icon or notice. */
  function tileName(tile) {
    const walker = tile.ownerDocument.createTreeWalker(tile, NodeFilter.SHOW_ELEMENT);
    for (let el = walker.nextNode(); el; el = walker.nextNode()) {
      if (el.childElementCount > 0) continue;
      if (el.closest('button, [role="button"], [aria-hidden="true"], video')) continue;
      const text = (el.textContent || '').trim();
      if (!text || text.length > 80) continue;
      if (isIconText(el)) continue;
      if (!looksLikeName(text)) continue;
      return text;
    }
    return null;
  }

  /** The red phone button only exists while you are in a call, in every UI language. */
  function hasLeaveButton(doc) {
    for (const el of doc.querySelectorAll('button i, button span')) {
      if (el.childElementCount === 0 && (el.textContent || '').trim() === 'call_end') return true;
    }
    return !!doc.querySelector('button[aria-label*="Leave call" i], button[aria-label*="Tinggalkan" i]');
  }

  /** In a call: a meeting address, and either the leave button or at least two participant tiles. */
  function inCall(doc, pathname) {
    if (!meetCode(pathname)) return false;
    return hasLeaveButton(doc) || tiles(doc).length >= 2;
  }

  /** The meeting's name if it has one, not just its code. */
  function callTitle(doc) {
    let t = (doc.title || '').trim();
    t = t.replace(/^Meet\s*[-–—]\s*/i, '').replace(/\s*[-–—]\s*Google Meet$/i, '').trim();
    if (!t || /^Meet$/i.test(t) || /^[a-z]{3}-[a-z]{4}-[a-z]{3}$/i.test(t)) return null;
    return t;
  }

  // ---------- chat ----------

  /** "10:42", "10:42 AM", "22.05": the time Meet prints over a group of chat messages. */
  const TIME = /^(\d{1,2})[:.](\d{2})(?:\s*([AaPp])\.?\s*[Mm]\.?)?$/;

  /**
   * When a chat header time ("10:42 AM") was, as epoch ms, on the day of `now`; a time later
   * than now is from yesterday. Null if `text` isn't a time.
   */
  function parseChatTime(text, now) {
    const m = TIME.exec((text || '').trim());
    if (!m) return null;
    let h = Number(m[1]);
    const min = Number(m[2]);
    if (m[3]) {
      if (h < 1 || h > 12) return null;
      h = (h % 12) + (/p/i.test(m[3]) ? 12 : 0);
    }
    if (h > 23 || min > 59) return null;
    const d = new Date(now);
    d.setHours(h, min, 0, 0);
    let at = d.getTime();
    if (at - now > 60000) at -= 24 * 60 * 60 * 1000;
    return at;
  }

  /** Meet calls your own chat messages "You" (or the same in the UI language). */
  function isOwnSender(sender, own) {
    const s = (sender || '').trim();
    return /^(you|anda)$/i.test(s) || (!!own && s === own);
  }

  /** Visible text inside `el`, skipping buttons, icons and anything hidden from screen readers. */
  function plainText(el, skip) {
    const parts = [];
    const walker = el.ownerDocument.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const p = n.parentElement;
      if (!p || p.closest('button, [role="button"], [aria-hidden="true"]') || isIconText(p)) continue;
      if (skip && skip(p)) continue;
      const t = n.textContent.trim();
      if (t) parts.push(t);
    }
    return parts.join(' ').replace(/\s+/g, ' ').trim();
  }

  /**
   * The sender and time over a message: the nearest enclosing group with text that isn't part of
   * a message. Meet tags groups with data-sender-name in some versions; otherwise the header is
   * the group's first non-message labels (name, then time).
   */
  function chatHeader(msg) {
    let el = msg.parentElement;
    for (let depth = 0; el && depth < 6; depth++, el = el.parentElement) {
      const tagged = el.getAttribute('data-sender-name');
      const labels = [];
      const walker = el.ownerDocument.createTreeWalker(el, NodeFilter.SHOW_ELEMENT);
      for (let e = walker.nextNode(); e && labels.length < 4; e = walker.nextNode()) {
        if (e.childElementCount > 0 || e.closest('[data-message-id]')) continue;
        if (e.closest('button, [role="button"], [aria-hidden="true"]') || isIconText(e)) continue;
        const t = (e.textContent || '').trim();
        if (t && t.length <= 80) labels.push(t);
      }
      if (!tagged && labels.length === 0) continue;
      const time = labels.find((t) => TIME.test(t)) || null;
      const sender = tagged || labels.find((t) => !TIME.test(t) && looksLikeName(t)) || null;
      return { sender, time };
    }
    return { sender: null, time: null };
  }

  /**
   * Messages in the chat panel, oldest first: { id, sender, time, text }. Meet only draws them
   * while the panel is open; each carries a stable data-message-id.
   */
  function chatMessages(doc) {
    const out = [];
    for (const msg of doc.querySelectorAll('[data-message-id]')) {
      // A message nested in another tagged element is the same message drawn twice.
      if (msg.parentElement && msg.parentElement.closest('[data-message-id]')) continue;
      const text = plainText(msg);
      if (!text) continue;
      const { sender, time } = chatHeader(msg);
      out.push({ id: msg.getAttribute('data-message-id'), sender, time, text });
    }
    return out;
  }

  const api = {
    meetCode,
    tiles,
    tileId,
    ownName,
    looksLikeName,
    tileName,
    hasLeaveButton,
    inCall,
    callTitle,
    parseChatTime,
    isOwnSender,
    chatMessages
  };
  root.FSSelectors = api;
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
})(typeof self !== 'undefined' ? self : globalThis);
