import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export type Source = 'mic' | 'loopback';

export interface Seg {
  key: string;
  source: Source;
  id: number;
  startMs: number;
  text: string;
  final: boolean;
}

interface TranscriptEvent {
  source: Source;
  kind: 'partial' | 'final';
  segmentId: number;
  startMs: number;
  text: string;
}

interface Status {
  recording: boolean;
  modelsDir: string | null;
}

export const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

const LANG_KEY = 'fivesight.language';

function loadLanguage(): string {
  try {
    return localStorage.getItem(LANG_KEY) ?? 'auto';
  } catch {
    return 'auto';
  }
}

class Session {
  segs = $state<Seg[]>([]);
  recording = $state(false);
  busy = $state(false);
  error = $state<string | null>(null);
  startedAt = $state<number | null>(null);
  modelsDir = $state<string | null>(null);
  language = $state(loadLanguage());

  /** Segments in time order. */
  sorted = $derived([...this.segs].sort((a, b) => a.startMs - b.startMs));

  private mockTimer: ReturnType<typeof setInterval> | null = null;

  async init() {
    if (!inTauri) return;
    const s = await invoke<Status>('get_status');
    this.recording = s.recording;
    this.modelsDir = s.modelsDir;
    await listen<TranscriptEvent>('transcript', (e) => this.ingest(e.payload));
    await listen<boolean>('recording', (e) => {
      this.recording = e.payload;
      if (!e.payload) this.startedAt = null;
    });
  }

  setLanguage(lang: string) {
    this.language = lang;
    try {
      localStorage.setItem(LANG_KEY, lang);
    } catch {
      /* storage unavailable: keep in memory */
    }
  }

  ingest(ev: TranscriptEvent) {
    const key = `${ev.source}:${ev.segmentId}`;
    const final = ev.kind === 'final';
    const i = this.segs.findIndex((s) => s.key === key);
    if (i === -1) {
      this.segs.push({ key, source: ev.source, id: ev.segmentId, startMs: ev.startMs, text: ev.text, final });
    } else if (!this.segs[i].final) {
      this.segs[i].text = ev.text;
      this.segs[i].final = final;
    }
  }

  async start() {
    if (this.busy || this.recording) return;
    this.busy = true;
    this.error = null;
    try {
      if (inTauri) {
        await invoke('start_recording', { language: this.language });
      } else {
        this.startMock();
      }
      this.recording = true;
      this.startedAt = Date.now();
    } catch (e) {
      this.error = String(e);
    } finally {
      this.busy = false;
    }
  }

  async stop() {
    if (this.busy || !this.recording) return;
    this.busy = true;
    try {
      if (inTauri) await invoke('stop_recording');
      else this.stopMock();
      this.recording = false;
      this.startedAt = null;
    } catch (e) {
      this.error = String(e);
    } finally {
      this.busy = false;
    }
  }

  clear() {
    this.segs = [];
  }

  async openLivePanel() {
    if (inTauri) await invoke('open_live_panel');
  }

  /** Scripted conversation so the UI can be previewed in a plain browser. */
  private startMock() {
    const script: [Source, string][] = [
      ['loopback', 'Good morning everyone. Let us start with the status of ticket PAY-142.'],
      ['mic', 'Thanks. I reproduced it on staging yesterday, the retry waits thirty seconds.'],
      ['loopback', 'Can you update the Confluence page with the root cause?'],
      ['mic', 'Sure, I will also add a regression test and share the pull request.']
    ];
    let line = 0;
    let word = 0;
    let t = 0;
    this.mockTimer = setInterval(() => {
      const [source, text] = script[line % script.length];
      const words = text.split(' ');
      word++;
      const id = Math.floor(line) + 1;
      const done = word >= words.length;
      this.ingest({
        source,
        kind: done ? 'final' : 'partial',
        segmentId: id,
        startMs: t,
        text: words.slice(0, word).join(' ')
      });
      if (done) {
        line++;
        word = 0;
        t += 6000;
      }
    }, 280);
  }

  private stopMock() {
    if (this.mockTimer) clearInterval(this.mockTimer);
    this.mockTimer = null;
  }
}

export const session = new Session();
