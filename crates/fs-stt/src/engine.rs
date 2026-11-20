//! whisper.cpp engine wrapper.

use std::path::Path;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("whisper: {0}")]
    Whisper(#[from] whisper_rs::WhisperError),
    #[error("model path is not valid UTF-8")]
    BadPath,
    #[error("qwen3-asr: {0}")]
    Qwen(String),
}

#[derive(Debug, Clone)]
pub struct Segment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub no_speech_prob: f32,
    /// Mean log-probability of the text tokens (special tokens excluded).
    pub avg_logprob: f32,
}

fn avg_logprob(seg: &whisper_rs::WhisperSegment) -> f32 {
    let (mut sum, mut n) = (0.0f32, 0u32);
    for i in 0..seg.n_tokens() {
        let Some(tok) = seg.get_token(i) else { continue };
        let is_special = tok.to_str_lossy().map(|t| t.starts_with("[_") || t.starts_with("<|")).unwrap_or(true);
        if !is_special {
            sum += tok.token_data().plog;
            n += 1;
        }
    }
    if n == 0 {
        0.0
    } else {
        sum / n as f32
    }
}

pub struct Engine {
    ctx: WhisperContext,
}

impl Engine {
    pub fn load(model: &Path, use_gpu: bool) -> Result<Self, EngineError> {
        let mut p = WhisperContextParameters::default();
        p.use_gpu(use_gpu);
        let ctx = WhisperContext::new_with_params(model.to_str().ok_or(EngineError::BadPath)?, p)?;
        Ok(Self { ctx })
    }

    /// Probability Whisper assigns to each of `langs` (e.g. `["id", "en"]`) for this audio.
    /// Unknown language codes score 0.
    pub fn detect_among(&self, samples: &[f32], langs: &[&str]) -> Result<Vec<f32>, EngineError> {
        let mut state = self.ctx.create_state()?;
        state.pcm_to_mel(samples, 4)?;
        let (_, probs) = state.lang_detect(0, 4)?;
        Ok(langs
            .iter()
            .map(|l| whisper_rs::get_lang_id(l).and_then(|i| probs.get(i as usize).copied()).unwrap_or(0.0))
            .collect())
    }

    /// Transcribe 16 kHz mono f32 samples. `language`: `Some("id")`, `Some("en")`, or `None` to auto-detect.
    pub fn transcribe(
        &self,
        samples: &[f32],
        language: Option<&str>,
        beam_size: i32,
        initial_prompt: Option<&str>,
    ) -> Result<Vec<Segment>, EngineError> {
        let mut state = self.ctx.create_state()?;
        let strategy = if beam_size > 1 {
            SamplingStrategy::BeamSearch { beam_size, patience: -1.0 }
        } else {
            SamplingStrategy::Greedy { best_of: 1 }
        };
        let mut params = FullParams::new(strategy);
        params.set_language(language);
        params.set_translate(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        params.set_no_context(true);
        if let Some(p) = initial_prompt {
            params.set_initial_prompt(p);
        }
        state.full(params, samples)?;

        let mut out = Vec::new();
        for seg in state.as_iter() {
            out.push(Segment {
                start_ms: seg.start_timestamp() * 10,
                end_ms: seg.end_timestamp() * 10,
                text: seg.to_str_lossy()?.trim().to_string(),
                no_speech_prob: seg.no_speech_probability(),
                avg_logprob: avg_logprob(&seg),
            });
        }
        Ok(out)
    }
}

/// Anything that turns audio into segments, for code that works with either engine.
pub trait Transcribe {
    fn transcribe(
        &self,
        samples: &[f32],
        language: Option<&str>,
        beam_size: i32,
        initial_prompt: Option<&str>,
    ) -> Result<Vec<Segment>, EngineError>;
}

impl Transcribe for Engine {
    fn transcribe(
        &self,
        samples: &[f32],
        language: Option<&str>,
        beam: i32,
        prompt: Option<&str>,
    ) -> Result<Vec<Segment>, EngineError> {
        Engine::transcribe(self, samples, language, beam, prompt)
    }
}

/// Which speech model to run.
#[derive(Debug, Clone)]
pub enum EngineSpec {
    /// whisper.cpp with this model file.
    Whisper(std::path::PathBuf),
    /// Qwen3-ASR through llama-server.
    Qwen(crate::qwen::QwenConfig),
}

/// The loaded speech model.
pub enum SpeechEngine {
    Whisper(Engine),
    Qwen(crate::qwen::QwenEngine),
}

impl SpeechEngine {
    pub fn load(spec: &EngineSpec, use_gpu: bool) -> Result<Self, EngineError> {
        Ok(match spec {
            EngineSpec::Whisper(model) => Self::Whisper(Engine::load(model, use_gpu)?),
            EngineSpec::Qwen(cfg) => Self::Qwen(crate::qwen::QwenEngine::start(cfg)?),
        })
    }

    /// Qwen picks the language itself; Whisper needs it chosen first (see [`Engine::detect_among`]).
    pub fn picks_language(&self) -> bool {
        matches!(self, Self::Qwen(_))
    }

    /// Whisper's language probabilities; Qwen has none (see [`SpeechEngine::picks_language`]).
    pub fn detect_among(&self, samples: &[f32], langs: &[&str]) -> Result<Vec<f32>, EngineError> {
        match self {
            Self::Whisper(e) => e.detect_among(samples, langs),
            Self::Qwen(_) => Ok(vec![0.0; langs.len()]),
        }
    }
}

impl Transcribe for SpeechEngine {
    /// For Qwen, `beam_size` is ignored and `initial_prompt` is used as context (vocabulary).
    fn transcribe(
        &self,
        samples: &[f32],
        language: Option<&str>,
        beam: i32,
        prompt: Option<&str>,
    ) -> Result<Vec<Segment>, EngineError> {
        match self {
            Self::Whisper(e) => e.transcribe(samples, language, beam, prompt),
            Self::Qwen(q) => q.transcribe(samples, language, prompt),
        }
    }
}
