//! Qwen3-ASR through llama.cpp's `llama-server`, run as a child process on a local port.
//!
//! The prompt is built the way Qwen's own `qwen-asr` package builds it: the meeting hints as the
//! system message and the audio as the user's message. The model answers
//! `language Indonesian<asr_text>what was said`; a forced language is prefilled as the start of
//! the answer, so only the text comes back.
//!
//! Why a separate process and not bindings: llama.cpp's audio input is still marked
//! experimental and changes quickly; a pinned server binary can be swapped without rebuilding,
//! and a crash in it doesn't take the app down. On Windows the server is tied to this process
//! with a job object, so it can't outlive the app and keep holding GPU memory.

use std::io::Cursor;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine as _;

use crate::engine::{EngineError, Segment};

/// Files needed to run Qwen3-ASR.
#[derive(Debug, Clone)]
pub struct QwenConfig {
    /// `llama-server` executable (with its DLLs beside it).
    pub server: PathBuf,
    /// The model, e.g. `Qwen3-ASR-1.7B-Q8_0.gguf`.
    pub model: PathBuf,
    /// Its audio encoder, e.g. `mmproj-Qwen3-ASR-1.7B-Q8_0.gguf`.
    pub mmproj: PathBuf,
    /// Run on the GPU (all layers) or the CPU.
    pub gpu: bool,
}

/// How long the server may take to load the model.
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// Longest wait for one line; lines are at most ~30 s of audio.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_TOKENS: u32 = 448;

pub struct QwenEngine {
    child: Child,
    base: String,
    agent: ureq::Agent,
    #[cfg(windows)]
    _job: job::Job,
}

impl QwenEngine {
    /// Start the server and wait until the model is loaded.
    pub fn start(cfg: &QwenConfig) -> Result<Self, EngineError> {
        for (what, p) in [("llama-server", &cfg.server), ("model", &cfg.model), ("audio encoder", &cfg.mmproj)] {
            if !p.exists() {
                return Err(EngineError::Qwen(format!("{what} not found at {}", p.display())));
            }
        }
        let port = free_port()?;
        let mut cmd = Command::new(&cfg.server);
        cmd.arg("-m")
            .arg(&cfg.model)
            .arg("--mmproj")
            .arg(&cfg.mmproj)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .args(["-c", "8192", "-ngl", if cfg.gpu { "99" } else { "0" }, "--no-webui"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| EngineError::Qwen(format!("starting llama-server: {e}")))?;
        #[cfg(windows)]
        let job = match job::Job::kill_on_close(&child) {
            Ok(j) => j,
            Err(e) => {
                let _ = child.kill();
                return Err(EngineError::Qwen(format!("job object: {e}")));
            }
        };

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into();
        let base = format!("http://127.0.0.1:{port}");
        let started = Instant::now();
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(EngineError::Qwen(format!("llama-server exited while loading ({status})")));
            }
            if agent.get(&format!("{base}/health")).call().is_ok_and(|r| r.status() == 200) {
                break;
            }
            if started.elapsed() > START_TIMEOUT {
                let _ = child.kill();
                return Err(EngineError::Qwen("llama-server didn't load the model in time".into()));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        tracing::info!("qwen3-asr: llama-server ready on port {port} after {} ms", started.elapsed().as_millis());
        Ok(Self {
            child,
            base,
            agent,
            #[cfg(windows)]
            _job: job,
        })
    }

    /// Transcribe 16 kHz mono f32 samples. `language`: `Some("id")`, `Some("en")`, or `None` for
    /// Qwen to decide. `context`: the meeting hints (names, jargon), used as vocabulary.
    pub fn transcribe(
        &self,
        samples: &[f32],
        language: Option<&str>,
        context: Option<&str>,
    ) -> Result<Vec<Segment>, EngineError> {
        let audio = base64::engine::general_purpose::STANDARD.encode(wav_bytes(samples));
        let mut messages = vec![
            serde_json::json!({ "role": "system", "content": context.unwrap_or("") }),
            serde_json::json!({ "role": "user", "content": [
                { "type": "input_audio", "input_audio": { "data": audio, "format": "wav" } }
            ]}),
        ];
        if let Some(name) = language.and_then(language_name) {
            // Prefill the answer's start, as qwen-asr does to force a language.
            messages.push(serde_json::json!({ "role": "assistant", "content": format!("language {name}<asr_text>") }));
        }
        let body = serde_json::json!({ "messages": messages, "temperature": 0, "max_tokens": MAX_TOKENS });
        let mut res = self
            .agent
            .post(&format!("{}/v1/chat/completions", self.base))
            .send_json(&body)
            .map_err(|e| EngineError::Qwen(format!("request: {e}")))?;
        let status = res.status();
        let reply: serde_json::Value =
            res.body_mut().read_json().map_err(|e| EngineError::Qwen(format!("reply ({status}): {e}")))?;
        if status != 200 {
            return Err(EngineError::Qwen(format!("llama-server {status}: {reply}")));
        }
        let raw = reply["choices"][0]["message"]["content"].as_str().unwrap_or("");
        let text = parse_answer(raw);
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let end_ms = (samples.len() as i64 * 1000) / 16_000;
        // No confidence scores from this model: neutral values, so only the text checks apply.
        Ok(vec![Segment { start_ms: 0, end_ms, text, no_speech_prob: 0.0, avg_logprob: 0.0 }])
    }
}

impl Drop for QwenEngine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Qwen's name for a language code it is asked to force.
fn language_name(code: &str) -> Option<&'static str> {
    match code {
        "id" => Some("Indonesian"),
        "en" => Some("English"),
        _ => None,
    }
}

/// The transcript out of `language X<asr_text>text` (or just `text` when the start was prefilled).
pub fn parse_answer(raw: &str) -> String {
    let text = raw.split_once("<asr_text>").map_or(raw, |(_, t)| t);
    // A silent clip comes back as just "language None" or similar, with no text.
    if !raw.contains("<asr_text>") && raw.trim_start().starts_with("language ") && raw.split_whitespace().count() <= 2 {
        return String::new();
    }
    text.trim().to_string()
}

fn free_port() -> Result<u16, EngineError> {
    let l = TcpListener::bind("127.0.0.1:0").map_err(|e| EngineError::Qwen(format!("finding a free port: {e}")))?;
    l.local_addr().map(|a| a.port()).map_err(|e| EngineError::Qwen(e.to_string()))
}

/// 16-bit PCM WAV of 16 kHz mono samples.
fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    let mut w = Cursor::new(&mut out);
    use std::io::Write;
    let _ = w.write_all(b"RIFF");
    let _ = w.write_all(&(36 + data_len).to_le_bytes());
    let _ = w.write_all(b"WAVEfmt ");
    let _ = w.write_all(&16u32.to_le_bytes()); // fmt chunk size
    let _ = w.write_all(&1u16.to_le_bytes()); // PCM
    let _ = w.write_all(&1u16.to_le_bytes()); // mono
    let _ = w.write_all(&16_000u32.to_le_bytes());
    let _ = w.write_all(&32_000u32.to_le_bytes()); // byte rate
    let _ = w.write_all(&2u16.to_le_bytes()); // block align
    let _ = w.write_all(&16u16.to_le_bytes()); // bits per sample
    let _ = w.write_all(b"data");
    let _ = w.write_all(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        let _ = w.write_all(&v.to_le_bytes());
    }
    out
}

#[cfg(windows)]
mod job {
    //! A job object that kills its processes when its last handle closes (this app exits or
    //! crashes), so a llama-server can't be left running.
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub struct Job(HANDLE);

    // The handle is only closed in Drop; the job object itself is thread-safe.
    unsafe impl Send for Job {}

    impl Job {
        pub fn kill_on_close(child: &Child) -> Result<Self, String> {
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                let job = Job(h);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                if AssignProcessToJobObject(h, child.as_raw_handle() as HANDLE) == 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                Ok(job)
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_text_after_the_language_tag() {
        assert_eq!(parse_answer("language Indonesian<asr_text>Yang lain gimana?"), "Yang lain gimana?");
        assert_eq!(parse_answer("Oke, lanjut."), "Oke, lanjut."); // language was prefilled
        assert_eq!(parse_answer("language None<asr_text>"), "");
        assert_eq!(parse_answer("language None"), "");
    }

    #[test]
    fn writes_a_valid_wav_header() {
        let w = wav_bytes(&[0.0, 0.5, -0.5, 1.0]);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 8);
        assert_eq!(w.len(), 44 + 8);
    }
}
