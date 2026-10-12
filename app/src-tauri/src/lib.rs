//! fivesight desktop shell: owns the pipeline and forwards transcript events to the UI.

use std::path::PathBuf;
use std::sync::Mutex;

use fs_pipeline::{Pipeline, PipelineConfig};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

#[derive(Default)]
struct Recorder {
    pipeline: Mutex<Option<Pipeline>>,
}

/// Where the models live. Order: `FIVESIGHT_MODELS`, `%LOCALAPPDATA%\fivesight\models`,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    recording: bool,
    models_dir: Option<String>,
}

#[tauri::command]
fn get_status(rec: State<'_, Recorder>) -> Status {
    Status {
        recording: rec.pipeline.lock().unwrap().is_some(),
        models_dir: models_dir().map(|p| p.display().to_string()),
    }
}

#[tauri::command]
async fn start_recording(
    app: AppHandle,
    rec: State<'_, Recorder>,
    language: Option<String>,
) -> Result<(), String> {
    if rec.pipeline.lock().unwrap().is_some() {
        return Ok(());
    }
    let dir = models_dir().ok_or("models not found: put ggml-large-v3-turbo.bin and silero_vad.onnx in %LOCALAPPDATA%\\fivesight\\models")?;
    let mut cfg = PipelineConfig::new(dir.join("ggml-large-v3-turbo.bin"), dir.join("silero_vad.onnx"));
    cfg.language = language.filter(|l| l != "auto");

    let (tx, rx) = std::sync::mpsc::channel();
    // Loading the model takes a few seconds; keep it off the async runtime.
    let pipeline = tauri::async_runtime::spawn_blocking(move || Pipeline::start(cfg, tx))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    *rec.pipeline.lock().unwrap() = Some(pipeline);

    let forward = app.clone();
    std::thread::spawn(move || {
        for ev in rx {
            if let Some(why) = &ev.dropped {
                tracing::debug!("dropped final: {why}");
                continue;
            }
            let _ = forward.emit("transcript", &ev);
        }
    });
    let _ = app.emit("recording", true);
    Ok(())
}

#[tauri::command]
async fn stop_recording(app: AppHandle, rec: State<'_, Recorder>) -> Result<(), String> {
    let pipeline = rec.pipeline.lock().unwrap().take();
    if let Some(p) = pipeline {
        // Dropping joins the audio and worker threads.
        tauri::async_runtime::spawn_blocking(move || drop(p))
            .await
            .map_err(|e| e.to_string())?;
    }
    let _ = app.emit("recording", false);
    Ok(())
}

/// Show the compact always-on-top live panel, creating it on first use.
#[tauri::command]
async fn open_live_panel(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("live") {
        w.show().map_err(|e| e.to_string())?;
        return w.set_focus().map_err(|e| e.to_string());
    }
    WebviewWindowBuilder::new(&app, "live", WebviewUrl::App("live.html".into()))
        .title("fivesight live")
        .inner_size(380.0, 520.0)
        .min_inner_size(300.0, 240.0)
        .decorations(false)
        .always_on_top(true)
        .build()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn run() {
    tauri::Builder::default()
        .manage(Recorder::default())
        .invoke_handler(tauri::generate_handler![
            get_status,
            start_recording,
            stop_recording,
            open_live_panel
        ])
        .run(tauri::generate_context!())
        .expect("error while running fivesight");
}
