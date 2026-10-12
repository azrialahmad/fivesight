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

export interface Meeting {
  id: number;
  title: string;
  startedAt: number;
  endedAt: number | null;
  notes: string;
  enhanced: string;
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
  provider: 'openrouter' | 'local';
  openrouterModel: string;
  fallbackModel: string;
  localUrl: string;
  localModel: string;
  language: string;
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
  /** Records into `meetingId` if given, otherwise creates a meeting. Returns the meeting id. */
  startRecording(language: string, meetingId: number | null): Promise<number>;
  stopRecording(): Promise<void>;
  listMeetings(): Promise<MeetingSummary[]>;
  getMeeting(id: number): Promise<{ meeting: Meeting; transcript: StoredSeg[]; chat: ChatMsg[] }>;
  newMeeting(): Promise<number>;
  setNotes(id: number, notes: string): Promise<void>;
  renameMeeting(id: number, title: string): Promise<void>;
  deleteMeeting(id: number): Promise<void>;
  enhance(id: number): Promise<void>;
  ask(meetingId: number | null, question: string): Promise<void>;
  search(query: string): Promise<Hit[]>;
  getSettings(): Promise<Settings>;
  saveSettings(s: Settings, apiKey: string | null): Promise<void>;
  openLivePanel(): Promise<void>;
  onTranscript(cb: (e: TranscriptEvent) => void): Promise<Off>;
  onRecording(cb: (recording: boolean) => void): Promise<Off>;
  onEnhance(cb: (e: StreamEvent) => void): Promise<Off>;
  onAsk(cb: (e: StreamEvent) => void): Promise<Off>;
}

export const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

const tauriApi: Api = {
  status: () => invoke('get_status'),
  startRecording: (language, meetingId) => invoke('start_recording', { language, meetingId }),
  stopRecording: () => invoke('stop_recording'),
  listMeetings: () => invoke('list_meetings'),
  getMeeting: (id) => invoke('get_meeting', { id }),
  newMeeting: () => invoke('new_meeting'),
  setNotes: (id, notes) => invoke('set_notes', { id, notes }),
  renameMeeting: (id, title) => invoke('rename_meeting', { id, title }),
  deleteMeeting: (id) => invoke('delete_meeting', { id }),
  enhance: (id) => invoke('enhance_notes', { id }),
  ask: (meetingId, question) => invoke('ask', { meetingId, question }),
  search: (query) => invoke('search', { query }),
  getSettings: () => invoke('get_settings'),
  saveSettings: (settings, apiKey) => invoke('save_settings', { settings, apiKey }),
  openLivePanel: () => invoke('open_live_panel'),
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
    provider: 'openrouter',
    openrouterModel: 'google/gemini-2.5-flash',
    fallbackModel: '',
    localUrl: 'http://localhost:11434/v1',
    localModel: 'gemma3',
    language: 'auto',
    hasKey: false
  };
  const day = 86_400_000;
  const meetings: Meeting[] = [
    { id: 3, title: 'Payments sync', startedAt: Date.now() - 2 * 3600_000, endedAt: Date.now() - 3600_000, notes: '- PAY-142 timeout\n- retry = 30s?\n- Budi owns fix, Fri', enhanced: '## PAY-142 checkout timeout\n\nThe retry logic **waits 30 seconds** before failing on staging.\n\n## Action items\n\n- Budi: ship the fix by Friday\n- You: add a regression test', },
    { id: 2, title: 'Design review', startedAt: Date.now() - day, endedAt: Date.now() - day + 2400_000, notes: '', enhanced: '' },
    { id: 1, title: 'Weekly planning', startedAt: Date.now() - 3 * day, endedAt: Date.now() - 3 * day + 3000_000, notes: '', enhanced: '' }
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
      meetings.unshift({ id, title: 'Untitled meeting', startedAt: Date.now(), endedAt: null, notes: '', enhanced: '' });
      return id;
    },
    startRecording: async (_language, meetingId) => {
      let id = meetingId;
      if (id === null) {
        id = nextId++;
        meetings.unshift({ id, title: 'Untitled meeting', startedAt: Date.now(), endedAt: null, notes: '', enhanced: '' });
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
      const md = `## ${m.title}\n\nThe team went through **PAY-142**. The retry logic waits thirty seconds before failing on staging, which is why checkout times out.\n\n## Decisions\n\n- Fix the retry window rather than raising the timeout\n\n## Action items\n\n- Budi: ship the fix by Friday\n- You: add a regression test and share the pull request\n`;
      await stream('enhance-delta', id, md);
      m.enhanced = md;
    },
    ask: async (meetingId, question) => {
      const a = `Based on the transcript, the checkout timeout comes from the retry logic waiting thirty seconds before failing [0:07]. Budi was asked to ship a fix by Friday [0:15].\n\n(Mock answer to: "${question}")`;
      if (meetingId !== null) (chats[meetingId] ??= []).push({ role: 'user', content: question });
      await stream('ask-delta', meetingId, a);
      if (meetingId !== null) chats[meetingId].push({ role: 'assistant', content: a });
    },
    search: async (q) =>
      Object.entries(transcripts).flatMap(([id, segs]) =>
        segs.filter((s) => s.text.toLowerCase().includes(q.toLowerCase())).map((s) => ({ meetingId: Number(id), meetingTitle: get(Number(id)).title, startMs: s.startMs, snippet: s.text }))
      ),
    getSettings: async () => ({ ...settings }),
    saveSettings: async (s, key) => void (settings = { ...s, hasKey: key ? true : s.hasKey }),
    openLivePanel: async () => {},
    onTranscript: (cb) => sub('transcript', cb),
    onRecording: (cb) => sub('recording', cb),
    onEnhance: (cb) => sub('enhance-delta', cb),
    onAsk: (cb) => sub('ask-delta', cb)
  };
}

export const api: Api = inTauri ? tauriApi : createMockApi();
