//! fivesight desktop shell: owns the pipeline, the database and the LLM calls, and
//! forwards everything to the UI as commands and events.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use fs_audio::Source;
use fs_llm::{chat_stream, prompts, LlmConfig, Message};
use fs_pipeline::{Kind, Pipeline, PipelineConfig, TranscriptEvent};
use fs_stt::lang::LangMode;
use fs_store::{ChatMessage, Meeting, MeetingSummary, SearchHit, Segment, Store};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

const KEYRING_SERVICE: &str = "fivesight";
/// Credential Manager entries: one key per provider so switching doesn't lose either.
const KEY_OPENROUTER: &str = "openrouter";
const KEY_CUSTOM: &str = "custom";

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

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    /// "openrouter" or "custom" (any OpenAI-compatible server). Older files say "local".
    provider: String,
    openrouter_model: String,
    fallback_model: String,
    #[serde(alias = "localUrl")]
    custom_url: String,
    #[serde(alias = "localModel")]
    custom_model: String,
    /// Language given to new meetings: "auto", "id" or "en". Each meeting can override it.
    language: String,
    /// Filled in on read; never persisted (keys live in Windows Credential Manager).
    #[serde(default)]
    has_key: bool,
    #[serde(default)]
    has_custom_key: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: "openrouter".into(),
            openrouter_model: "google/gemini-2.5-flash".into(),
            fallback_model: String::new(),
            custom_url: "http://localhost:11434/v1".into(),
            custom_model: "gemma3".into(),
            language: "auto".into(),
            has_key: false,
            has_custom_key: false,
        }
    }
}

impl Settings {
    /// Settings files from before the provider was renamed say "local".
    fn normalized(mut self) -> Self {
        if self.provider == "local" {
            self.provider = "custom".into();
        }
        self
    }
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
}

/// Where the speech models live. Order: `FIVESIGHT_MODELS`, `%LOCALAPPDATA%\fivesight\models`,
/// then the repo's `models/` folder (development).
fn models_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("FIVESIGHT_MODELS") {
        candidates.push(PathBuf::from(p));
    }
    if let Ok(p) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(p).join("fivesight").join("models"));
    }
    candidates.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../models")));
    candidates
        .into_iter()
        .find(|d| d.join("ggml-large-v3-turbo.bin").exists() && d.join("silero_vad.onnx").exists())
}

fn stored_key(user: &str) -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, user).ok()?.get_password().ok().filter(|k| !k.trim().is_empty())
}

fn llm_config(state: &AppState) -> Result<LlmConfig, String> {
    let s = lock(&state.settings).clone();
    let fallback = Some(s.fallback_model.trim().to_string()).filter(|m| !m.is_empty());
    let mut cfg = if s.provider == "custom" {
        if s.custom_url.trim().is_empty() || s.custom_model.trim().is_empty() {
            return Err("Set the server address and model name in Settings, under AI model.".into());
        }
        // The key is optional: local servers don't need one, hosted services do.
        LlmConfig::custom(&s.custom_url, stored_key(KEY_CUSTOM), &s.custom_model)
    } else {
        let key = stored_key(KEY_OPENROUTER).ok_or("Add your OpenRouter API key in Settings, under AI model.")?;
        LlmConfig::openrouter(key, &s.openrouter_model)
    };
    cfg.fallback_model = fallback;
    Ok(cfg)
}

fn lines(transcript: &[Segment]) -> Vec<prompts::Line> {
    transcript
        .iter()
        .map(|s| prompts::Line {
            speaker: if s.source == "mic" { "You".into() } else { "Others".into() },
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
        models_dir: models_dir().map(|p| p.display().to_string()),
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
}

#[tauri::command]
async fn start_recording(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: Option<i64>,
) -> Result<i64, String> {
    if lock(&state.recording).is_some() {
        return Err("Already recording.".into());
    }
    let dir = models_dir().ok_or(
        "Speech models not found. Put ggml-large-v3-turbo.bin and silero_vad.onnx in %LOCALAPPDATA%\\fivesight\\models.",
    )?;
    let default_language = lock(&state.settings).language.clone();

    // Reuse the meeting or create one. Times in the transcript are measured from when the
    // meeting was created, and segment ids continue after any earlier recording in it.
    let (mid, offset_ms, seg_base, language) = {
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
        (id, (now_ms() - meeting.started_at).max(0) as u64, base as u64, meeting.language)
    };
    let mut cfg = PipelineConfig::new(dir.join("ggml-large-v3-turbo.bin"), dir.join("silero_vad.onnx"));
    cfg.language = LangMode::parse(&language);

    let (tx, rx) = std::sync::mpsc::channel::<TranscriptEvent>();
    // Loading the model takes a few seconds; keep it off the async runtime.
    let pipeline = tauri::async_runtime::spawn_blocking(move || Pipeline::start(cfg, tx))
        .await
        .map_err(err)?
        .map_err(err)?;
    *lock(&state.recording) = Some(Recording { _pipeline: pipeline, meeting_id: mid });

    let forward = app.clone();
    std::thread::spawn(move || {
        for mut ev in rx {
            if let Some(why) = &ev.dropped {
                tracing::debug!("dropped final: {why}");
                continue;
            }
            ev.start_ms += offset_ms;
            ev.segment_id += seg_base;
            if ev.kind == Kind::Final && !ev.text.trim().is_empty() {
                let source = match ev.source {
                    Source::Mic => "mic",
                    Source::Loopback => "loopback",
                };
                let st = forward.state::<AppState>();
                let saved = lock(&st.store).add_segment(mid, source, ev.segment_id as i64, ev.start_ms as i64, &ev.text);
                if let Err(e) = saved {
                    tracing::error!("saving transcript line: {e}");
                }
            }
            let _ = forward.emit("transcript", &TranscriptPayload { ev: &ev, meeting_id: mid });
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
async fn enhance_notes(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let cfg = llm_config(&state)?;
    let (meeting, transcript) = {
        let st = lock(&state.store);
        (st.get_meeting(id).map_err(err)?, st.transcript(id).map_err(err)?)
    };
    let msgs = prompts::enhance_notes(&meeting.title, &meeting.notes, &prompts::format_transcript(&lines(&transcript), 60_000));
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
async fn ask(app: AppHandle, state: State<'_, AppState>, meeting_id: Option<i64>, question: String) -> Result<(), String> {
    let cfg = llm_config(&state)?;
    let now = now_ms();
    let key_sent = cfg.api_key.is_some();
    let msgs = {
        let st = lock(&state.store);
        let history: Vec<Message> = st
            .chat_history(meeting_id)
            .map_err(err)?
            .into_iter()
            .map(|m| Message { role: m.role, content: m.content })
            .collect();
        st.add_chat(meeting_id, "user", &question, now).map_err(err)?;
        match meeting_id {
            Some(id) => {
                let m = st.get_meeting(id).map_err(err)?;
                let t = st.transcript(id).map_err(err)?;
                prompts::ask(&question, &m.notes, &prompts::format_transcript(&lines(&t), 60_000), &[], &history)
            }
            None => {
                // No meeting open: ground the answer in the best-matching transcript lines.
                let docs: Vec<prompts::ContextDoc> = st
                    .search(&question, 8)
                    .map_err(err)?
                    .into_iter()
                    .map(|h| prompts::ContextDoc { title: format!("{} at {}", h.meeting_title, h.start_ms / 1000), text: h.snippet })
                    .collect();
                prompts::ask(&question, "", "", &docs, &history)
            }
        }
    };

    tauri::async_runtime::spawn(async move {
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

// ---------- settings ----------

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    let mut s = lock(&state.settings).clone();
    s.has_key = stored_key(KEY_OPENROUTER).is_some();
    s.has_custom_key = stored_key(KEY_CUSTOM).is_some();
    s
}

/// `api_key` is saved for whichever provider is being saved; the other provider's key is kept.
#[tauri::command]
fn save_settings(state: State<'_, AppState>, settings: Settings, api_key: Option<String>) -> Result<(), String> {
    let mut to_store = settings.normalized();
    if let Some(key) = api_key.filter(|k| !k.trim().is_empty()) {
        let slot = if to_store.provider == "custom" { KEY_CUSTOM } else { KEY_OPENROUTER };
        keyring::Entry::new(KEYRING_SERVICE, slot)
            .and_then(|e| e.set_password(key.trim()))
            .map_err(|e| format!("Couldn't save the key to Windows Credential Manager: {e}"))?;
    }
    to_store.has_key = false;
    to_store.has_custom_key = false;
    std::fs::write(&state.settings_path, serde_json::to_string_pretty(&to_store).map_err(err)?).map_err(err)?;
    *lock(&state.settings) = to_store;
    Ok(())
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
        .setup(|app| {
            let dir = app.path().app_local_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let store = Store::open(&dir.join("fivesight.db"))?;
            let settings_path = dir.join("settings.json");
            let settings = std::fs::read_to_string(&settings_path)
                .ok()
                .and_then(|t| serde_json::from_str::<Settings>(&t).ok())
                .map(Settings::normalized)
                .unwrap_or_default();
            app.manage(AppState {
                store: Mutex::new(store),
                recording: Mutex::new(None),
                settings: Mutex::new(settings),
                settings_path,
            });
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
            open_live_panel
        ])
        .run(tauri::generate_context!())
        .expect("error while running fivesight");
}
