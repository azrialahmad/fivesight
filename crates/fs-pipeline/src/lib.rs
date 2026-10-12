//! Live transcription pipeline.
//!
//! ```text
//! mic loopback ─► [stream thread: Silero ─► Endpointer] ─┐
//!                                                         ├─► jobs ─► [whisper worker] ─► filter ─► events
//! loopback ────► [stream thread: Silero ─► Endpointer] ─┘
//! ```
//!
//! Partials are coalesced: a stream never has more than one partial job in flight, so a
//! slow GPU cannot build a backlog. Finals are always queued.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fs_audio::{rms, Capture, Source};
use fs_stt::engine::Engine;
use fs_stt::filter::{judge, FilterConfig, SegmentStats, Verdict};
use fs_vad::silero::SileroVad;
use fs_vad::{EndpointConfig, Endpointer, Event, FRAME, SAMPLE_RATE};

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub whisper_model: PathBuf,
    pub vad_model: PathBuf,
    /// `Some("id")`, `Some("en")`, or `None` for auto-detect.
    pub language: Option<String>,
    pub partial_interval: Duration,
    pub partial_window_s: f32,
    pub me_end_silence_ms: usize,
    pub others_end_silence_ms: usize,
    /// Segments quieter than this RMS are dropped. Runs regardless of VAD.
    pub min_rms_me: f32,
    pub min_rms_others: f32,
    /// Finalize the other side's segment when the local user starts talking.
    pub barge_in: bool,
    pub final_beam: i32,
    pub filter: FilterConfig,
}

impl PipelineConfig {
    pub fn new(whisper_model: PathBuf, vad_model: PathBuf) -> Self {
        Self {
            whisper_model,
            vad_model,
            language: None,
            partial_interval: Duration::from_millis(700),
            partial_window_s: 15.0,
            me_end_silence_ms: 800,
            others_end_silence_ms: 1100,
            min_rms_me: 0.01,
            min_rms_others: 0.002,
            barge_in: true,
            final_beam: 3,
            filter: FilterConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Partial,
    Final,
}

#[derive(Debug, Clone)]
pub struct TranscriptEvent {
    pub source: Source,
    pub kind: Kind,
    /// Partials and the final of one utterance share an id.
    pub segment_id: u64,
    pub start_ms: u64,
    pub text: String,
    /// Set when a final was discarded (and `text` is what was discarded).
    pub dropped: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("audio: {0}")]
    Audio(#[from] fs_audio::AudioError),
    #[error("whisper: {0}")]
    Whisper(String),
    #[error("vad: {0}")]
    Vad(String),
    #[error("thread: {0}")]
    Thread(String),
}

struct Job {
    source: Source,
    segment_id: u64,
    kind: Kind,
    audio: Vec<f32>,
    start_sample: u64,
}

pub struct Pipeline {
    captures: Vec<Capture>,
    threads: Vec<JoinHandle<()>>,
}

impl Pipeline {
    /// Load models, open both audio streams and start transcribing.
    /// Events arrive on `events`; dropping the `Pipeline` stops everything.
    pub fn start(cfg: PipelineConfig, events: Sender<TranscriptEvent>) -> Result<Self, PipelineError> {
        let (job_tx, job_rx) = sync_channel::<Job>(16);
        let inflight_mic = Arc::new(AtomicBool::new(false));
        let inflight_out = Arc::new(AtomicBool::new(false));
        let barge = Arc::new(AtomicBool::new(false));

        // Whisper worker owns the engine; load it here and report errors synchronously.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let worker = {
            let cfg = cfg.clone();
            let events = events.clone();
            let (im, io) = (inflight_mic.clone(), inflight_out.clone());
            thread::Builder::new()
                .name("whisper-worker".into())
                .spawn(move || match Engine::load(&cfg.whisper_model, true) {
                    Ok(engine) => {
                        let _ = ready_tx.send(Ok(()));
                        worker_loop(engine, cfg, job_rx, events, im, io);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                    }
                })
                .map_err(|e| PipelineError::Thread(e.to_string()))?
        };
        ready_rx
            .recv()
            .map_err(|e| PipelineError::Thread(e.to_string()))?
            .map_err(PipelineError::Whisper)?;

        let mut threads = vec![worker];
        let mut captures = Vec::new();
        for (source, inflight) in [(Source::Mic, inflight_mic), (Source::Loopback, inflight_out)] {
            let (frame_tx, frame_rx) = sync_channel::<Vec<f32>>(512);
            let vad = SileroVad::load(&cfg.vad_model).map_err(|e| PipelineError::Vad(e.to_string()))?;
            let end_ms = match source {
                Source::Mic => cfg.me_end_silence_ms,
                Source::Loopback => cfg.others_end_silence_ms,
            };
            let ep = Endpointer::new(EndpointConfig::with_end_silence_ms(end_ms));
            let ctx = StreamCtx {
                source,
                partial_interval: cfg.partial_interval,
                partial_window: (cfg.partial_window_s * SAMPLE_RATE as f32) as usize,
                min_rms: match source {
                    Source::Mic => cfg.min_rms_me,
                    Source::Loopback => cfg.min_rms_others,
                },
                barge_in: cfg.barge_in,
                inflight,
                barge: barge.clone(),
                jobs: job_tx.clone(),
            };
            threads.push(
                thread::Builder::new()
                    .name(format!("stream-{source:?}"))
                    .spawn(move || stream_loop(ctx, vad, ep, frame_rx))
                    .map_err(|e| PipelineError::Thread(e.to_string()))?,
            );
            captures.push(Capture::start(source, frame_tx)?);
        }
        Ok(Self { captures, threads })
    }

    pub fn dropped_frames(&self) -> u64 {
        self.captures.iter().map(Capture::dropped_frames).sum()
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.captures.clear(); // stops capture, closes frame channels
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

struct StreamCtx {
    source: Source,
    partial_interval: Duration,
    partial_window: usize,
    min_rms: f32,
    barge_in: bool,
    inflight: Arc<AtomicBool>,
    barge: Arc<AtomicBool>,
    jobs: SyncSender<Job>,
}

fn stream_loop(ctx: StreamCtx, mut vad: SileroVad, mut ep: Endpointer, frames: Receiver<Vec<f32>>) {
    let mut segment_id = 0u64;
    let mut seg_start = 0u64;
    let mut last_partial = Instant::now();

    let send_final = |ctx: &StreamCtx, id: u64, start_sample: u64, audio: Vec<f32>| {
        let _ = ctx.jobs.send(Job { source: ctx.source, segment_id: id, kind: Kind::Final, audio, start_sample });
    };

    loop {
        let frame = match frames.recv_timeout(Duration::from_millis(200)) {
            Ok(f) => f,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let prob = match vad.prob(&frame) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("vad: {e}");
                continue;
            }
        };
        debug_assert_eq!(frame.len(), FRAME);
        for ev in ep.push(&frame, prob) {
            match ev {
                Event::Start { at_sample } => {
                    segment_id += 1;
                    seg_start = at_sample;
                    last_partial = Instant::now();
                    if ctx.source == Source::Mic && ctx.barge_in && rms(ep.current_audio()) >= ctx.min_rms {
                        ctx.barge.store(true, Ordering::Relaxed);
                    }
                }
                Event::End { start_sample, audio, .. } => send_final(&ctx, segment_id, start_sample, audio),
                Event::Dropped => {}
            }
        }

        // Barge-in: the local user started talking, so close the other side's open segment.
        if ctx.source == Source::Loopback && ctx.barge.swap(false, Ordering::Relaxed) && ep.is_speaking() {
            if let Some(Event::End { start_sample, audio, .. }) = ep.force_finalize() {
                send_final(&ctx, segment_id, start_sample, audio);
            }
        }

        // Streaming partial over the tail of the current segment.
        if ep.is_speaking()
            && last_partial.elapsed() >= ctx.partial_interval
            && !ctx.inflight.load(Ordering::Relaxed)
        {
            let cur = ep.current_audio();
            let tail = &cur[cur.len().saturating_sub(SAMPLE_RATE)..];
            if cur.len() >= SAMPLE_RATE / 2 && rms(tail) >= ctx.min_rms {
                let from = cur.len().saturating_sub(ctx.partial_window);
                ctx.inflight.store(true, Ordering::Relaxed);
                let job = Job {
                    source: ctx.source,
                    segment_id,
                    kind: Kind::Partial,
                    audio: cur[from..].to_vec(),
                    start_sample: seg_start,
                };
                if ctx.jobs.try_send(job).is_err() {
                    ctx.inflight.store(false, Ordering::Relaxed);
                }
                last_partial = Instant::now();
            }
        }
    }
    if let Some(Event::End { start_sample, audio, .. }) = ep.force_finalize() {
        send_final(&ctx, segment_id, start_sample, audio);
    }
}

fn worker_loop(
    engine: Engine,
    cfg: PipelineConfig,
    jobs: Receiver<Job>,
    events: Sender<TranscriptEvent>,
    inflight_mic: Arc<AtomicBool>,
    inflight_out: Arc<AtomicBool>,
) {
    let lang = cfg.language.as_deref();
    for job in jobs {
        let start_ms = job.start_sample * 1000 / SAMPLE_RATE as u64;
        let mk = |text: String, dropped: Option<String>| TranscriptEvent {
            source: job.source,
            kind: job.kind,
            segment_id: job.segment_id,
            start_ms,
            text,
            dropped,
        };

        let result = match job.kind {
            Kind::Partial => engine.transcribe(&job.audio, lang, 1, None),
            Kind::Final => engine.transcribe(&job.audio, lang, cfg.final_beam, None),
        };
        if job.kind == Kind::Partial {
            match job.source {
                Source::Mic => inflight_mic.store(false, Ordering::Relaxed),
                Source::Loopback => inflight_out.store(false, Ordering::Relaxed),
            }
        }
        let segments = match result {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("transcribe: {e}");
                continue;
            }
        };

        let min_rms = match job.source {
            Source::Mic => cfg.min_rms_me,
            Source::Loopback => cfg.min_rms_others,
        };
        let level = rms(&job.audio);
        let mut kept = Vec::new();
        let mut rejected: Vec<String> = Vec::new();
        for s in &segments {
            let stats = SegmentStats { no_speech_prob: s.no_speech_prob, avg_logprob: s.avg_logprob, vad_speech: true };
            match judge(&s.text, stats, &cfg.filter) {
                Verdict::Keep => kept.push(s.text.clone()),
                Verdict::Drop(why) => rejected.push(format!("{why}: {}", s.text)),
            }
        }
        let text = kept.join(" ");

        let ev = if level < min_rms && job.kind == Kind::Final {
            let all = segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
            (!all.is_empty()).then(|| mk(all, Some(format!("below energy gate (rms {level:.4})"))))
        } else if !text.is_empty() {
            Some(mk(text, None))
        } else if job.kind == Kind::Final && !rejected.is_empty() {
            Some(mk(String::new(), Some(rejected.join(" | "))))
        } else {
            None
        };
        if let Some(ev) = ev {
            if events.send(ev).is_err() {
                break;
            }
        }
    }
}
