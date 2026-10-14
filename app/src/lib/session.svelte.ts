import { api, inTauri } from './api';
import type { AskLookup, AskSources, CallEvent, ChatMsg, Hit, KbAddResult, KbDoc, Meeting, MeetingSummary, Seg, Settings, StoredSeg, StreamEvent, TranscriptEvent } from './api';

export { inTauri };
export type View = 'home' | 'meeting' | 'settings' | 'knowledge';

const DEFAULT_SETTINGS: Settings = {
  baseUrl: 'https://openrouter.ai/api/v1',
  model: 'google/gemini-2.5-flash',
  fallbackModel: '',
  language: 'auto',
  atlassian: true,
  autoNotes: true,
  hasKey: false
};

/** Automatic AI notes while recording: at most one pass this often... */
const AUTO_MIN_GAP_MS = 45_000;
/** ...after the user stops typing in their notes for this long... */
const AUTO_TYPING_PAUSE_MS = 4_000;
/** ...or once this many new transcript lines are in... */
const AUTO_NEW_LINES = 6;
/** ...or after this long with any new line at all. */
const AUTO_MAX_WAIT_MS = 90_000;

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

  /** Meeting whose AI notes are being written, and whether that is an automatic pass. */
  enhancingId = $state<number | null>(null);
  enhancingAuto = $state(false);
  enhancedLive = $state('');
  /** Last automatic pass that failed; shown quietly, it retries on the next pass. */
  autoError = $state<string | null>(null);
  asking = $state(false);
  answerLive = $state('');

  /** A Meet call the extension saw start (offer to record) or end (offer to stop). */
  callPrompt = $state<CallEvent | null>(null);

  kb = $state<KbDoc[]>([]);
  kbBusy = $state(false);
  /** Files that couldn't be added, from the last add. */
  kbErrors = $state<KbAddResult[]>([]);
  /** Files the latest answer drew on. */
  answerSources = $state<string[]>([]);
  /** Jira/Confluence reads made for the latest answer. */
  lookups = $state<AskLookup[]>([]);

  drawer = $state(false);
  query = $state('');
  hits = $state<Hit[]>([]);

  sorted = $derived([...this.segs].sort((a, b) => a.startMs - b.startMs));
  isRecordingCurrent = $derived(this.recording && this.recordingMeetingId === this.current?.id);
  enhancing = $derived(this.enhancingId !== null && this.enhancingId === this.current?.id);

  private notesTimer: ReturnType<typeof setTimeout> | null = null;
  private ready = false;

  // Automatic AI notes for the meeting being recorded (which need not be the one on screen).
  private autoTimer: ReturnType<typeof setInterval> | null = null;
  private lastAutoAt = 0;
  private newLines = 0;
  private notesEditedAt = 0;
  /** Recording stopped with changes the last pass hasn't seen: run once more, not live. */
  private finalPassFor: number | null = null;

  async init() {
    if (this.ready) return;
    this.ready = true;
    await Promise.all([
      api.onTranscript((e) => this.ingest(e)),
      api.onRecording((r) => {
        if (!r && this.recordingMeetingId !== null) this.recordingEnded(this.recordingMeetingId);
        this.recording = r;
        if (!r) {
          this.recordingMeetingId = null;
          this.startedAt = null;
          this.refreshMeetings();
        }
      }),
      api.onCall((e) => this.onCall(e)),
      api.onAskSources((e) => this.onAskSources(e)),
      api.onAskLookup((e) => this.onAskLookup(e)),
      api.onFileDrop((paths) => {
        if (this.view === 'knowledge') this.addFiles(paths);
      }),
      api.onEnhance((e) => this.onEnhance(e)),
      api.onAsk((e) => this.onAsk(e))
    ]);
    const [status, settings] = await Promise.all([api.status(), api.getSettings()]);
    this.recording = status.recording;
    this.recordingMeetingId = status.recordingMeetingId;
    this.modelsDir = status.modelsDir;
    this.settings = settings;
    await Promise.all([this.refreshMeetings(), this.loadKb()]);
    if (status.recording && status.recordingMeetingId !== null) {
      this.recordingStarted();
      await this.openMeeting(status.recordingMeetingId);
    }
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

  private onCall(e: CallEvent) {
    if (e.state === 'joined') this.callPrompt = this.recording ? null : e;
    else this.callPrompt = this.recording ? e : null;
  }

  dismissCall() {
    this.callPrompt = null;
  }

  /** Start from the "call detected" prompt: always a new meeting, named after the call if it has a name. */
  async startFromCall() {
    const title = this.callPrompt?.title ?? null;
    this.callPrompt = null;
    await this.startRecording(true);
    if (title && this.recording && this.current?.title === 'Untitled meeting') await this.rename(title);
  }

  async stopFromCall() {
    this.callPrompt = null;
    await this.stopRecording();
  }

  async startRecording(fresh = false) {
    if (this.busy || this.recording) return;
    this.busy = true;
    this.error = null;
    try {
      // Record into the meeting that's open (also when continuing a finished one).
      const id = await api.startRecording(!fresh && this.view === 'meeting' ? (this.current?.id ?? null) : null);
      this.recording = true;
      this.recordingMeetingId = id;
      this.startedAt = Date.now();
      this.recordingStarted();
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
      const id = this.recordingMeetingId;
      await api.stopRecording();
      if (id !== null) this.recordingEnded(id);
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
    if (ev.kind === 'final' && ev.text.trim() && ev.meetingId === this.recordingMeetingId) this.newLines++;
    if (ev.meetingId !== this.current?.id) return;
    const key = `${ev.source}:${ev.segmentId}`;
    const final = ev.kind === 'final';
    const i = this.segs.findIndex((s) => s.key === key);
    // An empty final means the line was dropped (noise, or echo of the call).
    if (final && !ev.text.trim()) {
      if (i !== -1 && !this.segs[i].final) this.segs.splice(i, 1);
      return;
    }
    if (i === -1) this.segs.push({ key, source: ev.source, id: ev.segmentId, startMs: ev.startMs, text: ev.text, final, speaker: ev.speaker ?? null });
    else if (!this.segs[i].final) {
      this.segs[i].text = ev.text;
      this.segs[i].final = final;
      this.segs[i].speaker = ev.speaker ?? this.segs[i].speaker;
    }
  }

  setNotes(text: string) {
    if (!this.current) return;
    this.current.notes = text;
    if (this.current.id === this.recordingMeetingId) this.notesEditedAt = Date.now();
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

  /** Write the AI notes from scratch now (the Enhance button). */
  async enhance() {
    if (!this.current || this.enhancingId !== null) return;
    this.error = null;
    await this.runEnhance(this.current.id, false);
  }

  private async runEnhance(id: number, auto: boolean) {
    if (this.current?.id === id) await this.flushNotes();
    this.enhancingId = id;
    this.enhancingAuto = auto;
    this.enhancedLive = '';
    try {
      // Automatic passes build on the last AI notes so they don't churn.
      await api.enhance(id, auto);
    } catch (e) {
      this.enhanceFailed(String(e));
    }
  }

  private enhanceFailed(message: string) {
    if (this.enhancingAuto) this.autoError = message;
    else this.error = message;
    this.enhancingId = null;
    this.enhancedLive = '';
  }

  private onEnhance(e: StreamEvent) {
    if (e.meetingId === null || e.meetingId !== this.enhancingId) return;
    if (e.error) {
      this.enhanceFailed(e.error);
    } else if (e.done) {
      if (this.current?.id === e.meetingId) this.current.enhanced = this.enhancedLive;
      if (this.enhancingAuto) this.autoError = null;
      this.enhancingId = null;
      this.enhancedLive = '';
      this.autoTick();
    } else this.enhancedLive += e.delta;
  }

  private recordingStarted() {
    this.lastAutoAt = Date.now();
    this.newLines = 0;
    this.notesEditedAt = 0;
    this.finalPassFor = null;
    this.autoError = null;
    if (!this.autoTimer) this.autoTimer = setInterval(() => this.autoTick(), 2_000);
  }

  private recordingEnded(id: number) {
    if (this.autoTimer) clearInterval(this.autoTimer);
    this.autoTimer = null;
    // Called from both stopRecording and the backend's "recording" event; the second call
    // finds nothing changed because the final pass stamps lastAutoAt.
    const changed = this.newLines > 0 || this.notesEditedAt > this.lastAutoAt;
    if (this.settings.autoNotes && changed) {
      this.finalPassFor = id;
      this.autoTick();
    }
  }

  /** Start an automatic pass when one is due and nothing else is being written. */
  private autoTick() {
    if (this.enhancingId !== null || !this.settings.autoNotes) return;
    if (this.finalPassFor !== null) {
      const id = this.finalPassFor;
      this.finalPassFor = null;
      this.lastAutoAt = Date.now();
      this.newLines = 0;
      this.runEnhance(id, true);
      return;
    }
    const id = this.recordingMeetingId;
    if (!this.recording || id === null) return;
    const now = Date.now();
    if (now - this.lastAutoAt < AUTO_MIN_GAP_MS) return;
    const typedThenPaused = this.notesEditedAt > this.lastAutoAt && now - this.notesEditedAt >= AUTO_TYPING_PAUSE_MS;
    const enoughNew = this.newLines >= AUTO_NEW_LINES || (this.newLines > 0 && now - this.lastAutoAt >= AUTO_MAX_WAIT_MS);
    if (!typedThenPaused && !enoughNew) return;
    this.lastAutoAt = now;
    this.newLines = 0;
    this.runEnhance(id, true);
  }

  async ask(question: string) {
    const q = question.trim();
    if (!q || this.asking) return;
    this.chat.push({ role: 'user', content: q });
    this.answerSources = [];
    this.lookups = [];
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

  private onAskSources(e: AskSources) {
    if (e.meetingId === (this.current?.id ?? null)) this.answerSources = e.files;
  }

  private onAskLookup(e: AskLookup) {
    if (e.meetingId !== (this.current?.id ?? null)) return;
    const i = this.lookups.findIndex((l) => l.id === e.id);
    if (i === -1) this.lookups.push(e);
    else this.lookups[i] = e;
  }

  async loadKb() {
    this.kb = await api.kbList();
  }

  async addFiles(paths: string[]) {
    if (!paths.length || this.kbBusy) return;
    this.kbBusy = true;
    this.kbErrors = [];
    try {
      const results = await api.kbAdd(paths);
      this.kbErrors = results.filter((r) => r.error);
      await this.loadKb();
    } catch (e) {
      this.kbErrors = [{ path: '', title: null, error: String(e) }];
    } finally {
      this.kbBusy = false;
    }
  }

  async pickFiles() {
    const paths = await api.pickFiles();
    if (paths) await this.addFiles(paths);
  }

  async refreshDoc(id: number) {
    this.kbErrors = [];
    try {
      await api.kbRefresh(id);
    } catch (e) {
      const d = this.kb.find((x) => x.id === id);
      this.kbErrors = [{ path: d?.path ?? '', title: d?.title ?? null, error: String(e) }];
    }
    await this.loadKb();
  }

  async removeDoc(id: number) {
    await api.kbRemove(id);
    await this.loadKb();
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
  return { key: `${s.source}:${s.segId}`, source: s.source, id: s.segId, startMs: s.startMs, text: s.text, final: true, speaker: s.speaker ?? null };
}

export const session = new Session();
