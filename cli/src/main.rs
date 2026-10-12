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

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
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
