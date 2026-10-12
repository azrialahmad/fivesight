//! whisper.cpp engine wrapper.

use std::path::Path;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("whisper: {0}")]
    Whisper(#[from] whisper_rs::WhisperError),
    #[error("model path is not valid UTF-8")]
    BadPath,
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
        let is_special = tok
            .to_str_lossy()
            .map(|t| t.starts_with("[_") || t.starts_with("<|"))
            .unwrap_or(true);
        if !is_special {
            sum += tok.token_data().plog;
            n += 1;
        }
    }
    if n == 0 { 0.0 } else { sum / n as f32 }
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
            .map(|l| {
                whisper_rs::get_lang_id(l)
                    .and_then(|i| probs.get(i as usize).copied())
                    .unwrap_or(0.0)
            })
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
