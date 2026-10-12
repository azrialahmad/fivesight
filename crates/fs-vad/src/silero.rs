//! Silero VAD (v5-style ONNX) via ONNX Runtime.
//!
//! Model I/O: `input` f32 [1, 64 + 512] (previous 64 samples as context, then the frame),
//! `state` f32 [2, 1, 128], `sr` i64 scalar. Outputs `output` [1, 1] and `stateN`.

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;

use crate::{FRAME, SAMPLE_RATE};

const CONTEXT: usize = 64;
const STATE_LEN: usize = 2 * 128;

#[derive(Debug, thiserror::Error)]
pub enum SileroError {
    #[error("onnx runtime: {0}")]
    Ort(String),
    #[error("frame must be {FRAME} samples, got {0}")]
    BadFrame(usize),
}

fn oerr<E: std::fmt::Display>(e: E) -> SileroError {
    SileroError::Ort(e.to_string())
}

pub struct SileroVad {
    session: Session,
    state: Vec<f32>,
    context: [f32; CONTEXT],
}

impl SileroVad {
    pub fn load(model: &Path) -> Result<Self, SileroError> {
        let session = Session::builder()
            .map_err(oerr)?
            .with_intra_threads(1)
            .map_err(oerr)?
            .commit_from_file(model)
            .map_err(oerr)?;
        Ok(Self { session, state: vec![0.0; STATE_LEN], context: [0.0; CONTEXT] })
    }

    /// Names and types of the model's inputs and outputs, for debugging.
    pub fn describe(&self) -> String {
        let fmt = |o: &ort::value::Outlet| format!("{}: {:?}", o.name(), o.dtype());
        format!(
            "inputs: [{}]\noutputs: [{}]",
            self.session.inputs().iter().map(fmt).collect::<Vec<_>>().join(", "),
            self.session.outputs().iter().map(fmt).collect::<Vec<_>>().join(", ")
        )
    }

    /// Clear recurrent state. Call between unrelated streams.
    pub fn reset(&mut self) {
        self.state.fill(0.0);
        self.context = [0.0; CONTEXT];
    }

    /// Speech probability for one [`FRAME`]-sample frame at 16 kHz.
    pub fn prob(&mut self, frame: &[f32]) -> Result<f32, SileroError> {
        if frame.len() != FRAME {
            return Err(SileroError::BadFrame(frame.len()));
        }
        let mut input = Vec::with_capacity(CONTEXT + FRAME);
        input.extend_from_slice(&self.context);
        input.extend_from_slice(frame);

        let outputs = self
            .session
            .run(ort::inputs! {
                "input" => Tensor::from_array(([1usize, CONTEXT + FRAME], input)).map_err(oerr)?,
                "state" => Tensor::from_array(([2usize, 1, 128], self.state.clone())).map_err(oerr)?,
                "sr" => Tensor::from_array(((), vec![SAMPLE_RATE as i64])).map_err(oerr)?,
            })
            .map_err(oerr)?;

        let (_, p) = outputs["output"].try_extract_tensor::<f32>().map_err(oerr)?;
        let prob = p.first().copied().unwrap_or(0.0);
        let (_, st) = outputs["stateN"].try_extract_tensor::<f32>().map_err(oerr)?;
        self.state.copy_from_slice(st);
        self.context.copy_from_slice(&frame[FRAME - CONTEXT..]);
        Ok(prob)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Needs models/silero_vad.onnx and testdata/synthetic_en.wav (see scripts/).
    #[test]
    fn separates_speech_from_silence_on_synthetic_meeting() {
        let model = root().join("models/silero_vad.onnx");
        let wav = root().join("testdata/synthetic_en.wav");
        if !model.exists() || !wav.exists() {
            eprintln!("skipping: model or test wav missing");
            return;
        }
        let mut vad = SileroVad::load(&model).unwrap();
        eprintln!("{}", vad.describe());

        let mut r = hound::WavReader::open(wav).unwrap();
        let samples: Vec<f32> = r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
        let probs: Vec<f32> = samples
            .chunks_exact(FRAME)
            .map(|f| vad.prob(f).unwrap())
            .collect();

        // The clip is speech for ~6 s then a 0.6 s gap, repeated. Check the extremes.
        let speech_frames = probs.iter().filter(|&&p| p > 0.5).count();
        let secs_speech = speech_frames as f32 * FRAME as f32 / SAMPLE_RATE as f32;
        eprintln!("speech {secs_speech:.1}s of {:.1}s", samples.len() as f32 / SAMPLE_RATE as f32);
        assert!((20.0..31.0).contains(&secs_speech), "speech seconds {secs_speech}");

        // Digital silence must score near zero.
        let mut vad2 = SileroVad::load(&model).unwrap();
        let quiet = vad2.prob(&vec![0.0; FRAME]).unwrap();
        assert!(quiet < 0.1, "silence prob {quiet}");
    }
}
