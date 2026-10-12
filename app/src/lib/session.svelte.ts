import { api, inTauri } from './api';
import type { ChatMsg, Hit, Meeting, MeetingSummary, Seg, Settings, StoredSeg, StreamEvent, TranscriptEvent } from './api';

export { inTauri };
export type View = 'home' | 'meeting' | 'settings';

const DEFAULT_SETTINGS: Settings = {
  baseUrl: 'https://openrouter.ai/api/v1',
  model: 'google/gemini-2.5-flash',
  fallbackModel: '',
  language: 'auto',
  hasKey: false
};

class Session {
  view = $state<View>('home');
  meetings = $state<MeetingSummary[]>([]);
  current = $state<Meeting | null>(null);
  segs = $state<Seg[]>([]);
  chat = $state<ChatMsg[]>([]);
  settings = $state<Settings>({ ...DEFAULT_SETTINGS });

  recording = $state(false);
  recordingMeetingId = $state<number | null>(null);
  startedAt = $state<number | null>(null);
  busy = $state(false);
  error = $state<string | null>(null);
  modelsDir = $state<string | null>(null);

  enhancing = $state(false);
  enhancedLive = $state('');
  asking = $state(false);
  answerLive = $state('');

  drawer = $state(false);
  query = $state('');
  hits = $state<Hit[]>([]);

  sorted = $derived([...this.segs].sort((a, b) => a.startMs - b.startMs));
  isRecordingCurrent = $derived(this.recording && this.recordingMeetingId === this.current?.id);

  private notesTimer: ReturnType<typeof setTimeout> | null = null;
  private ready = false;

  async init() {
    if (this.ready) return;
    this.ready = true;
    await Promise.all([
      api.onTranscript((e) => this.ingest(e)),
      api.onRecording((r) => {
        this.recording = r;
        if (!r) {
          this.recordingMeetingId = null;
          this.startedAt = null;
          this.refreshMeetings();
        }
      }),
      api.onEnhance((e) => this.onEnhance(e)),
      api.onAsk((e) => this.onAsk(e))
    ]);
    const [status, settings] = await Promise.all([api.status(), api.getSettings()]);
    this.recording = status.recording;
    this.recordingMeetingId = status.recordingMeetingId;
    this.modelsDir = status.modelsDir;
    this.settings = settings;
    await this.refreshMeetings();
    if (status.recording && status.recordingMeetingId !== null) await this.openMeeting(status.recordingMeetingId);
  }

  async refreshMeetings() {
    this.meetings = await api.listMeetings();
  }

  go(view: View) {
    this.view = view;
  }

  async openMeeting(id: number) {
    await this.flushNotes();
    const { meeting, transcript, chat } = await api.getMeeting(id);
    this.current = meeting;
    this.segs = transcript.map(toSeg);
    this.chat = chat;
    this.answerLive = '';
    this.enhancedLive = '';
    this.drawer = false;
    this.error = null;
    this.view = 'meeting';
  }

  /** Create an empty meeting you can take notes in before (or without) recording. */
  async newMeeting() {
    const id = await api.newMeeting();
    await this.refreshMeetings();
    await this.openMeeting(id);
  }

  async startRecording() {
    if (this.busy || this.recording) return;
    this.busy = true;
    this.error = null;
    try {
      // Record into the meeting that's open (also when continuing a finished one).
      const id = await api.startRecording(this.view === 'meeting' ? (this.current?.id ?? null) : null);
      this.recording = true;
      this.recordingMeetingId = id;
      this.startedAt = Date.now();
      await this.refreshMeetings();
      if (this.current?.id !== id) await this.openMeeting(id);
    } catch (e) {
      this.error = String(e);
    } finally {
      this.busy = false;
    }
  }

  async stopRecording() {
    if (this.busy || !this.recording) return;
    this.busy = true;
    try {
      await api.stopRecording();
      this.recording = false;
      this.recordingMeetingId = null;
      this.startedAt = null;
      await this.refreshMeetings();
      if (this.current) this.current = (await api.getMeeting(this.current.id)).meeting;
    } catch (e) {
      this.error = String(e);
    } finally {
      this.busy = false;
    }
  }

  private ingest(ev: TranscriptEvent) {
    if (ev.meetingId !== this.current?.id) return;
    const key = `${ev.source}:${ev.segmentId}`;
    const final = ev.kind === 'final';
    const i = this.segs.findIndex((s) => s.key === key);
    if (i === -1) this.segs.push({ key, source: ev.source, id: ev.segmentId, startMs: ev.startMs, text: ev.text, final });
    else if (!this.segs[i].final) {
      this.segs[i].text = ev.text;
      this.segs[i].final = final;
    }
  }

  setNotes(text: string) {
    if (!this.current) return;
    this.current.notes = text;
    if (this.notesTimer) clearTimeout(this.notesTimer);
    this.notesTimer = setTimeout(() => this.flushNotes(), 600);
  }

  async flushNotes() {
    if (this.notesTimer) clearTimeout(this.notesTimer);
    this.notesTimer = null;
    if (this.current) await api.setNotes(this.current.id, this.current.notes).catch(() => {});
  }

  async rename(title: string) {
    if (!this.current) return;
    const t = title.trim() || 'Untitled meeting';
    this.current.title = t;
    await api.renameMeeting(this.current.id, t);
    this.refreshMeetings();
  }

  /** Pick what the speech recognizer expects. Applies from the next time recording starts. */
  async setMeetingLanguage(language: string) {
    if (!this.current) return;
    this.current.language = language;
    try {
      await api.setMeetingLanguage(this.current.id, language);
    } catch (e) {
      this.error = String(e);
    }
  }

  async remove(id: number) {
    await api.deleteMeeting(id);
    if (this.current?.id === id) {
      this.current = null;
      this.view = 'home';
    }
    await this.refreshMeetings();
  }

  async enhance() {
    if (!this.current || this.enhancing) return;
    await this.flushNotes();
    this.enhancing = true;
    this.enhancedLive = '';
    this.error = null;
    try {
      await api.enhance(this.current.id);
    } catch (e) {
      this.enhancing = false;
      this.error = String(e);
    }
  }

  private onEnhance(e: StreamEvent) {
    if (e.meetingId !== this.current?.id) return;
    if (e.error) {
      this.enhancing = false;
      this.error = e.error;
    } else if (e.done) {
      this.enhancing = false;
      if (this.current) this.current.enhanced = this.enhancedLive;
      this.enhancedLive = '';
    } else this.enhancedLive += e.delta;
  }

  async ask(question: string) {
    const q = question.trim();
    if (!q || this.asking) return;
    this.chat.push({ role: 'user', content: q });
    this.asking = true;
    this.answerLive = '';
    this.error = null;
    try {
      await api.ask(this.current?.id ?? null, q);
    } catch (e) {
      this.asking = false;
      this.error = String(e);
    }
  }

  private onAsk(e: StreamEvent) {
    if (e.meetingId !== (this.current?.id ?? null)) return;
    if (e.error) {
      this.asking = false;
      this.error = e.error;
    } else if (e.done) {
      this.asking = false;
      this.chat.push({ role: 'assistant', content: this.answerLive });
      this.answerLive = '';
    } else this.answerLive += e.delta;
  }

  async runSearch(q: string) {
    this.query = q;
    this.hits = q.trim().length < 2 ? [] : await api.search(q.trim());
  }

  async saveSettings(s: Settings, apiKey: string | null) {
    await api.saveSettings(s, apiKey);
    this.settings = await api.getSettings();
  }

  openLivePanel() {
    return api.openLivePanel();
  }
}

function toSeg(s: StoredSeg): Seg {
  return { key: `${s.source}:${s.segId}`, source: s.source, id: s.segId, startMs: s.startMs, text: s.text, final: true };
}

export const session = new Session();
