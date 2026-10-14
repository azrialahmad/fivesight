//! The speech models fivesight can run, which one suits this PC, where they are on disk, and
//! downloading them (resumable, checked against a pinned SHA-256).

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::system::SystemInfo;

/// Hugging Face commit the Whisper files are pinned to, so a file can't change under its checksum.
const WHISPER_REV: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

/// A file fivesight can download.
#[derive(Debug, Clone, Copy)]
pub struct ModelFile {
    pub file: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

impl ModelFile {
    pub fn url(&self) -> String {
        if self.file == VAD.file {
            "https://github.com/snakers4/silero-vad/raw/v6.2/src/silero_vad/data/silero_vad.onnx".into()
        } else {
            format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/{WHISPER_REV}/{}", self.file)
        }
    }
}

/// Silero VAD v6.2: finds where speech starts and stops. Always needed.
pub const VAD: ModelFile = ModelFile {
    file: "silero_vad.onnx",
    bytes: 2_327_524,
    sha256: "1a153a22f4509e292a94e67d6f9b85e8deb25b4988682b7e174c65279d8788e3",
};

#[derive(Debug, Clone, Copy)]
pub struct SpeechModel {
    pub id: &'static str,
    pub label: &'static str,
    pub note: &'static str,
    /// Graphics memory it needs while transcribing, roughly (weights plus working buffers).
    pub vram_mb: u64,
    pub file: ModelFile,
}

/// Best first. All are multilingual; the large-v3 family handles Indonesian/English mixing best.
pub const SPEECH_MODELS: &[SpeechModel] = &[
    SpeechModel {
        id: "large-v3-turbo",
        label: "Large v3 Turbo",
        note: "Best balance of accuracy and speed for live meetings.",
        vram_mb: 2_300,
        file: ModelFile {
            file: "ggml-large-v3-turbo.bin",
            bytes: 1_624_555_275,
            sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        },
    },
    SpeechModel {
        id: "large-v3-turbo-q8_0",
        label: "Large v3 Turbo (8-bit)",
        note: "Almost the same accuracy in about half the memory.",
        vram_mb: 1_500,
        file: ModelFile {
            file: "ggml-large-v3-turbo-q8_0.bin",
            bytes: 874_188_075,
            sha256: "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
        },
    },
    SpeechModel {
        id: "large-v3-turbo-q5_0",
        label: "Large v3 Turbo (5-bit)",
        note: "For cards with little memory. Slightly less accurate.",
        vram_mb: 1_100,
        file: ModelFile {
            file: "ggml-large-v3-turbo-q5_0.bin",
            bytes: 574_041_195,
            sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        },
    },
    SpeechModel {
        id: "large-v3",
        label: "Large v3",
        note: "Most accurate, but several times slower. Needs a strong card to keep up live.",
        vram_mb: 4_200,
        file: ModelFile {
            file: "ggml-large-v3.bin",
            bytes: 3_095_033_483,
            sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
        },
    },
    SpeechModel {
        id: "small-q5_1",
        label: "Small (5-bit)",
        note: "Fastest and smallest. Noticeably weaker, especially for Indonesian.",
        vram_mb: 600,
        file: ModelFile {
            file: "ggml-small-q5_1.bin",
            bytes: 190_085_487,
            sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
        },
    },
];

pub const DEFAULT_SPEECH_MODEL: &str = "large-v3-turbo";

pub fn speech_model(id: &str) -> Option<&'static SpeechModel> {
    SPEECH_MODELS.iter().find(|m| m.id == id)
}

/// Where downloads go.
pub fn download_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("fivesight").join("models"))
}

/// Folders searched for model files, in order.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(p) = std::env::var_os("FIVESIGHT_MODELS") {
        dirs.push(PathBuf::from(p));
    }
    dirs.extend(download_dir());
    dirs.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../models")));
    dirs
}

/// A complete copy of `f` (right size) in any search folder.
pub fn find(f: &ModelFile) -> Option<PathBuf> {
    search_dirs()
        .into_iter()
        .map(|d| d.join(f.file))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() == f.bytes))
}

// ---------- recommendation ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    /// False when this PC can't run speech recognition with this build (no NVIDIA card).
    pub supported: bool,
    pub speech_model: Option<&'static str>,
    /// Why, in a sentence.
    pub summary: String,
    pub warnings: Vec<String>,
    /// What size of local AI model fits next to speech recognition, for the AI model step.
    pub local_llm_hint: String,
}

pub fn recommend(info: &SystemInfo) -> Recommendation {
    let mut warnings = Vec::new();
    let Some(gpu) = info.best_nvidia() else {
        let what = match info.gpus.first() {
            Some(g) => format!("This PC has {}, ", g.name),
            None => "No graphics card was found, ".into(),
        };
        return Recommendation {
            supported: false,
            speech_model: None,
            summary: format!(
                "{what}but this version of fivesight transcribes on an NVIDIA graphics card (CUDA). \
                 Support for other PCs is planned."
            ),
            warnings,
            local_llm_hint: "Use a hosted AI model.".into(),
        };
    };
    let v = gpu.vram_mb;
    let pick = match v {
        3_500.. => "large-v3-turbo",
        2_500.. => "large-v3-turbo-q8_0",
        1_800.. => "large-v3-turbo-q5_0",
        _ => "small-q5_1",
    };
    let model = speech_model(pick).expect("catalog has the pick");
    let gb = |mb: u64| format!("{:.0} GB", mb as f64 / 1024.0);
    let summary = format!("{} with {} of graphics memory: {} fits comfortably.", gpu.name, gb(v), model.label);

    if info.ram_mb > 0 && info.ram_mb < 8 * 1024 {
        warnings.push(format!("{} of memory is on the low side; close other apps during meetings.", gb(info.ram_mb)));
    }
    if let Some(free) = info.free_disk_mb {
        let need = (model.file.bytes + VAD.bytes) / (1024 * 1024) + 500;
        if free < need {
            warnings.push(format!("Only {} free on the model drive; the download needs about {}.", gb(free), gb(need)));
        }
    }

    // What's left once Whisper is loaded, with some headroom for Windows and the browser.
    let left = v.saturating_sub(model.vram_mb + 1_000);
    let local_llm_hint = match left {
        9_000.. => "A local model up to about 12B (e.g. gemma3:12b) fits next to speech recognition.",
        5_500.. => "A local 7-8B model (e.g. qwen3:8b or llama3.1:8b) fits next to speech recognition.",
        3_000.. => "A small local model (3-4B, e.g. gemma3:4b) fits; a hosted model will write better notes.",
        _ => "Not enough graphics memory left for a local model during meetings; use a hosted one.",
    }
    .into();

    Recommendation { supported: true, speech_model: Some(pick), summary, warnings, local_llm_hint }
}

// ---------- download ----------

/// Download `f` into `dir`, resuming a partial `.part` file, and check size and SHA-256 before
/// putting it in place. `progress(received, total)` is called as data arrives.
pub async fn download(
    client: &reqwest::Client,
    f: &ModelFile,
    dir: &Path,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    download_from(client, &f.url(), f, dir, cancel, progress).await
}

async fn download_from(
    client: &reqwest::Client,
    url: &str,
    f: &ModelFile,
    dir: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let dest = dir.join(f.file);
    let part = dir.join(format!("{}.part", f.file));
    let io = |e: std::io::Error| format!("Couldn't write {}: {e}", part.display());

    let mut out =
        std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&part).map_err(io)?;
    let mut have = out.metadata().map_err(io)?.len();
    if have > f.bytes {
        have = 0;
    }
    let mut req = client.get(url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let resp = req.send().await.map_err(|e| format!("Couldn't reach the download server: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("The download server answered {status} for {}.", f.file));
    }
    // 206: the server continues where the partial file ends. Anything else: start over.
    let mut hasher = Sha256::new();
    if status == reqwest::StatusCode::PARTIAL_CONTENT && have > 0 {
        out.seek(SeekFrom::Start(0)).map_err(io)?;
        let mut buf = vec![0u8; 1 << 20];
        let mut left = have;
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            let n = out.read(&mut buf[..want]).map_err(io)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            left -= n as u64;
        }
        out.seek(SeekFrom::Start(have)).map_err(io)?;
    } else {
        have = 0;
        out.set_len(0).map_err(io)?;
        out.seek(SeekFrom::Start(0)).map_err(io)?;
    }

    let mut received = have;
    let mut last = Instant::now();
    progress(received, f.bytes);
    let mut body = resp.bytes_stream();
    while let Some(chunk) = body.next().await {
        if cancel.load(Ordering::Relaxed) {
            return Err("Download cancelled.".into());
        }
        let chunk = chunk.map_err(|e| format!("The download was interrupted: {e}. Try again to resume."))?;
        out.write_all(&chunk).map_err(io)?;
        hasher.update(&chunk);
        received += chunk.len() as u64;
        if last.elapsed() >= Duration::from_millis(150) {
            progress(received, f.bytes);
            last = Instant::now();
        }
    }
    out.flush().map_err(io)?;
    drop(out);
    progress(received, f.bytes);

    let digest: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if received != f.bytes || digest != f.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!("{} didn't download correctly (size or checksum mismatch). Try again.", f.file));
    }
    let _ = std::fs::remove_file(&dest);
    std::fs::rename(&part, &dest).map_err(|e| format!("Couldn't move {} into place: {e}", f.file))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::Gpu;

    fn pc(gpus: &[(&str, u64, bool)], ram_gb: u64) -> SystemInfo {
        SystemInfo {
            cpu: "Test CPU".into(),
            cores: 8,
            ram_mb: ram_gb * 1024,
            gpus: gpus.iter().map(|&(n, v, nv)| Gpu { name: n.into(), vram_mb: v, nvidia: nv }).collect(),
            free_disk_mb: Some(100_000),
        }
    }

    #[test]
    fn picks_a_model_by_graphics_memory() {
        let r = recommend(&pc(&[("NVIDIA GeForce RTX 5070", 11_944, true)], 32));
        assert_eq!(r.speech_model, Some("large-v3-turbo"));
        assert!(r.supported && r.warnings.is_empty());
        // 12 GB minus Whisper and headroom leaves ~8.6 GB: room for 7-8B, not quite 12B.
        assert!(r.local_llm_hint.contains("7-8B"), "{}", r.local_llm_hint);
        assert!(recommend(&pc(&[("RTX 4090", 24_564, true)], 64)).local_llm_hint.contains("12B"));

        assert_eq!(recommend(&pc(&[("RTX 3050 Laptop", 4_096, true)], 16)).speech_model, Some("large-v3-turbo"));
        assert_eq!(recommend(&pc(&[("GTX 1050", 3_072, true)], 16)).speech_model, Some("large-v3-turbo-q8_0"));
        assert_eq!(recommend(&pc(&[("MX450", 2_048, true)], 8)).speech_model, Some("large-v3-turbo-q5_0"));
        assert_eq!(recommend(&pc(&[("MX150", 1_024, true)], 8)).speech_model, Some("small-q5_1"));
    }

    #[test]
    fn uses_the_nvidia_card_when_there_are_several() {
        let r = recommend(&pc(&[("Intel UHD Graphics", 128, false), ("NVIDIA RTX 4060 Laptop", 8_188, true)], 16));
        assert_eq!(r.speech_model, Some("large-v3-turbo"));
        assert!(r.summary.contains("RTX 4060"));
    }

    #[test]
    fn says_so_without_an_nvidia_card() {
        let r = recommend(&pc(&[("AMD Radeon RX 7800 XT", 16_368, false)], 32));
        assert!(!r.supported);
        assert_eq!(r.speech_model, None);
        assert!(r.summary.contains("AMD Radeon") && r.summary.contains("NVIDIA"));
    }

    #[test]
    fn warns_about_low_memory_and_disk() {
        let mut p = pc(&[("RTX 3060", 12_288, true)], 4);
        p.free_disk_mb = Some(900);
        let r = recommend(&p);
        assert_eq!(r.warnings.len(), 2, "{:?}", r.warnings);
    }

    /// Serve `body` over HTTP on localhost, honouring `Range: bytes=N-`, for `conns` requests.
    fn serve(body: Vec<u8>, conns: usize) -> String {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.bin", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().take(conns) {
                let mut stream = stream.unwrap();
                let mut from = 0usize;
                for line in BufReader::new(stream.try_clone().unwrap()).lines() {
                    let line = line.unwrap();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(r) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        from = r.trim_end_matches('-').parse().unwrap();
                    }
                }
                let status = if from > 0 { "206 Partial Content" } else { "200 OK" };
                let part = &body[from..];
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", part.len());
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(part).unwrap();
            }
        });
        url
    }

    fn file_for(body: &[u8]) -> ModelFile {
        let sha: String = Sha256::digest(body).iter().map(|b| format!("{b:02x}")).collect();
        ModelFile { file: "model.bin", bytes: body.len() as u64, sha256: Box::leak(sha.into_boxed_str()) }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fivesight-dl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn downloads_resumes_and_checks_the_file() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let f = file_for(&body);
        let client = reqwest::Client::new();
        let no = AtomicBool::new(false);

        // Fresh download.
        let dir = temp_dir("fresh");
        let url = serve(body.clone(), 1);
        let mut last = (0, 0);
        let got = tauri::async_runtime::block_on(download_from(&client, &url, &f, &dir, &no, |r, t| last = (r, t)));
        assert_eq!(std::fs::read(got.unwrap()).unwrap(), body);
        assert_eq!(last, (body.len() as u64, body.len() as u64));
        assert!(!dir.join("model.bin.part").exists());

        // Resume: the first 100 kB are already there, the server sends the rest.
        let dir = temp_dir("resume");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.bin.part"), &body[..100_000]).unwrap();
        let url = serve(body.clone(), 1);
        let got = tauri::async_runtime::block_on(download_from(&client, &url, &f, &dir, &no, |_, _| {}));
        assert_eq!(std::fs::read(got.unwrap()).unwrap(), body);
    }

    #[test]
    fn rejects_a_file_with_the_wrong_checksum() {
        let body = vec![1u8; 50_000];
        let mut f = file_for(&body);
        f.sha256 = "0000000000000000000000000000000000000000000000000000000000000000";
        let dir = temp_dir("bad");
        let url = serve(body, 1);
        let r = tauri::async_runtime::block_on(download_from(
            &reqwest::Client::new(),
            &url,
            &f,
            &dir,
            &AtomicBool::new(false),
            |_, _| {},
        ));
        assert!(r.unwrap_err().contains("checksum"));
        assert!(!dir.join("model.bin").exists() && !dir.join("model.bin.part").exists());
    }

    #[test]
    fn catalog_is_consistent() {
        assert!(speech_model(DEFAULT_SPEECH_MODEL).is_some());
        for m in SPEECH_MODELS {
            assert_eq!(m.file.sha256.len(), 64, "{}", m.id);
            assert!(m.file.url().ends_with(m.file.file));
        }
        assert!(VAD.url().contains("/v6.2/"));
    }
}
