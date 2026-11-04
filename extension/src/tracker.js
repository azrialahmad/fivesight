// Turns noisy "something changed in this tile" signals into clean speaking / call events.
// Pure logic with no DOM access, so it is unit-tested in Node (see test/tracker.test.js).
(function (root) {
  /**
   * A tile counts as speaking while its subtree keeps changing (the audio indicator animates),
   * and stops after a short quiet spell. Two quick hits start it and about a second of quiet
   * ends it, so a hover highlight or a brief blip never names anyone.
   */
  function createSpeakingTracker(opts) {
    const o = Object.assign({ windowMs: 500, minEvents: 3, onTicks: 2, offTicks: 5, maxEvents: 64 }, opts);
    const tiles = new Map(); // id -> { events: number[], hit, miss, on, name }
    const said = new Map(); // name -> bool, what we last told the app

    function note(id, at) {
      let t = tiles.get(id);
      if (!t) tiles.set(id, (t = { events: [], hit: 0, miss: 0, on: false, name: null }));
      t.events.push(at);
      if (t.events.length > o.maxEvents) t.events.splice(0, t.events.length - o.maxEvents);
    }

    /** `present` maps tile id -> display name (null if unknown). Call every ~200 ms. */
    function tick(now, present) {
      for (const [id, name] of present) {
        if (!tiles.has(id)) tiles.set(id, { events: [], hit: 0, miss: 0, on: false, name: null });
        tiles.get(id).name = name;
      }
      for (const [id, t] of tiles) {
        if (!present.has(id)) {
          // Tile left the grid: it can't be speaking any more.
          t.on = false;
          t.events.length = 0;
          continue;
        }
        const recent = t.events.filter((e) => now - e <= o.windowMs).length;
        t.events = t.events.filter((e) => now - e <= o.windowMs);
        if (recent >= o.minEvents) {
          t.hit++;
          t.miss = 0;
          if (!t.on && t.hit >= o.onTicks) t.on = true;
        } else {
          t.miss++;
          t.hit = 0;
          if (t.on && t.miss >= o.offTicks) t.on = false;
        }
      }
      const names = new Set();
      for (const t of tiles.values()) if (t.name) names.add(t.name);
      for (const n of said.keys()) names.add(n);
      const out = [];
      for (const n of names) {
        const speaking = anyOn(n);
        if ((said.get(n) || false) !== speaking) {
          said.set(n, speaking);
          out.push({ name: n, speaking, at: now });
        }
      }
      for (const [id, t] of tiles) if (!present.has(id) && !t.on) tiles.delete(id);
      for (const [n, s] of said) if (!s && !names.has(n)) said.delete(n);
      return out;
    }

    function anyOn(name) {
      for (const t of tiles.values()) if (t.on && t.name === name) return true;
      return false;
    }

    /** Everyone stops (left the call). */
    function reset(now) {
      const out = [];
      for (const [n, s] of said) if (s) out.push({ name: n, speaking: false, at: now });
      tiles.clear();
      said.clear();
      return out;
    }

    return { note, tick, reset };
  }

  /** Debounces "am I in a call" so a re-render doesn't look like leaving and rejoining. */
  function createCallTracker(opts) {
    const o = Object.assign({ joinTicks: 2, leaveTicks: 3 }, opts);
    let inCall = false;
    let up = 0;
    let down = 0;
    return {
      /** Returns 'joined', 'left', or null when nothing changed. */
      tick(now) {
        if (now) {
          up++;
          down = 0;
          if (!inCall && up >= o.joinTicks) return ((inCall = true), 'joined');
        } else {
          down++;
          up = 0;
          if (inCall && down >= o.leaveTicks) return ((inCall = false), 'left');
        }
        return null;
      },
      /** The page is going away. */
      leaveNow() {
        if (!inCall) return null;
        inCall = false;
        up = down = 0;
        return 'left';
      },
      get inCall() {
        return inCall;
      }
    };
  }

  /**
   * Picks out chat messages not reported yet. Meet redraws the whole history each time the chat
   * panel opens, so messages are remembered by id. A message seen as it arrives is stamped now;
   * one found later (the panel was closed) takes the minute Meet printed over it.
   */
  function createChatTracker(opts) {
    const o = Object.assign({ liveMs: 2 * 60 * 1000, maxText: 2000, maxSeen: 5000 }, opts);
    const seen = new Set();
    return {
      /** `messages` from FSSelectors.chatMessages, with `at` (parsed header time or null). */
      fresh(now, messages) {
        const out = [];
        for (const m of messages) {
          if (!m.id || seen.has(m.id)) continue;
          seen.add(m.id);
          const at = m.at != null && now - m.at > o.liveMs ? m.at : now;
          out.push({ id: m.id, sender: m.sender, text: m.text.slice(0, o.maxText), at, me: !!m.me });
        }
        if (seen.size > o.maxSeen) seen.clear(); // a marathon call; the app also drops repeats
        return out;
      },
      reset() {
        seen.clear();
      }
    };
  }

  const api = { createSpeakingTracker, createCallTracker, createChatTracker };
  root.FSTracker = api;
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
})(typeof self !== 'undefined' ? self : globalThis);
