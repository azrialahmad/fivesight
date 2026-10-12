// Everything that knows what Google Meet's page looks like lives in this file.
// Meet changes its markup without notice; when speaker names stop appearing, fix it here.
// Turn on "Debug logging" in the extension popup to see what these functions find.
(function (root) {
  const CODE = /^\/([a-z]{3}-[a-z]{4}-[a-z]{3})(?:$|[/?])/;

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

  /** The display name shown on a tile: the first plain text label that isn't a button or icon. */
  function tileName(tile) {
    const walker = tile.ownerDocument.createTreeWalker(tile, NodeFilter.SHOW_ELEMENT);
    for (let el = walker.nextNode(); el; el = walker.nextNode()) {
      if (el.childElementCount > 0) continue;
      if (el.closest('button, [role="button"], [aria-hidden="true"], video')) continue;
      const text = (el.textContent || '').trim();
      if (!text || text.length > 80) continue;
      if (isIconText(el)) continue;
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

  const api = { meetCode, tiles, tileId, ownName, tileName, hasLeaveButton, inCall, callTitle };
  root.FSSelectors = api;
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
})(typeof self !== 'undefined' ? self : globalThis);
