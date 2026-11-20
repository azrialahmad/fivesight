//! Replay a recorded meeting through the real pipeline (Silero endpointing, energy gates,
//! hallucination filters, echo guard), as if it were live, and print every final line.
//!
//! Usage:
//!   replay --model <whisper.bin> --vad <silero_vad.onnx> --mic <me.wav> --loopback <others.wav>
//!          [--prompt-file <prompt.txt>] [--from <sec>] [--to <sec>] [--speed <x>]
//!   With Qwen3-ASR instead of Whisper, replace --model with
//!          --qwen-server <llama-server.exe> --qwen-model <model.gguf> --qwen-mmproj <mmproj.gguf>
//!
//! Recordings must be 16 kHz mono (what bench/stt-compare's `record` writes). Prints one JSON
//! object per final line to stdout: source, start in the recording (s), text, and why it was
//! dropped if it was.

use std::path::PathBuf;
use std::sync::mpsc::channel;

use fs_pipeline::{AudioInput, Kind, Pipeline, PipelineConfig};

fn read_wav(path: &str) -> Vec<f32> {
    let mut r = hound::WavReader::open(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let spec = r.spec();
    assert!(spec.sample_rate == 16_000 && spec.channels == 1, "{path}: needs 16 kHz mono");
    match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().map(Result::unwrap).collect(),
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.unwrap() as f32 / scale).collect()
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let need = |name: &str| get(name).unwrap_or_else(|| panic!("missing {name}"));
    let from_s: f32 = get("--from").map_or(0.0, |v| v.parse().unwrap());
    let to_s: Option<f32> = get("--to").map(|v| v.parse().unwrap());
    let speed: f32 = get("--speed").map_or(1.0, |v| v.parse().unwrap());

    let cut = |mut s: Vec<f32>| {
        let end = to_s.map_or(s.len(), |t| ((t * 16_000.0) as usize).min(s.len()));
        let start = ((from_s * 16_000.0) as usize).min(end);
        s.truncate(end);
        s.drain(..start);
        s
    };
    let mic = cut(read_wav(&need("--mic")));
    let loopback = cut(read_wav(&need("--loopback")));
    eprintln!("replaying {:.1} min at {speed}x", mic.len().max(loopback.len()) as f32 / 16_000.0 / 60.0);

    let mut cfg = PipelineConfig::new(PathBuf::from(get("--model").unwrap_or_default()), PathBuf::from(need("--vad")));
    // Qwen3-ASR instead of Whisper: --qwen-server <llama-server.exe> --qwen-model <gguf> --qwen-mmproj <gguf>
    if let Some(server) = get("--qwen-server") {
        cfg.qwen = Some(fs_pipeline::QwenConfig {
            server: server.into(),
            model: need("--qwen-model").into(),
            mmproj: need("--qwen-mmproj").into(),
            gpu: true,
        });
    }
    if let Some(p) = get("--prompt-file") {
        let text = std::fs::read_to_string(p).expect("reading prompt").trim().to_string();
        cfg.initial_prompt = (!text.is_empty()).then_some(text);
    }

    let (tx, rx) = channel();
    let pipeline =
        Pipeline::start_with(cfg, tx, AudioInput::Recorded { mic, loopback, speed }).expect("starting pipeline");
    for ev in rx {
        if ev.kind != Kind::Final {
            continue;
        }
        let line = serde_json::json!({
            "source": ev.source,
            "start_s": from_s + ev.start_ms as f32 / 1000.0,
            "duration_s": ev.duration_ms as f32 / 1000.0,
            "text": ev.text,
            "dropped": ev.dropped,
        });
        println!("{line}");
    }
    drop(pipeline);
}
