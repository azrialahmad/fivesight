use anyhow::{bail, Context, Result};

fn read_wav_16k_mono(path: &str) -> Result<Vec<f32>> {
    let mut r = hound::WavReader::open(path).with_context(|| format!("open {path}"))?;
    let spec = r.spec();
    if spec.sample_rate != 16_000 || spec.channels != 1 {
        bail!(
            "need 16 kHz mono wav, got {} Hz x {} ch",
            spec.sample_rate,
            spec.channels
        );
    }
    Ok(match spec.sample_format {
        hound::SampleFormat::Int => r
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / 32768.0))
            .collect::<Result<_, _>>()?,
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
    })
}

#[cfg(feature = "engine")]
fn bench(model: &str, wav: &str, lang: Option<&str>) -> Result<()> {
    use std::time::Instant;
    let samples = read_wav_16k_mono(wav)?;
    let audio_s = samples.len() as f32 / 16_000.0;

    let t = Instant::now();
    let eng = fs_stt::engine::Engine::load(std::path::Path::new(model), true)?;
    println!("model load: {:.2}s", t.elapsed().as_secs_f32());

    // Warm-up pass so the timed runs exclude one-time kernel setup.
    let warm = &samples[..samples.len().min(16_000 * 3)];
    eng.transcribe(warm, lang, 1, None)?;

    for beam in [1, 3] {
        let t = Instant::now();
        let segs = eng.transcribe(&samples, lang, beam, None)?;
        let dt = t.elapsed().as_secs_f32();
        println!(
            "\nbeam {beam}: {dt:.2}s for {audio_s:.1}s audio (RTF {:.3})",
            dt / audio_s
        );
        for s in segs {
            println!(
                "[{:>6}-{:>6} ms] (nsp {:.2}) {}",
                s.start_ms, s.end_ms, s.no_speech_prob, s.text
            );
        }
    }
    Ok(())
}

/// Capture both streams for `secs` seconds and print the level per second.
/// Levels only: no audio is saved.
fn capture(secs: u64) -> Result<()> {
    use fs_audio::{rms, Capture, Source};
    use std::sync::mpsc::sync_channel;
    use std::time::{Duration, Instant};

    let (mic_tx, mic_rx) = sync_channel(256);
    let (out_tx, out_rx) = sync_channel(256);
    let mic = Capture::start(Source::Mic, mic_tx)?;
    let out = Capture::start(Source::Loopback, out_tx)?;
    println!("mic      : {}\nloopback : {}", mic.device_name, out.device_name);
    println!("capturing {secs}s (talk, and play some audio) ...\n");

    let frames_per_sec = fs_audio::SAMPLE_RATE as usize / fs_audio::FRAME;
    let (mut mic_buf, mut out_buf) = (Vec::new(), Vec::new());
    let (mut mic_total, mut out_total) = (0usize, 0usize);
    let start = Instant::now();
    let mut next_report = 1;
    while start.elapsed() < Duration::from_secs(secs) {
        for f in mic_rx.try_iter() {
            mic_total += 1;
            mic_buf.extend(f);
        }
        for f in out_rx.try_iter() {
            out_total += 1;
            out_buf.extend(f);
        }
        if start.elapsed() >= Duration::from_secs(next_report) {
            println!("t={next_report:>2}s  mic rms {:.4}   loopback rms {:.4}", rms(&mic_buf), rms(&out_buf));
            mic_buf.clear();
            out_buf.clear();
            next_report += 1;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!(
        "\nframes: mic {mic_total} (expect ~{}), loopback {out_total} (expect ~{}); dropped: mic {}, loopback {}",
        secs as usize * frames_per_sec,
        secs as usize * frames_per_sec,
        mic.dropped_frames(),
        out.dropped_frames()
    );
    Ok(())
}

/// Live transcription of mic ("me") and system audio ("others") for `secs` seconds.
/// Nothing is recorded to disk; transcripts only print to the console.
#[cfg(feature = "engine")]
fn live(secs: u64, lang: Option<String>) -> Result<()> {
    use fs_pipeline::{Kind, Pipeline, PipelineConfig};
    use std::time::{Duration, Instant};

    let mut cfg = PipelineConfig::new("models/ggml-large-v3-turbo.bin".into(), "models/silero_vad.onnx".into());
    cfg.language = fs_stt::lang::LangMode::parse(lang.as_deref().unwrap_or("auto"));
    let (tx, rx) = std::sync::mpsc::channel();
    println!("loading models ...");
    let pipeline = Pipeline::start(cfg, tx)?;
    println!("listening for {secs}s. '..' = partial, '>>' = final, 'xx' = filtered out\n");

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) else { continue };
        let who = match ev.source {
            fs_audio::Source::Mic => "me    ",
            fs_audio::Source::Loopback => "others",
        };
        let t = format!("{:>5.1}s", ev.start_ms as f32 / 1000.0);
        match (&ev.dropped, ev.kind) {
            (Some(why), _) => println!("xx {who} {t} #{} [{why}] {}", ev.segment_id, ev.text),
            (None, Kind::Partial) => println!(".. {who} {t} #{} {}", ev.segment_id, ev.text),
            (None, Kind::Final) => println!(">> {who} {t} #{} {}", ev.segment_id, ev.text),
        }
    }
    println!("\ndropped frames: {}", pipeline.dropped_frames());
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        #[cfg(feature = "engine")]
        Some("live") => live(
            args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30),
            args.get(3).filter(|l| l.as_str() != "auto").cloned(),
        ),
        Some("capture") => capture(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(6)),
        #[cfg(feature = "engine")]
        Some("bench") => {
            let (Some(model), Some(wav)) = (args.get(2), args.get(3)) else {
                bail!("usage: fivesight-cli bench <ggml-model> <wav16k-mono> [lang]");
            };
            bench(model, wav, args.get(4).map(String::as_str))
        }
        #[cfg(not(feature = "engine"))]
        Some("bench") => bail!("built without the engine feature; rebuild with --features cuda"),
        _ => {
            println!("fivesight-cli {}\ncommands: bench", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}
