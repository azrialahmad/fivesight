//! Transcribe a list of 16 kHz mono WAV clips the way the app does, for comparing speech models.
//!
//! Each clip is one utterance: the language is picked between Indonesian and English per clip
//! (sticky, like the app), then transcribed with the final-pass beam and the given prompt.
//! It leaves out the app's echo and hallucination filters, so it shows the raw model output.
//!
//! Usage: transcribe_chunks <model.bin> <list.txt> [prompt.txt] [--cpu]
//!   list.txt: one WAV path per line. Prints one JSON object per clip to stdout.

use std::path::Path;
use std::time::Instant;

use fs_stt::engine::Engine;
use fs_stt::lang::{LangTracker, MIN_DETECT_SECS};

const BEAM: i32 = 3;

fn read_wav(path: &str) -> Result<Vec<f32>, String> {
    let mut r = hound::WavReader::open(path).map_err(|e| format!("{path}: {e}"))?;
    let spec = r.spec();
    if spec.sample_rate != 16_000 || spec.channels != 1 {
        return Err(format!("{path}: needs 16 kHz mono, got {} Hz x{}", spec.sample_rate, spec.channels));
    }
    match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>().map_err(|e| e.to_string()),
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.map(|v| v as f32 / scale)).collect::<Result<_, _>>().map_err(|e| e.to_string())
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cpu = args.iter().any(|a| a == "--cpu");
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if pos.len() < 2 {
        eprintln!("usage: transcribe_chunks <model.bin> <list.txt> [prompt.txt] [--cpu]");
        std::process::exit(2);
    }
    let prompt = pos.get(2).map(|p| std::fs::read_to_string(p).expect("reading prompt").trim().to_string());
    let prompt = prompt.filter(|p| !p.is_empty());
    let list = std::fs::read_to_string(pos[1]).expect("reading list");

    let t = Instant::now();
    let engine = Engine::load(Path::new(pos[0]), !cpu).expect("loading model");
    eprintln!("whisper: model loaded in {} ms", t.elapsed().as_millis());

    let mut tracker = LangTracker::default();
    for path in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let samples = match read_wav(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                continue;
            }
        };
        let secs = samples.len() as f32 / 16_000.0;
        let t = Instant::now();
        let probs = if secs >= MIN_DETECT_SECS {
            engine.detect_among(&samples, &["id", "en"]).unwrap_or_else(|_| vec![0.0, 0.0])
        } else {
            vec![0.0, 0.0]
        };
        let lang = tracker.choose(probs[0], probs[1], secs);
        let text = match engine.transcribe(&samples, Some(lang), BEAM, prompt.as_deref()) {
            Ok(segs) => segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" "),
            Err(e) => {
                eprintln!("{path}: {e}");
                String::new()
            }
        };
        let line = serde_json::json!({
            "file": path,
            "lang": lang,
            "text": text.trim(),
            "audio_ms": (secs * 1000.0) as u64,
            "ms": t.elapsed().as_millis() as u64,
        });
        println!("{line}");
    }
}
