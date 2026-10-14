//! fivesight desktop shell: owns the pipeline, the database and the LLM calls, and
//! forwards everything to the UI as commands and events.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use fs_audio::Source;
use fs_bridge::{Bridge, CallState, Incoming, SpeakerLog};
use fs_llm::{chat_stream, prompts, LlmConfig, Message};
use fs_pipeline::{Kind, Pipeline, PipelineConfig, TranscriptEvent};
use fs_store::{ChatMessage, KbDoc, Meeting, MeetingSummary, SearchHit, Segment, Store};
use fs_stt::lang::LangMode;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

mod logging;
mod lookups;
mod models;
mod system;

const KEYRING_SERVICE: &str = "fivesight";
/// Credential Manager entries from before keys were stored per server (see `key_slot`).
const LEGACY_KEY_OPENROUTER: &str = "openrouter";
const LEGACY_KEY_CUSTOM: &str = "custom";
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Lock, recovering the data if another thread panicked while holding it.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Every field has a serde default, so a field missing from `settings.json` (older or
/// hand-edited file) doesn't fail the whole file. `base_url` defaults to empty on purpose:
/// that is how [`Settings::migrated`] spots a file from before the provider was generalised.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    /// Any OpenAI-compatible server: OpenRouter, OpenAI, Groq, Ollama, a company gateway...
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    fallback_model: String,
    /// Language given to new meetings: "auto", "id" or "en". Each meeting can override it.
    #[serde(default = "auto_language")]
    language: String,
    /// Let Ask read Jira and Confluence through acli (read-only).
    #[serde(default = "enabled")]
    atlassian: bool,
    /// Keep the AI notes up to date on their own while recording (one AI request every minute or so).
    #[serde(default = "enabled")]
    auto_notes: bool,
    /// Which Whisper model to transcribe with (an id from `models::SPEECH_MODELS`).
    #[serde(default = "default_speech_model")]
    speech_model: String,
    /// The first-run setup was finished (or skipped); it opens on start until then.
    #[serde(default)]
    setup_done: bool,
    /// Filled in on read; never persisted (keys live in Windows Credential Manager, one per server).
    #[serde(default)]
    has_key: bool,
    // Fields from before the provider was generalised. Read once to migrate, never written back.
    #[serde(default, skip_serializing)]
    provider: Option<String>,
    #[serde(default, skip_serializing)]
    openrouter_model: Option<String>,
    #[serde(default, skip_serializing, alias = "localUrl")]
    custom_url: Option<String>,
    #[serde(default, skip_serializing, alias = "localModel")]
    custom_model: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            base_url: OPENROUTER_URL.into(),
            model: "google/gemini-2.5-flash".into(),
            fallback_model: String::new(),
            language: "auto".into(),
            atlassian: true,
            auto_notes: true,
            speech_model: default_speech_model(),
            setup_done: false,
            has_key: false,
            provider: None,
            openrouter_model: None,
            custom_url: None,
            custom_model: None,
        }
    }
}

impl Settings {
    /// Fold the old provider/openrouterModel/customUrl shape into `base_url` + `model`.
    /// Returns the migrated settings and the legacy key slot that belonged to them, if any.
    fn migrated(mut self) -> (Self, Option<&'static str>) {
        let mut legacy_slot = None;
        if self.base_url.trim().is_empty() {
            match self.provider.as_deref() {
                Some("custom") | Some("local") => {
                    self.base_url = self.custom_url.clone().unwrap_or_default();
                    self.model = self.custom_model.clone().unwrap_or_default();
                    legacy_slot = Some(LEGACY_KEY_CUSTOM);
                }
                _ => {
                    self.base_url = OPENROUTER_URL.into();
                    self.model = self.openrouter_model.clone().unwrap_or_else(|| Settings::default().model);
                    legacy_slot = Some(LEGACY_KEY_OPENROUTER);
                }
            }
        }
        self.provider = None;
        self.openrouter_model = None;
        self.custom_url = None;
        self.custom_model = None;
        (self, legacy_slot)
    }
}

fn enabled() -> bool {
    true
}

fn auto_language() -> String {
    "auto".into()
}

fn default_speech_model() -> String {
    models::DEFAULT_SPEECH_MODEL.into()
}

/// The Credential Manager entry for a server: one key per host, so each provider keeps its own
/// and a key is never sent to a server it wasn't saved for.
fn key_slot(base_url: &str) -> String {
    let t = base_url.trim();
    let rest = t.split_once("://").map_or(t, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    format!("key:{}", host.to_ascii_lowercase())
}

struct Recording {
    /// Dropping it stops capture and joins the worker threads.
    _pipeline: Pipeline,
    meeting_id: i64,
}

struct AppState {
    store: Mutex<Store>,
    recording: Mutex<Option<Recording>>,
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    /// Set when `settings.json` couldn't be read at startup; Settings shows it until the next save.
    settings_notice: Mutex<Option<String>>,
    /// A model download is running, and the flag that stops it.
    downloading: std::sync::atomic::AtomicBool,
    download_cancel: std::sync::atomic::AtomicBool,
    /// What the Meet extension reports: who is speaking, and whether a call is on.
    meet: Mutex<MeetState>,
    /// Last known acli login state; refreshed when Settings opens.
    atlassian: Mutex<fs_atlassian::Status>,
}

#[derive(Default)]
struct MeetState {
    speakers: SpeakerLog,
    /// Pairing secret the extension must present.
    token: String,
    port: u16,
    /// Why the local endpoint couldn't start, if it couldn't.
    error: Option<String>,
    last_seen: i64,
    in_call: bool,
    call_title: Option<String>,
}

/// Extension events older than this mean the browser isn't sending any (tab closed, extension off).
const MEET_STALE_MS: i64 = 45_000;

fn pairing_token() -> String {
    if let Some(t) = stored_key("bridge-token") {
        return t;
    }
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        let n = now_ms() as u128 * 0x9E37_79B9_7F4A_7C15;
        bytes.copy_from_slice(&n.to_le_bytes());
    }
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let _ = keyring::Entry::new(KEYRING_SERVICE, "bridge-token").and_then(|e| e.set_password(&token));
    token
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CallEvent {
    state: &'static str,
    title: Option<String>,
}

fn handle_meet_event(app: &AppHandle, ev: Incoming) {
    let st = app.state::<AppState>();
    let now = now_ms();
    let mut call: Option<CallEvent> = None;
    {
        let mut meet = lock(&st.meet);
        meet.last_seen = now;
        match ev {
            Incoming::Hello => {}
            Incoming::Speaking { name, speaking, at } => {
                // Trust the browser's clock only if it agrees with ours; otherwise use ours.
                let at = if (at - now).abs() < 5_000 { at } else { now };
                meet.speakers.apply(&name, speaking, at);
            }
            Incoming::Call { state, title, .. } => match state {
                CallState::Joined => {
                    if !meet.in_call {
                        call = Some(CallEvent { state: "joined", title: title.clone() });
                    }
                    meet.in_call = true;
                    meet.call_title = title;
                }
                CallState::Left => {
                    if meet.in_call {
                        call = Some(CallEvent { state: "left", title: meet.call_title.clone() });
                    }
                    meet.in_call = false;
                    meet.speakers.stop_all(now);
                }
            },
        }
    }
    if let Some(c) = call {
        let _ = app.emit("call", c);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeStatus {
    port: u16,
    token: String,
    /// The extension has been heard from recently.
    connected: bool,
    in_call: bool,
    call_title: Option<String>,
    error: Option<String>,
}

#[tauri::command]
fn get_bridge(state: State<'_, AppState>) -> BridgeStatus {
    let m = lock(&state.meet);
    BridgeStatus {
        port: m.port,
        token: m.token.clone(),
        connected: m.error.is_none() && now_ms() - m.last_seen < MEET_STALE_MS,
        in_call: m.in_call && now_ms() - m.last_seen < MEET_STALE_MS,
        call_title: m.call_title.clone(),
        error: m.error.clone(),
    }
}

/// Where the speech models live. Order: `FIVESIGHT_MODELS`, `%LOCALAPPDATA%\fivesight\models`,
/// then the repo's `models/` folder (development).
/// The chosen Whisper model and the voice-activity model, if both are on disk.
fn model_paths(s: &Settings) -> Result<(PathBuf, PathBuf), String> {
    let chosen = models::speech_model(&s.speech_model)
        .ok_or_else(|| format!("Unknown speech model \"{}\". Choose one in Settings → Run setup.", s.speech_model))?;
    let whisper = models::find(&chosen.file).ok_or_else(|| {
        format!("The speech model ({}) isn't downloaded yet. Open Settings → Run setup to get it.", chosen.label)
    })?;
    let vad = models::find(&models::VAD)
        .ok_or("The voice detection model isn't downloaded yet. Open Settings → Run setup to get it.")?;
    Ok((whisper, vad))
}

fn stored_key(user: &str) -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, user).ok()?.get_password().ok().filter(|k| !k.trim().is_empty())
}

fn build_config(s: &Settings, key: Option<String>) -> Result<LlmConfig, String> {
    let url = s.base_url.trim();
    if url.is_empty() || s.model.trim().is_empty() {
        return Err("Set the server address and model name in Settings, under AI model.".into());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("The server address must start with http:// or https://.".into());
    }
    // The key is optional: local servers don't need one, hosted services do.
    let mut cfg = LlmConfig::custom(url, key, &s.model);
    cfg.fallback_model = Some(s.fallback_model.trim().to_string()).filter(|m| !m.is_empty());
    Ok(cfg)
}

fn llm_config(state: &AppState) -> Result<LlmConfig, String> {
    let s = lock(&state.settings).clone();
    let key = stored_key(&key_slot(&s.base_url));
    build_config(&s, key)
}

fn lines(transcript: &[Segment]) -> Vec<prompts::Line> {
    transcript
        .iter()
        .map(|s| prompts::Line {
            speaker: match (s.source.as_str(), &s.speaker) {
                ("mic", _) => "You".into(),
                (_, Some(name)) => name.clone(),
                _ => "Others".into(),
            },
            start_ms: s.start_ms.max(0) as u64,
            text: s.text.clone(),
        })
        .collect()
}

// ---------- status ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    recording: bool,
    recording_meeting_id: Option<i64>,
    models_dir: Option<String>,
}

#[tauri::command]
fn get_status(state: State<'_, AppState>) -> Status {
    let rec = lock(&state.recording);
    Status {
        recording: rec.is_some(),
        recording_meeting_id: rec.as_ref().map(|r| r.meeting_id),
        models_dir: model_paths(&lock(&state.settings))
            .ok()
            .and_then(|(w, _)| w.parent().map(|p| p.display().to_string())),
    }
}

// ---------- meetings ----------

#[derive(Serialize)]
struct MeetingDetail {
    meeting: Meeting,
    transcript: Vec<Segment>,
    chat: Vec<ChatMessage>,
}

#[tauri::command]
fn new_meeting(state: State<'_, AppState>) -> Result<i64, String> {
    let language = lock(&state.settings).language.clone();
    lock(&state.store).create_meeting_in("Untitled meeting", now_ms(), &language).map_err(err)
}

/// Choose what the speech recognizer expects for this meeting. Takes effect the next time
/// recording starts (the running pipeline keeps the language it began with).
#[tauri::command]
fn set_meeting_language(state: State<'_, AppState>, id: i64, language: String) -> Result<(), String> {
    if !matches!(language.as_str(), "auto" | "id" | "en") {
        return Err(format!("Unknown language \"{language}\"."));
    }
    lock(&state.store).set_language(id, &language).map_err(err)
}

#[tauri::command]
fn list_meetings(state: State<'_, AppState>) -> Result<Vec<MeetingSummary>, String> {
    lock(&state.store).list_meetings().map_err(err)
}

#[tauri::command]
fn get_meeting(state: State<'_, AppState>, id: i64) -> Result<MeetingDetail, String> {
    let st = lock(&state.store);
    Ok(MeetingDetail {
        meeting: st.get_meeting(id).map_err(err)?,
        transcript: st.transcript(id).map_err(err)?,
        chat: st.chat_history(Some(id)).map_err(err)?,
    })
}

#[tauri::command]
fn set_notes(state: State<'_, AppState>, id: i64, notes: String) -> Result<(), String> {
    lock(&state.store).set_notes(id, &notes).map_err(err)
}

#[tauri::command]
fn rename_meeting(state: State<'_, AppState>, id: i64, title: String) -> Result<(), String> {
    lock(&state.store).rename_meeting(id, &title).map_err(err)
}

#[tauri::command]
fn delete_meeting(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    if lock(&state.recording).as_ref().is_some_and(|r| r.meeting_id == id) {
        return Err("Stop recording before deleting this meeting.".into());
    }
    lock(&state.store).delete_meeting(id).map_err(err)
}

#[tauri::command]
fn search(state: State<'_, AppState>, query: String) -> Result<Vec<SearchHit>, String> {
    lock(&state.store).search(&query, 30).map_err(err)
}

// ---------- recording ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptPayload<'a> {
    #[serde(flatten)]
    ev: &'a TranscriptEvent,
    meeting_id: i64,
    /// Who was speaking, from the Meet extension. Only for what other people said.
    speaker: Option<String>,
}

#[tauri::command]
async fn start_recording(app: AppHandle, state: State<'_, AppState>, meeting_id: Option<i64>) -> Result<i64, String> {
    if lock(&state.recording).is_some() {
        return Err("Already recording.".into());
    }
    let (whisper_path, vad_path) = model_paths(&lock(&state.settings))?;
    let default_language = lock(&state.settings).language.clone();

    // Reuse the meeting or create one. Times in the transcript are measured from when the
    // meeting was created, and segment ids continue after any earlier recording in it.
    let (mid, offset_ms, seg_base, language, meeting_started) = {
        let st = lock(&state.store);
        let id = match meeting_id {
            Some(id) => {
                st.reopen_meeting(id).map_err(err)?;
                id
            }
            None => st.create_meeting_in("Untitled meeting", now_ms(), &default_language).map_err(err)?,
        };
        let meeting = st.get_meeting(id).map_err(err)?;
        let base = st.transcript(id).map_err(err)?.iter().map(|s| s.seg_id).max().unwrap_or(0);
        (id, (now_ms() - meeting.started_at).max(0) as u64, base as u64, meeting.language, meeting.started_at)
    };
    tracing::info!("recording with {}", whisper_path.display());
    let mut cfg = PipelineConfig::new(whisper_path, vad_path);
    cfg.language = LangMode::parse(&language);

    let (tx, rx) = std::sync::mpsc::channel::<TranscriptEvent>();
    // Loading the model takes a few seconds; keep it off the async runtime.
    let pipeline =
        tauri::async_runtime::spawn_blocking(move || Pipeline::start(cfg, tx)).await.map_err(err)?.map_err(err)?;
    *lock(&state.recording) = Some(Recording { _pipeline: pipeline, meeting_id: mid });

    let forward = app.clone();
    std::thread::spawn(move || {
        for mut ev in rx {
            if let Some(why) = ev.dropped.take() {
                tracing::debug!("dropped final ({why}): {}", ev.text);
                // Still tell the UI, so it clears the partial it is showing for this line.
                ev.text.clear();
            }
            ev.start_ms += offset_ms;
            ev.segment_id += seg_base;
            // Other people's lines get the name of whoever Meet showed as speaking meanwhile.
            let speaker = match ev.source {
                Source::Loopback => {
                    let st = forward.state::<AppState>();
                    let wall_start = meeting_started + ev.start_ms as i64;
                    let found = lock(&st.meet).speakers.speaker_for(wall_start, now_ms());
                    found
                }
                Source::Mic => None,
            };
            if ev.kind == Kind::Final && !ev.text.trim().is_empty() {
                let source = match ev.source {
                    Source::Mic => "mic",
                    Source::Loopback => "loopback",
                };
                let st = forward.state::<AppState>();
                let saved = lock(&st.store).add_segment_by(
                    mid,
                    source,
                    ev.segment_id as i64,
                    ev.start_ms as i64,
                    &ev.text,
                    speaker.as_deref(),
                );
                if let Err(e) = saved {
                    tracing::error!("saving transcript line: {e}");
                }
            }
            let _ = forward.emit("transcript", &TranscriptPayload { ev: &ev, meeting_id: mid, speaker });
        }
    });
    let _ = app.emit("recording", true);
    Ok(mid)
}

#[tauri::command]
async fn stop_recording(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let rec = lock(&state.recording).take();
    if let Some(rec) = rec {
        let mid = rec.meeting_id;
        // Dropping joins the audio and worker threads; the last transcript lines flush first.
        tauri::async_runtime::spawn_blocking(move || drop(rec)).await.map_err(err)?;
        lock(&state.store).end_meeting(mid, now_ms()).map_err(err)?;
    }
    let _ = app.emit("recording", false);
    Ok(())
}

// ---------- AI ----------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StreamPayload {
    meeting_id: Option<i64>,
    delta: String,
    done: bool,
    error: Option<String>,
}

fn emit_stream(app: &AppHandle, event: &str, meeting_id: Option<i64>, delta: &str, done: bool, error: Option<String>) {
    let _ = app.emit(event, StreamPayload { meeting_id, delta: delta.into(), done, error });
}

#[tauri::command]
/// Write the AI notes. `update` builds on the last AI notes instead of starting over; the
/// automatic passes during a meeting use it so the notes don't churn.
async fn enhance_notes(app: AppHandle, state: State<'_, AppState>, id: i64, update: bool) -> Result<(), String> {
    let cfg = llm_config(&state)?;
    let (meeting, transcript) = {
        let st = lock(&state.store);
        (st.get_meeting(id).map_err(err)?, st.transcript(id).map_err(err)?)
    };
    let live = lock(&state.recording).as_ref().is_some_and(|r| r.meeting_id == id);
    let previous = (update && !meeting.enhanced.trim().is_empty()).then_some(meeting.enhanced.as_str());
    let msgs = prompts::enhance_notes(
        &meeting.title,
        &meeting.notes,
        &prompts::format_transcript(&lines(&transcript), 60_000),
        prompts::EnhanceMode { live, previous },
    );
    tracing::debug!("enhancing meeting {id} (live: {live}, update: {})", previous.is_some());
    let key_sent = cfg.api_key.is_some();

    tauri::async_runtime::spawn(async move {
        let result = chat_stream(&cfg, &msgs, |d| emit_stream(&app, "enhance-delta", Some(id), d, false, None)).await;
        match result {
            Ok(full) => {
                let st = app.state::<AppState>();
                let saved = lock(&st.store).set_enhanced(id, &full);
                if let Err(e) = saved {
                    tracing::error!("saving AI notes: {e}");
                }
                emit_stream(&app, "enhance-delta", Some(id), "", true, None);
            }
            Err(e) => emit_stream(&app, "enhance-delta", Some(id), "", false, Some(e.user_message(key_sent))),
        }
    });
    Ok(())
}

#[tauri::command]
async fn ask(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: Option<i64>,
    question: String,
) -> Result<(), String> {
    let cfg = llm_config(&state)?;
    let now = now_ms();
    let key_sent = cfg.api_key.is_some();
    let sources: Vec<String>;
    let msgs = {
        let st = lock(&state.store);
        let history: Vec<Message> = st
            .chat_history(meeting_id)
            .map_err(err)?
            .into_iter()
            .map(|m| Message { role: m.role, content: m.content, ..Default::default() })
            .collect();
        st.add_chat(meeting_id, "user", &question, now).map_err(err)?;
        let (mut docs, used) = file_context(&st, &question).map_err(err)?;
        sources = used;
        match meeting_id {
            Some(id) => {
                let m = st.get_meeting(id).map_err(err)?;
                let t = st.transcript(id).map_err(err)?;
                prompts::ask(&question, &m.notes, &prompts::format_transcript(&lines(&t), 60_000), &docs, &history)
            }
            None => {
                // No meeting open: ground the answer in the best-matching transcript lines too.
                let mut found: Vec<prompts::ContextDoc> = st
                    .search_relevant(&question, 8)
                    .map_err(err)?
                    .into_iter()
                    .map(|h| prompts::ContextDoc {
                        title: format!("{} at {}", h.meeting_title, clock_label(h.start_ms)),
                        text: h.snippet,
                    })
                    .collect();
                found.append(&mut docs);
                prompts::ask(&question, "", "", &found, &history)
            }
        }
    };
    let _ = app.emit("ask-sources", AskSources { meeting_id, files: sources });
    // Jira through acli, Confluence through confluence-cli; each only when it's there and set up.
    let enabled = lock(&state.settings).atlassian;
    let jira = enabled && fs_atlassian::find_acli().is_some();
    let confluence = enabled && lock(&state.atlassian).confluence;

    tauri::async_runtime::spawn(async move {
        let mut msgs = msgs;
        if jira || confluence {
            let answered = lookups::run_lookups(&app, meeting_id, &cfg, &mut msgs, &question, jira, confluence).await;
            if let Some(text) = answered {
                // The AI finished while using tools; show it as one piece.
                emit_stream(&app, "ask-delta", meeting_id, &text, false, None);
                let st = app.state::<AppState>();
                let saved = lock(&st.store).add_chat(meeting_id, "assistant", &text, now_ms());
                if let Err(e) = saved {
                    tracing::error!("saving answer: {e}");
                }
                emit_stream(&app, "ask-delta", meeting_id, "", true, None);
                return;
            }
        }
        let result = chat_stream(&cfg, &msgs, |d| emit_stream(&app, "ask-delta", meeting_id, d, false, None)).await;
        match result {
            Ok(full) => {
                let st = app.state::<AppState>();
                let saved = lock(&st.store).add_chat(meeting_id, "assistant", &full, now_ms());
                if let Err(e) = saved {
                    tracing::error!("saving answer: {e}");
                }
                emit_stream(&app, "ask-delta", meeting_id, "", true, None);
            }
            Err(e) => emit_stream(&app, "ask-delta", meeting_id, "", false, Some(e.user_message(key_sent))),
        }
    });
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AskSources {
    meeting_id: Option<i64>,
    /// Titles of the files whose passages were given to the model.
    files: Vec<String>,
}

fn clock_label(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Up to five passages from the user's files that share words with the question,
/// capped so they never crowd out the meeting itself.
fn file_context(st: &Store, question: &str) -> fs_store::Result<(Vec<prompts::ContextDoc>, Vec<String>)> {
    const BUDGET: usize = 8_000;
    let mut docs = Vec::new();
    let mut titles: Vec<String> = Vec::new();
    let mut used = 0;
    for h in st.kb_search(question, 5)? {
        if used + h.text.len() > BUDGET {
            break;
        }
        used += h.text.len();
        if !titles.contains(&h.title) {
            titles.push(h.title.clone());
        }
        docs.push(prompts::ContextDoc { title: format!("file: {}", h.title), text: h.text });
    }
    Ok((docs, titles))
}

// ---------- jira & confluence ----------

/// Ask acli who is logged in (a second or two), remember it, and return it.
#[tauri::command]
async fn atlassian_status(app: AppHandle) -> Result<fs_atlassian::Status, String> {
    let s = tauri::async_runtime::spawn_blocking(fs_atlassian::status).await.map_err(err)?;
    *lock(&app.state::<AppState>().atlassian) = s.clone();
    Ok(s)
}

// ---------- knowledge base ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KbAddResult {
    path: String,
    title: Option<String>,
    error: Option<String>,
}

fn index_file(st: &AppState, path: &std::path::Path) -> Result<String, String> {
    let extracted = fs_kb::extract(path).map_err(err)?;
    let chunks = fs_kb::chunk(&extracted.text);
    let meta = std::fs::metadata(path).map_err(err)?;
    let modified =
        meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
    let shown = path.to_string_lossy().to_string();
    lock(&st.store)
        .kb_put(&shown, &extracted.title, extracted.kind.as_str(), meta.len() as i64, modified, now_ms(), &chunks)
        .map_err(err)?;
    Ok(extracted.title)
}

#[tauri::command]
fn kb_list(state: State<'_, AppState>) -> Result<Vec<KbDoc>, String> {
    lock(&state.store).kb_docs().map_err(err)
}

/// Read and index files. PDFs can take a moment, so this runs off the UI thread.
#[tauri::command]
async fn kb_add(app: AppHandle, paths: Vec<String>) -> Result<Vec<KbAddResult>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        paths
            .into_iter()
            .map(|p| match index_file(&st, std::path::Path::new(&p)) {
                Ok(title) => KbAddResult { path: p, title: Some(title), error: None },
                Err(e) => KbAddResult { path: p, title: None, error: Some(e) },
            })
            .collect()
    })
    .await
    .map_err(err)
}

/// Re-read a file that changed on disk.
#[tauri::command]
async fn kb_refresh(app: AppHandle, id: i64) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let path = lock(&st.store).kb_doc(id).map_err(err)?.path;
        index_file(&st, std::path::Path::new(&path)).map(|_| ())
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn kb_remove(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    lock(&state.store).kb_remove(id).map_err(err)
}

// ---------- settings ----------

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    let mut s = lock(&state.settings).clone();
    s.has_key = stored_key(&key_slot(&s.base_url)).is_some();
    s
}

/// Whether a key is saved for this server (the settings form asks while you type the address).
#[tauri::command]
fn key_saved(base_url: String) -> bool {
    stored_key(&key_slot(&base_url)).is_some()
}

#[tauri::command]
fn forget_key(base_url: String) -> Result<(), String> {
    match keyring::Entry::new(KEYRING_SERVICE, &key_slot(&base_url)).map_err(err)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Couldn't remove the key: {e}")),
    }
}

/// `api_key` is saved for the server address being saved; keys for other servers are kept.
#[tauri::command]
fn save_settings(state: State<'_, AppState>, settings: Settings, api_key: Option<String>) -> Result<(), String> {
    let (mut to_store, _) = settings.migrated();
    to_store.base_url = to_store.base_url.trim().to_string();
    to_store.model = to_store.model.trim().to_string();
    to_store.fallback_model = to_store.fallback_model.trim().to_string();
    if models::speech_model(&to_store.speech_model).is_none() {
        return Err(format!("Unknown speech model \"{}\".", to_store.speech_model));
    }
    for (field, value) in [("Model", &to_store.model), ("Fallback model", &to_store.fallback_model)] {
        if looks_like_address(value) {
            return Err(format!(
                "{field} should be a model name, not a server address. Put the address in Server address."
            ));
        }
    }
    if let Some(key) = api_key.filter(|k| !k.trim().is_empty()) {
        keyring::Entry::new(KEYRING_SERVICE, &key_slot(&to_store.base_url))
            .and_then(|e| e.set_password(key.trim()))
            .map_err(|e| format!("Couldn't save the key to Windows Credential Manager: {e}"))?;
    }
    to_store.has_key = false;
    std::fs::write(&state.settings_path, serde_json::to_string_pretty(&to_store).map_err(err)?)
        .map_err(|e| format!("Couldn't save settings to {}: {e}", state.settings_path.display()))?;
    *lock(&state.settings) = to_store;
    *lock(&state.settings_notice) = None;
    tracing::info!("settings saved");
    Ok(())
}

/// Why settings were reset to defaults at startup, if they were.
#[tauri::command]
fn settings_notice(state: State<'_, AppState>) -> Option<String> {
    lock(&state.settings_notice).clone()
}

/// Model names never contain "://" or start with "www."; server addresses do.
fn looks_like_address(s: &str) -> bool {
    let s = s.trim().to_ascii_lowercase();
    s.contains("://") || s.starts_with("www.")
}

/// Read `settings.json`. A file that can't be parsed is copied to `settings.broken.json`
/// (so a later save doesn't destroy it) and defaults are used, with a notice for Settings.
fn load_settings(path: &std::path::Path) -> (Option<Settings>, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (None, None),
        Err(e) => {
            tracing::warn!("reading {}: {e}", path.display());
            return (None, Some(format!("Couldn't read your settings file, so defaults are in use. ({e})")));
        }
    };
    match serde_json::from_str::<Settings>(&text) {
        Ok(s) => (Some(s), None),
        Err(e) => {
            let backup = path.with_file_name("settings.broken.json");
            let _ = std::fs::copy(path, &backup);
            tracing::warn!("parsing {}: {e}; copied to {}", path.display(), backup.display());
            (
                None,
                Some(format!(
                    "Your settings file couldn't be read, so defaults are in use. The old file was kept as {}. ({e})",
                    backup.display()
                )),
            )
        }
    }
}

/// Try the model with a one-word request, using the form's values (a typed key wins over a saved one).
/// Nothing is saved.
#[tauri::command]
async fn test_llm(settings: Settings, api_key: Option<String>) -> Result<String, String> {
    let (s, _) = settings.migrated();
    let typed = api_key.filter(|k| !k.trim().is_empty());
    let key = typed.or_else(|| stored_key(&key_slot(&s.base_url)));
    let key_sent = key.is_some();
    let mut cfg = build_config(&s, key)?;
    cfg.fallback_model = None;
    let msgs = [Message::user("Reply with the single word: ok")];
    match chat_stream(&cfg, &msgs, |_| {}).await {
        Ok(reply) => {
            Ok(format!("Connected. {} replied: {}", cfg.model, reply.trim().chars().take(60).collect::<String>()))
        }
        Err(e) => Err(e.user_message(key_sent)),
    }
}

// ---------- first-run setup ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SpeechModelInfo {
    id: &'static str,
    label: &'static str,
    note: &'static str,
    vram_mb: u64,
    bytes: u64,
    /// Already on disk, nothing to download.
    present: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupInfo {
    system: system::SystemInfo,
    recommendation: models::Recommendation,
    speech_models: Vec<SpeechModelInfo>,
    vad_present: bool,
    download_dir: Option<String>,
}

/// Look at this PC and say which speech model suits it.
#[tauri::command]
async fn setup_info() -> Result<SetupInfo, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let dir = models::download_dir().unwrap_or_else(std::env::temp_dir);
        let system = system::detect(&dir);
        tracing::info!(
            "system: {} ({} threads), {} MB RAM, gpus {:?}, {:?} MB free",
            system.cpu,
            system.cores,
            system.ram_mb,
            system.gpus,
            system.free_disk_mb
        );
        SetupInfo {
            recommendation: models::recommend(&system),
            system,
            speech_models: models::SPEECH_MODELS
                .iter()
                .map(|m| SpeechModelInfo {
                    id: m.id,
                    label: m.label,
                    note: m.note,
                    vram_mb: m.vram_mb,
                    bytes: m.file.bytes,
                    present: models::find(&m.file).is_some(),
                })
                .collect(),
            vad_present: models::find(&models::VAD).is_some(),
            download_dir: models::download_dir().map(|d| d.display().to_string()),
        }
    })
    .await
    .map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalServer {
    name: &'static str,
    base_url: &'static str,
    models: Vec<String>,
}

/// AI servers already running on this PC (Ollama, LM Studio) and the models they have.
#[tauri::command]
async fn detect_local_llms() -> Vec<LocalServer> {
    let Ok(client) = reqwest::Client::builder().timeout(std::time::Duration::from_millis(1500)).build() else {
        return Vec::new();
    };
    let get = |url: &'static str| {
        let client = client.clone();
        async move {
            let text = client.get(url).send().await.ok()?.error_for_status().ok()?.text().await.ok()?;
            serde_json::from_str::<serde_json::Value>(&text).ok()
        }
    };
    let names = |v: &serde_json::Value, list: &str, key: &str| -> Vec<String> {
        v[list].as_array().into_iter().flatten().filter_map(|m| m[key].as_str().map(str::to_string)).collect()
    };
    let (ollama, lmstudio) =
        futures_util::join!(get("http://127.0.0.1:11434/api/tags"), get("http://127.0.0.1:1234/v1/models"));
    let mut out = Vec::new();
    if let Some(v) = ollama {
        out.push(LocalServer {
            name: "Ollama",
            base_url: "http://localhost:11434/v1",
            models: names(&v, "models", "name"),
        });
    }
    if let Some(v) = lmstudio {
        out.push(LocalServer {
            name: "LM Studio",
            base_url: "http://localhost:1234/v1",
            models: names(&v, "data", "id"),
        });
    }
    out
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadEvent {
    file: String,
    /// Position of this file in the queue (0-based) and the queue length.
    index: usize,
    count: usize,
    received: u64,
    total: u64,
    /// Everything in the queue is downloaded.
    done: bool,
    error: Option<String>,
}

/// Download the voice detection model if it's missing, plus the chosen speech models that
/// aren't on disk yet. Progress arrives as "model-download" events. Returns the files queued.
#[tauri::command]
async fn download_models(
    app: AppHandle,
    state: State<'_, AppState>,
    speech_models: Vec<String>,
) -> Result<Vec<String>, String> {
    let mut queue: Vec<models::ModelFile> = Vec::new();
    if models::find(&models::VAD).is_none() {
        queue.push(models::VAD);
    }
    for id in &speech_models {
        let m = models::speech_model(id).ok_or_else(|| format!("Unknown speech model \"{id}\"."))?;
        if models::find(&m.file).is_none() && !queue.iter().any(|f| f.file == m.file.file) {
            queue.push(m.file);
        }
    }
    let dir = models::download_dir().ok_or("Couldn't find the local app data folder.")?;
    if state.downloading.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("A download is already running.".into());
    }
    state.download_cancel.store(false, std::sync::atomic::Ordering::SeqCst);
    let files: Vec<String> = queue.iter().map(|f| f.file.to_string()).collect();

    tauri::async_runtime::spawn(async move {
        let st = app.state::<AppState>();
        let count = queue.len();
        let ev = |file: &str, index: usize, received: u64, total: u64, done: bool, error: Option<String>| {
            let _ = app.emit(
                "model-download",
                DownloadEvent { file: file.into(), index, count, received, total, done, error },
            );
        };
        let client = reqwest::Client::new();
        let mut failed = false;
        for (index, f) in queue.iter().enumerate() {
            tracing::info!("downloading {} ({} bytes)", f.file, f.bytes);
            let result =
                models::download(&client, f, &dir, &st.download_cancel, |r, t| ev(f.file, index, r, t, false, None))
                    .await;
            if let Err(e) = result {
                tracing::warn!("download {}: {e}", f.file);
                ev(f.file, index, 0, f.bytes, false, Some(e));
                failed = true;
                break;
            }
        }
        if !failed {
            ev("", count, 0, 0, true, None);
        }
        st.downloading.store(false, std::sync::atomic::Ordering::SeqCst);
    });
    Ok(files)
}

/// Stop the running download. The partial file is kept so the next try resumes.
#[tauri::command]
fn cancel_download(state: State<'_, AppState>) {
    state.download_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
}

// ---------- windows ----------

/// Show the compact always-on-top live panel, creating it on first use.
#[tauri::command]
async fn open_live_panel(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("live") {
        w.show().map_err(err)?;
        return w.set_focus().map_err(err);
    }
    WebviewWindowBuilder::new(&app, "live", WebviewUrl::App("live.html".into()))
        .title("fivesight live")
        .inner_size(380.0, 520.0)
        .min_inner_size(300.0, 240.0)
        .decorations(false)
        .always_on_top(true)
        .build()
        .map(|_| ())
        .map_err(err)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_local_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            logging::init(&dir);
            let store = Store::open(&dir.join("fivesight.db"))?;
            let settings_path = dir.join("settings.json");
            let (loaded, settings_notice) = load_settings(&settings_path);
            let settings = match loaded {
                Some(old) => {
                    let (settings, legacy_slot) = old.migrated();
                    if let Some(slot) = legacy_slot {
                        // Carry the old key over to the per-server slot, then rewrite the file in the new shape.
                        let new_slot = key_slot(&settings.base_url);
                        if stored_key(&new_slot).is_none() {
                            if let Some(k) = stored_key(slot) {
                                let _ =
                                    keyring::Entry::new(KEYRING_SERVICE, &new_slot).and_then(|e| e.set_password(&k));
                            }
                        }
                        if let Ok(json) = serde_json::to_string_pretty(&settings) {
                            let _ = std::fs::write(&settings_path, json);
                        }
                    }
                    settings
                }
                None => Settings::default(),
            };
            app.manage(AppState {
                store: Mutex::new(store),
                recording: Mutex::new(None),
                settings: Mutex::new(settings),
                settings_path,
                settings_notice: Mutex::new(settings_notice),
                downloading: Default::default(),
                download_cancel: Default::default(),
                meet: Mutex::new(MeetState::default()),
                atlassian: Mutex::new(fs_atlassian::Status::default()),
            });
            // Asking acli who is logged in takes a second or two; don't hold up startup.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let s = fs_atlassian::status();
                *lock(&handle.state::<AppState>().atlassian) = s;
            });

            // The Meet extension reports speaker names here. Failing to listen is not fatal.
            let token = pairing_token();
            let handle = app.handle().clone();
            let started =
                Bridge::start(fs_bridge::DEFAULT_PORT, token.clone(), move |ev| handle_meet_event(&handle, ev));
            {
                let st = app.state::<AppState>();
                let mut meet = lock(&st.meet);
                meet.token = token;
                meet.port = fs_bridge::DEFAULT_PORT;
                match started {
                    Ok(b) => {
                        app.manage(b);
                    }
                    Err(e) => {
                        tracing::warn!("meet bridge: {e}");
                        meet.error = Some(format!(
                            "Couldn't listen on port {}. Another program may be using it. ({e})",
                            fs_bridge::DEFAULT_PORT
                        ));
                    }
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            new_meeting,
            list_meetings,
            get_meeting,
            set_notes,
            rename_meeting,
            set_meeting_language,
            delete_meeting,
            search,
            start_recording,
            stop_recording,
            enhance_notes,
            ask,
            get_settings,
            save_settings,
            settings_notice,
            setup_info,
            detect_local_llms,
            download_models,
            cancel_download,
            get_bridge,
            atlassian_status,
            kb_list,
            kb_add,
            kb_refresh,
            kb_remove,
            key_saved,
            forget_key,
            test_llm,
            open_live_panel
        ])
        .run(tauri::generate_context!())
        .expect("error while running fivesight");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Settings {
        serde_json::from_str(json).expect("settings json")
    }

    fn temp_settings(name: &str, contents: Option<&str>) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fivesight-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        if let Some(c) = contents {
            std::fs::write(&path, c).unwrap();
        }
        path
    }

    #[test]
    fn missing_fields_take_defaults() {
        let s = parse(r#"{"baseUrl":"http://gw:8080/v1","model":"m"}"#);
        assert_eq!(s.fallback_model, "");
        assert_eq!(s.language, "auto");
        assert!(s.atlassian);
        assert!(s.auto_notes);
    }

    #[test]
    fn missing_settings_file_is_not_a_problem() {
        let (s, notice) = load_settings(&temp_settings("missing", None));
        assert!(s.is_none() && notice.is_none());
    }

    #[test]
    fn broken_settings_file_is_kept_and_reported() {
        let path = temp_settings("broken", Some("{ not json"));
        let (s, notice) = load_settings(&path);
        assert!(s.is_none());
        assert!(notice.unwrap().contains("settings.broken.json"));
        assert_eq!(std::fs::read_to_string(path.with_file_name("settings.broken.json")).unwrap(), "{ not json");
    }

    #[test]
    fn spots_addresses_in_model_fields() {
        assert!(looks_like_address("http://asus-homelab:20128/v1"));
        assert!(looks_like_address(" HTTPS://openrouter.ai/api/v1"));
        assert!(looks_like_address("www.example.com"));
        assert!(!looks_like_address("google/gemini-2.5-flash"));
        assert!(!looks_like_address("ag/gemini-3.8-flash-medium"));
        assert!(!looks_like_address("llama3.1:8b"));
        assert!(!looks_like_address(""));
    }

    #[test]
    fn key_slot_is_per_host() {
        assert_eq!(key_slot("https://openrouter.ai/api/v1"), "key:openrouter.ai");
        assert_eq!(key_slot(" HTTPS://API.OpenAI.com/v1/ "), "key:api.openai.com");
        assert_eq!(key_slot("http://localhost:11434/v1"), "key:localhost:11434");
        assert_eq!(key_slot("https://user:pw@gw.example.com/x?y=1"), "key:gw.example.com");
        assert_ne!(key_slot("http://localhost:11434/v1"), key_slot("http://localhost:1234/v1"));
    }

    #[test]
    fn migrates_old_openrouter_settings() {
        let old = parse(
            r#"{"provider":"openrouter","openrouterModel":"anthropic/claude-sonnet-4.5","fallbackModel":"","customUrl":"http://x/v1","customModel":"m","language":"id"}"#,
        );
        let (s, slot) = old.migrated();
        assert_eq!(s.base_url, OPENROUTER_URL);
        assert_eq!(s.model, "anthropic/claude-sonnet-4.5");
        assert_eq!(s.language, "id");
        assert_eq!(slot, Some(LEGACY_KEY_OPENROUTER));
    }

    #[test]
    fn migrates_old_custom_and_local_settings() {
        for provider in ["custom", "local"] {
            let json = format!(
                r#"{{"provider":"{provider}","openrouterModel":"a","fallbackModel":"f","localUrl":"https://gw.example.com/v1","localModel":"gpt-x","language":"auto"}}"#
            );
            let (s, slot) = parse(&json).migrated();
            assert_eq!(s.base_url, "https://gw.example.com/v1");
            assert_eq!(s.model, "gpt-x");
            assert_eq!(s.fallback_model, "f");
            assert_eq!(slot, Some(LEGACY_KEY_CUSTOM));
        }
    }

    #[test]
    fn new_shape_is_left_alone_and_legacy_fields_are_not_written() {
        let (s, slot) =
            parse(r#"{"baseUrl":"https://api.groq.com/openai/v1","model":"llama","fallbackModel":"","language":"en"}"#)
                .migrated();
        assert_eq!(slot, None);
        assert_eq!(s.base_url, "https://api.groq.com/openai/v1");
        let out = serde_json::to_string(&s).unwrap();
        assert!(!out.contains("provider") && !out.contains("customUrl") && !out.contains("openrouterModel"));
    }

    #[test]
    fn build_config_validates_and_keeps_the_key_for_that_server() {
        let mut s = Settings {
            base_url: "https://api.openai.com/v1/chat/completions".into(),
            model: "gpt-4o-mini".into(),
            ..Settings::default()
        };
        let cfg = build_config(&s, Some("sk-test".into())).unwrap();
        assert_eq!(cfg.base_url, "https://api.openai.com/v1");
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test"));

        s.base_url = "api.openai.com/v1".into();
        assert!(build_config(&s, None).is_err());
        s.base_url = String::new();
        assert!(build_config(&s, None).is_err());
        s.base_url = "http://localhost:11434/v1".into();
        s.model = " ".into();
        assert!(build_config(&s, None).is_err());
    }
}
