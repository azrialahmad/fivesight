import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open } from '@tauri-apps/plugin-dialog';

export type Source = 'mic' | 'loopback';

export interface Seg {
  key: string;
  source: Source;
  id: number;
  startMs: number;
  text: string;
  final: boolean;
  /** Participant name from the Meet extension, for what other people said. */
  speaker?: string | null;
}

export interface Meeting {
  id: number;
  title: string;
  startedAt: number;
  endedAt: number | null;
  notes: string;
  enhanced: string;
  /** What the speech recognizer expects: 'auto' (Indonesian + English), 'id' or 'en'. */
  language: string;
}

export interface MeetingSummary {
  id: number;
  title: string;
  startedAt: number;
  endedAt: number | null;
  segmentCount: number;
}

export interface StoredSeg {
  source: Source;
  segId: number;
  startMs: number;
  text: string;
  speaker?: string | null;
}

export interface ChatMsg {
  role: 'user' | 'assistant';
  content: string;
}

export interface Hit {
  meetingId: number;
  meetingTitle: string;
  startMs: number;
  snippet: string;
}

export interface Settings {
  /** Any OpenAI-compatible server (OpenRouter, OpenAI, Groq, Ollama, a company gateway...). */
  baseUrl: string;
  model: string;
  fallbackModel: string;
  language: string;
  /** Let Ask read Jira and Confluence through acli (read-only). */
  atlassian: boolean;
  /** Keep the AI notes up to date on their own while recording. */
  autoNotes: boolean;
  /** Whisper model id, from the setup's speech model list. */
  speechModel: string;
  /** First-run setup finished or skipped. */
  setupDone: boolean;
  /** Whether a key is saved for `baseUrl`. Keys live in Windows Credential Manager, one per server. */
  hasKey: boolean;
}

export interface Status {
  recording: boolean;
  recordingMeetingId: number | null;
  modelsDir: string | null;
}

export interface TranscriptEvent {
  source: Source;
  kind: 'partial' | 'final';
  segmentId: number;
  startMs: number;
  text: string;
  meetingId: number;
  speaker?: string | null;
}

export interface Gpu {
  name: string;
  vramMb: number;
  nvidia: boolean;
}

export interface SystemInfo {
  cpu: string;
  cores: number;
  ramMb: number;
  gpus: Gpu[];
  freeDiskMb: number | null;
}

export interface Recommendation {
  /** False when this PC can't transcribe with this build (no NVIDIA card). */
  supported: boolean;
  speechModel: string | null;
  summary: string;
  warnings: string[];
  localLlmHint: string;
}

export interface SpeechModelInfo {
  id: string;
  label: string;
  note: string;
  vramMb: number;
  bytes: number;
  present: boolean;
}

export interface SetupInfo {
  system: SystemInfo;
  recommendation: Recommendation;
  speechModels: SpeechModelInfo[];
  vadPresent: boolean;
  downloadDir: string | null;
}

/** An AI server already running on this PC. */
export interface LocalServer {
  name: string;
  baseUrl: string;
  models: string[];
}

export interface DownloadEvent {
  file: string;
  index: number;
  count: number;
  received: number;
  total: number;
  /** Everything queued is downloaded. */
  done: boolean;
  error: string | null;
}

export interface BridgeStatus {
  port: number;
  /** Pairing code the browser extension asks for. */
  token: string;
  /** The extension has been heard from recently. */
  connected: boolean;
  inCall: boolean;
  callTitle: string | null;
  error: string | null;
}

export interface CallEvent {
  state: 'joined' | 'left';
  title: string | null;
}

export interface KbDoc {
  id: number;
  path: string;
  title: string;
  kind: 'md' | 'txt' | 'pdf' | string;
  bytes: number;
  modified: number;
  addedAt: number;
  chunks: number;
}

export interface KbAddResult {
  path: string;
  title: string | null;
  error: string | null;
}

export interface AtlassianStatus {
  installed: boolean;
  jiraSite: string | null;
  /** confluence-cli and Node are installed. */
  confluenceCli: boolean;
  /** confluence-cli is set up and answered. */
  confluence: boolean;
}

/** One Jira/Confluence read made while answering. */
export interface AskLookup {
  meetingId: number | null;
  id: string;
  label: string;
  state: 'running' | 'done' | 'failed';
  error: string | null;
}

export interface AskSources {
  meetingId: number | null;
  files: string[];
}

export interface StreamEvent {
  meetingId: number | null;
  delta: string;
  done: boolean;
  error: string | null;
}

type Off = () => void;

export interface Api {
  status(): Promise<Status>;
  /** Records into `meetingId` if given, otherwise creates a meeting; uses that meeting's language. */
  startRecording(meetingId: number | null): Promise<number>;
  setMeetingLanguage(id: number, language: string): Promise<void>;
  stopRecording(): Promise<void>;
  listMeetings(): Promise<MeetingSummary[]>;
  getMeeting(id: number): Promise<{ meeting: Meeting; transcript: StoredSeg[]; chat: ChatMsg[] }>;
  newMeeting(): Promise<number>;
  setNotes(id: number, notes: string): Promise<void>;
  renameMeeting(id: number, title: string): Promise<void>;
  deleteMeeting(id: number): Promise<void>;
  /** `update` builds on the last AI notes (automatic passes) instead of starting over. */
  enhance(id: number, update: boolean): Promise<void>;
  ask(meetingId: number | null, question: string): Promise<void>;
  search(query: string): Promise<Hit[]>;
  getSettings(): Promise<Settings>;
  saveSettings(s: Settings, apiKey: string | null): Promise<void>;
  /** Why settings fell back to defaults at startup, if they did. */
  settingsNotice(): Promise<string | null>;
  keySaved(baseUrl: string): Promise<boolean>;
  forgetKey(baseUrl: string): Promise<void>;
  /** Sends a one-word request with the form's values; resolves with a message or rejects with a fix. */
  testLlm(s: Settings, apiKey: string | null): Promise<string>;
  openLivePanel(): Promise<void>;
  getBridge(): Promise<BridgeStatus>;
  setupInfo(): Promise<SetupInfo>;
  detectLocalLlms(): Promise<LocalServer[]>;
  /** Queue the voice detection model (if missing) and these speech models; returns the files queued. */
  downloadModels(speechModels: string[]): Promise<string[]>;
  cancelDownload(): Promise<void>;
  onModelDownload(cb: (e: DownloadEvent) => void): Promise<Off>;
  kbList(): Promise<KbDoc[]>;
  kbAdd(paths: string[]): Promise<KbAddResult[]>;
  kbRefresh(id: number): Promise<void>;
  kbRemove(id: number): Promise<void>;
  /** Native file picker; null if cancelled. */
  pickFiles(): Promise<string[] | null>;
  onFileDrop(cb: (paths: string[]) => void): Promise<Off>;
  onAskSources(cb: (e: AskSources) => void): Promise<Off>;
  atlassianStatus(): Promise<AtlassianStatus>;
  onAskLookup(cb: (e: AskLookup) => void): Promise<Off>;
  onCall(cb: (e: CallEvent) => void): Promise<Off>;
  onTranscript(cb: (e: TranscriptEvent) => void): Promise<Off>;
  onRecording(cb: (recording: boolean) => void): Promise<Off>;
  onEnhance(cb: (e: StreamEvent) => void): Promise<Off>;
  onAsk(cb: (e: StreamEvent) => void): Promise<Off>;
}

export const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/** Model names never contain "://" or start with "www."; server addresses do. Mirrors the backend check. */
export const looksLikeAddress = (s: string) => {
  const v = s.trim().toLowerCase();
  return v.includes('://') || v.startsWith('www.');
};

const tauriApi: Api = {
  status: () => invoke('get_status'),
  startRecording: (meetingId) => invoke('start_recording', { meetingId }),
  setMeetingLanguage: (id, language) => invoke('set_meeting_language', { id, language }),
  stopRecording: () => invoke('stop_recording'),
  listMeetings: () => invoke('list_meetings'),
  getMeeting: (id) => invoke('get_meeting', { id }),
  newMeeting: () => invoke('new_meeting'),
  setNotes: (id, notes) => invoke('set_notes', { id, notes }),
  renameMeeting: (id, title) => invoke('rename_meeting', { id, title }),
  deleteMeeting: (id) => invoke('delete_meeting', { id }),
  enhance: (id, update) => invoke('enhance_notes', { id, update }),
  ask: (meetingId, question) => invoke('ask', { meetingId, question }),
  search: (query) => invoke('search', { query }),
  getSettings: () => invoke('get_settings'),
  saveSettings: (settings, apiKey) => invoke('save_settings', { settings, apiKey }),
  settingsNotice: () => invoke('settings_notice'),
  keySaved: (baseUrl) => invoke('key_saved', { baseUrl }),
  forgetKey: (baseUrl) => invoke('forget_key', { baseUrl }),
  testLlm: (settings, apiKey) => invoke('test_llm', { settings, apiKey }),
  openLivePanel: () => invoke('open_live_panel'),
  getBridge: () => invoke('get_bridge'),
  setupInfo: () => invoke('setup_info'),
  detectLocalLlms: () => invoke('detect_local_llms'),
  downloadModels: (speechModels) => invoke('download_models', { speechModels }),
  cancelDownload: () => invoke('cancel_download'),
  onModelDownload: (cb) => listen<DownloadEvent>('model-download', (e) => cb(e.payload)),
  kbList: () => invoke('kb_list'),
  kbAdd: (paths) => invoke('kb_add', { paths }),
  kbRefresh: (id) => invoke('kb_refresh', { id }),
  kbRemove: (id) => invoke('kb_remove', { id }),
  pickFiles: async () => {
    const r = await open({ multiple: true, title: 'Add files to fivesight', filters: [{ name: 'Documents', extensions: ['md', 'markdown', 'txt', 'pdf'] }] });
    return r === null ? null : Array.isArray(r) ? r : [r];
  },
  onFileDrop: (cb) =>
    getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === 'drop') cb(e.payload.paths);
    }),
  onAskSources: (cb) => listen<AskSources>('ask-sources', (e) => cb(e.payload)),
  atlassianStatus: () => invoke('atlassian_status'),
  onAskLookup: (cb) => listen<AskLookup>('ask-lookup', (e) => cb(e.payload)),
  onCall: (cb) => listen<CallEvent>('call', (e) => cb(e.payload)),
  onTranscript: (cb) => listen<TranscriptEvent>('transcript', (e) => cb(e.payload)),
  onRecording: (cb) => listen<boolean>('recording', (e) => cb(e.payload)),
  onEnhance: (cb) => listen<StreamEvent>('enhance-delta', (e) => cb(e.payload)),
  onAsk: (cb) => listen<StreamEvent>('ask-delta', (e) => cb(e.payload))
};

/** In-memory stand-in so the UI can be built and previewed in a plain browser. */
function createMockApi(): Api {
  type Handler<T> = (e: T) => void;
  const bus: Record<string, Set<Handler<never>>> = {};
  const sub = <T>(name: string, cb: Handler<T>): Promise<Off> => {
    (bus[name] ??= new Set()).add(cb as Handler<never>);
    return Promise.resolve(() => bus[name].delete(cb as Handler<never>));
  };
  const emit = <T>(name: string, e: T) => bus[name]?.forEach((h) => (h as Handler<T>)(e));

  let nextId = 4;
  let recording: number | null = null;
  let timer: ReturnType<typeof setInterval> | null = null;
  let settings: Settings = {
    baseUrl: 'https://openrouter.ai/api/v1',
    model: 'google/gemini-2.5-flash',
    fallbackModel: '',
    language: 'auto',
    atlassian: true,
    autoNotes: true,
    speechModel: 'large-v3-turbo',
    // The preview opens on the home screen; Settings → Run setup shows the wizard.
    setupDone: true,
    hasKey: false
  };
  const speechModels: SpeechModelInfo[] = [
    { id: 'large-v3-turbo', label: 'Large v3 Turbo', note: 'Best balance of accuracy and speed for live meetings.', vramMb: 2300, bytes: 1_624_555_275, present: false },
    { id: 'large-v3-turbo-q8_0', label: 'Large v3 Turbo (8-bit)', note: 'Almost the same accuracy in about half the memory.', vramMb: 1500, bytes: 874_188_075, present: false },
    { id: 'large-v3-turbo-q5_0', label: 'Large v3 Turbo (5-bit)', note: 'For cards with little memory. Slightly less accurate.', vramMb: 1100, bytes: 574_041_195, present: false },
    { id: 'large-v3', label: 'Large v3', note: 'Most accurate, but several times slower. Needs a strong card to keep up live.', vramMb: 4200, bytes: 3_095_033_483, present: false },
    { id: 'small-q5_1', label: 'Small (5-bit)', note: 'Fastest and smallest. Noticeably weaker, especially for Indonesian.', vramMb: 600, bytes: 190_085_487, present: false }
  ];
  let vadPresent = false;
  let cancelDl = false;
  const keys = new Set<string>();
  let kbNext = 3;
  let kb: KbDoc[] = [
    { id: 2, path: 'C:\\Users\\you\\Documents\\payments-runbook.md', title: 'Payments runbook', kind: 'md', bytes: 18_400, modified: Date.now() - 86_400_000, addedAt: Date.now() - 3_600_000, chunks: 14 },
    { id: 1, path: 'C:\\Users\\you\\Documents\\Checkout spec v3.pdf', title: 'Checkout spec v3', kind: 'pdf', bytes: 412_000, modified: Date.now() - 5 * 86_400_000, addedAt: Date.now() - 2 * 86_400_000, chunks: 61 }
  ];
  const day = 86_400_000;
  const meetings: Meeting[] = [
    { id: 3, title: 'Payments sync', startedAt: Date.now() - 2 * 3600_000, endedAt: Date.now() - 3600_000, notes: '- PAY-142 timeout\n- retry = 30s?\n- Budi owns fix, Fri', enhanced: '- PAY-142 timeout\n  - Checkout times out on staging; reproduced yesterday [0:07]\n- retry = 30s?\n  - Confirmed: the retry logic waits **30 seconds** before failing [0:07]\n- Budi owns fix, Fri\n  - The fix goes to the backend team, due Friday [0:15]\n\n## Also discussed\n\n- Update the Confluence page with the root cause [0:15]\n\n## Action items\n\n- Budi: ship the fix by Friday\n- You: update the Confluence page', language: 'auto' },
    { id: 2, title: 'Design review', startedAt: Date.now() - day, endedAt: Date.now() - day + 2400_000, notes: '', enhanced: '', language: 'auto' },
    { id: 1, title: 'Weekly planning', startedAt: Date.now() - 3 * day, endedAt: Date.now() - 3 * day + 3000_000, notes: '', enhanced: '', language: 'auto' }
  ];
  const transcripts: Record<number, StoredSeg[]> = {
    3: [
      { source: 'loopback', segId: 1, startMs: 0, text: 'Good morning everyone. Let us start with the status of ticket PAY-142.' },
      { source: 'mic', segId: 1, startMs: 7000, text: 'I reproduced it on staging. The retry logic waits thirty seconds before failing.' },
      { source: 'loopback', segId: 2, startMs: 15000, text: 'Can you update the Confluence page and assign the fix to the backend team by Friday?' }
    ]
  };
  const chats: Record<number, ChatMsg[]> = {};
  const get = (id: number) => meetings.find((m) => m.id === id)!;

  const stream = async (name: string, meetingId: number | null, text: string) => {
    for (const w of text.split(/(?<= )/)) {
      await new Promise((r) => setTimeout(r, 35));
      emit<StreamEvent>(name, { meetingId, delta: w, done: false, error: null });
    }
    emit<StreamEvent>(name, { meetingId, delta: '', done: true, error: null });
  };

  return {
    status: async () => ({ recording: recording !== null, recordingMeetingId: recording, modelsDir: null }),
    newMeeting: async () => {
      const id = nextId++;
      meetings.unshift({ id, title: 'Untitled meeting', startedAt: Date.now(), endedAt: null, notes: '', enhanced: '', language: 'auto' });
      return id;
    },
    startRecording: async (meetingId) => {
      let id = meetingId;
      if (id === null) {
        id = nextId++;
        meetings.unshift({ id, title: 'Untitled meeting', startedAt: Date.now(), endedAt: null, notes: '', enhanced: '', language: 'auto' });
      }
      transcripts[id] ??= [];
      const rid = id;
      recording = id;
      const script: [Source, string][] = [
        ['loopback', 'Good morning everyone. Let us start with the status of ticket PAY-142.'],
        ['mic', 'Thanks. I reproduced it on staging yesterday, the retry waits thirty seconds.'],
        ['loopback', 'Can you update the Confluence page with the root cause?'],
        ['mic', 'Sure, I will also add a regression test and share the pull request.']
      ];
      let line = 0, word = 0, t = 0;
      timer = setInterval(() => {
        const [source, text] = script[line % script.length];
        const words = text.split(' ');
        word++;
        const done = word >= words.length;
        emit<TranscriptEvent>('transcript', { source, kind: done ? 'final' : 'partial', segmentId: line + 1, startMs: t, text: words.slice(0, word).join(' '), meetingId: id });
        if (done) {
          transcripts[id].push({ source, segId: line + 1, startMs: t, text });
          line++; word = 0; t += 6000;
        }
      }, 300);
      emit('recording', true);
      return id;
    },
    stopRecording: async () => {
      if (timer) clearInterval(timer);
      timer = null;
      if (recording !== null) get(recording).endedAt = Date.now();
      recording = null;
      emit('recording', false);
    },
    listMeetings: async () => meetings.map((m) => ({ id: m.id, title: m.title, startedAt: m.startedAt, endedAt: m.endedAt, segmentCount: transcripts[m.id]?.length ?? 0 })),
    getMeeting: async (id) => ({ meeting: { ...get(id) }, transcript: [...(transcripts[id] ?? [])], chat: [...(chats[id] ?? [])] }),
    setNotes: async (id, notes) => void (get(id).notes = notes),
    renameMeeting: async (id, title) => void (get(id).title = title),
    deleteMeeting: async (id) => void meetings.splice(meetings.findIndex((m) => m.id === id), 1),
    enhance: async (id) => {
      const m = get(id);
      // Mirrors the real prompt: the user's lines are the skeleton, details go underneath.
      const mine = m.notes.split('\n').map((l) => l.replace(/^\s*[-*]\s*/, '').trim()).filter(Boolean);
      const details = ['Reproduced on staging; the retry waits thirty seconds [0:07]', 'Owner and date agreed in the call [0:15]', 'Came up again near the end'];
      const body = mine.length
        ? mine.map((l, i) => `- ${l}\n  - ${details[i % details.length]}`).join('\n')
        : '## PAY-142 checkout timeout\n\n- The retry logic waits thirty seconds before failing [0:07]';
      const md = `${body}\n\n## Action items\n\n- Budi: ship the fix by Friday\n- You: update the Confluence page\n`;
      await stream('enhance-delta', id, md);
      m.enhanced = md;
    },
    ask: async (meetingId, question) => {
      const a = `Based on the transcript, the checkout timeout comes from the retry logic waiting thirty seconds before failing [0:07]. Budi was asked to ship a fix by Friday [0:15].`;
      if (meetingId !== null) (chats[meetingId] ??= []).push({ role: 'user', content: question });
      emit<AskSources>('ask-sources', { meetingId, files: kb.length ? [kb[0].title] : [] });
      if (settings.atlassian && /PAY-\d+|ticket|jira/i.test(question)) {
        emit<AskLookup>('ask-lookup', { meetingId, id: 'm1', label: 'Read PAY-142', state: 'running', error: null });
        await new Promise((r) => setTimeout(r, 600));
        emit<AskLookup>('ask-lookup', { meetingId, id: 'm1', label: 'Read PAY-142', state: 'done', error: null });
      }
      await stream('ask-delta', meetingId, a);
      if (meetingId !== null) chats[meetingId].push({ role: 'assistant', content: a });
    },
    search: async (q) =>
      Object.entries(transcripts).flatMap(([id, segs]) =>
        segs.filter((s) => s.text.toLowerCase().includes(q.toLowerCase())).map((s) => ({ meetingId: Number(id), meetingTitle: get(Number(id)).title, startMs: s.startMs, snippet: s.text }))
      ),
    getSettings: async () => ({ ...settings }),
    saveSettings: async (s, key) => {
      for (const [field, value] of [['Model', s.model], ['Fallback model', s.fallbackModel]]) {
        if (looksLikeAddress(value)) throw `${field} should be a model name, not a server address. Put the address in Server address.`;
      }
      if (key) keys.add(s.baseUrl.trim());
      settings = { ...s, hasKey: keys.has(s.baseUrl.trim()) };
    },
    settingsNotice: async () => null,
    keySaved: async (baseUrl) => keys.has(baseUrl.trim()),
    forgetKey: async (baseUrl) => void keys.delete(baseUrl.trim()),
    testLlm: async (s, key) => {
      if (!key && !keys.has(s.baseUrl.trim()) && !/localhost|127\.0\.0\.1/.test(s.baseUrl)) throw 'The server needs an API key. Paste yours and try again.';
      return 'Connected. ' + s.model + ' replied: ok';
    },
    setMeetingLanguage: async (id, language) => void (get(id).language = language),
    openLivePanel: async () => {},
    kbList: async () => kb.map((d) => ({ ...d })),
    kbAdd: async (paths) =>
      paths.map((p) => {
        const name = p.split(/[\\/]/).pop() || p;
        if (!/\.(md|markdown|txt|pdf)$/i.test(name)) return { path: p, title: null, error: name + " isn't a supported file (use .md, .txt or .pdf)" };
        const title = name.replace(/\.[^.]+$/, '');
        kb = kb.filter((d) => d.path !== p);
        kb.unshift({ id: kbNext++, path: p, title, kind: name.split('.').pop()!.toLowerCase(), bytes: 2048, modified: Date.now(), addedAt: Date.now(), chunks: 3 });
        return { path: p, title, error: null };
      }),
    kbRefresh: async () => {},
    kbRemove: async (id) => void (kb = kb.filter((d) => d.id !== id)),
    pickFiles: async () => ['C:\\Users\\you\\Documents\\Onboarding notes.md'],
    onFileDrop: async () => () => {},
    onAskSources: (cb) => sub('ask-sources', cb),
    atlassianStatus: async () => {
      await new Promise((r) => setTimeout(r, 400));
      return { installed: true, jiraSite: 'yourcompany.atlassian.net', confluenceCli: true, confluence: false };
    },
    onAskLookup: (cb) => sub('ask-lookup', cb),
    getBridge: async () => ({ port: 47285, token: '3f9a1c7e5b2d4a60918c7be2d41f0a55', connected: false, inCall: false, callTitle: null, error: null }),
    setupInfo: async () => {
      await new Promise((r) => setTimeout(r, 500));
      return {
        system: { cpu: 'AMD Ryzen 7 5700X3D 8-Core Processor', cores: 16, ramMb: 32_671, gpus: [{ name: 'NVIDIA GeForce RTX 5070', vramMb: 11_944, nvidia: true }], freeDiskMb: 1_080_092 },
        recommendation: {
          supported: true,
          speechModel: 'large-v3-turbo',
          summary: 'NVIDIA GeForce RTX 5070 with 12 GB of graphics memory: Large v3 Turbo fits comfortably.',
          warnings: [],
          localLlmHint: 'A local 7-8B model (e.g. qwen3:8b or llama3.1:8b) fits next to speech recognition.'
        },
        speechModels: speechModels.map((m) => ({ ...m })),
        vadPresent,
        downloadDir: 'C:\\Users\\you\\AppData\\Local\\fivesight\\models'
      };
    },
    detectLocalLlms: async () => {
      await new Promise((r) => setTimeout(r, 300));
      return [{ name: 'Ollama', baseUrl: 'http://localhost:11434/v1', models: ['qwen3:8b', 'gemma3:4b'] }];
    },
    downloadModels: async (ids) => {
      const queue = [...(vadPresent ? [] : [{ file: 'silero_vad.onnx', bytes: 2_327_524, done: () => (vadPresent = true) }])];
      for (const id of ids) {
        const m = speechModels.find((x) => x.id === id);
        if (m && !m.present) queue.push({ file: `ggml-${m.id}.bin`, bytes: m.bytes, done: () => (m.present = true) });
      }
      cancelDl = false;
      (async () => {
        for (const [index, q] of queue.entries()) {
          // Pretend the whole file takes about three seconds.
          for (let got = 0; got < q.bytes; got += q.bytes / 20) {
            if (cancelDl) return emit<DownloadEvent>('model-download', { file: q.file, index, count: queue.length, received: got, total: q.bytes, done: false, error: 'Download cancelled.' });
            emit<DownloadEvent>('model-download', { file: q.file, index, count: queue.length, received: got, total: q.bytes, done: false, error: null });
            await new Promise((r) => setTimeout(r, 150));
          }
          q.done();
        }
        emit<DownloadEvent>('model-download', { file: '', index: queue.length, count: queue.length, received: 0, total: 0, done: true, error: null });
      })();
      return queue.map((q) => q.file);
    },
    cancelDownload: async () => void (cancelDl = true),
    onModelDownload: (cb) => sub('model-download', cb),
    onCall: (cb) => sub('call', cb),
    onTranscript: (cb) => sub('transcript', cb),
    onRecording: (cb) => sub('recording', cb),
    onEnhance: (cb) => sub('enhance-delta', cb),
    onAsk: (cb) => sub('ask-delta', cb)
  };
}

export const api: Api = inTauri ? tauriApi : createMockApi();
