//! Live transcription pipeline.
//!
//! ```text
//! mic loopback ─► [stream thread: Silero ─► Endpointer] ─┐
//!                                                         ├─► jobs ─► [whisper worker] ─► filter ─► events
//! loopback ────► [stream thread: Silero ─► Endpointer] ─┘
//! ```
//!
//! Partials are coalesced: a stream never has more than one partial job in flight, so a
//! slow GPU cannot build a backlog. Finals are always queued. Mic lines that repeat what
//! the other side just said (speakers picked up by the mic) are dropped, see [`echo`].

pub mod echo;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use echo::{EchoConfig, EchoGuard};
use fs_audio::{rms, Capture, Source};
use fs_stt::engine::{Engine, Segment};
use fs_stt::filter::{judge, FilterConfig, SegmentStats, Verdict};
use fs_stt::hints::{echoes_prompt, retry_prompt};
use fs_stt::lang::{LangMode, LangTracker, BILINGUAL_PROMPT, MIN_DETECT_SECS};
use fs_vad::silero::SileroVad;
use fs_vad::{EndpointConfig, Endpointer, Event, FRAME, SAMPLE_RATE};

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub whisper_model: PathBuf,
    pub vad_model: PathBuf,
    /// Auto picks between Indonesian and English per utterance; Id/En pin one language.
    pub language: LangMode,
    /// Biases Whisper toward the right vocabulary and language style. `Pipeline::set_prompt`
    /// replaces it while running. The bilingual default suits Auto and Indonesian, not English.
    pub initial_prompt: Option<String>,
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
    /// Drop mic lines that are the call echoing from the speakers. `None` turns it off.
    pub echo: Option<EchoConfig>,
}

impl PipelineConfig {
    pub fn new(whisper_model: PathBuf, vad_model: PathBuf) -> Self {
        Self {
            whisper_model,
            vad_model,
            language: LangMode::Auto,
            initial_prompt: Some(BILINGUAL_PROMPT.into()),
            partial_interval: Duration::from_millis(700),
            partial_window_s: 15.0,
            me_end_silence_ms: 800,
            others_end_silence_ms: 1100,
            min_rms_me: 0.01,
            min_rms_others: 0.002,
            barge_in: true,
            final_beam: 3,
            filter: FilterConfig::default(),
            echo: Some(EchoConfig::default()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Partial,
    Final,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptEvent {
    pub source: Source,
    pub kind: Kind,
    /// Partials and the final of one utterance share an id.
    pub segment_id: u64,
    pub start_ms: u64,
    pub text: String,
    /// Set when a final was discarded (and `text` is what was discarded).
    pub dropped: Option<String>,
    /// Wall-clock time (Unix ms) when this audio ended; for a partial, when it was taken.
    /// Lines are matched to who Meet showed as speaking by this, not by sample counts, which
    /// drift from the clock over a long call.
    pub ended_at: i64,
    /// Length of the audio this text came from.
    pub duration_ms: u64,
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// How far back a speaker change may be placed: Meet's indicator, the extension's smoothing
/// and the trip to the app put the report up to about a second after the actual change.
const SPLIT_LOOKBACK_FRAMES: usize = 1_200 * SAMPLE_RATE / 1000 / FRAME;
/// Neither side of a split is shorter than this.
const SPLIT_MIN_FRAMES: usize = 400 * SAMPLE_RATE / 1000 / FRAME;

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
    ended_at: i64,
}

pub struct Pipeline {
    captures: Vec<Capture>,
    threads: Vec<JoinHandle<()>>,
    prompt: Arc<Mutex<Option<String>>>,
    split_others: Arc<AtomicBool>,
}

impl Pipeline {
    /// Load models, open both audio streams and start transcribing.
    /// Events arrive on `events`; dropping the `Pipeline` stops everything.
    pub fn start(cfg: PipelineConfig, events: Sender<TranscriptEvent>) -> Result<Self, PipelineError> {
        let (job_tx, job_rx) = sync_channel::<Job>(16);
        let inflight_mic = Arc::new(AtomicBool::new(false));
        let inflight_out = Arc::new(AtomicBool::new(false));
        let barge = Arc::new(AtomicBool::new(false));
        let other = Arc::new(OtherSide::default());
        let prompt = Arc::new(Mutex::new(cfg.initial_prompt.clone()));
        let split_others = Arc::new(AtomicBool::new(false));

        // Whisper worker owns the engine; load it here and report errors synchronously.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let worker = {
            let cfg = cfg.clone();
            let events = events.clone();
            let shared = WorkerShared {
                inflight_mic: inflight_mic.clone(),
                inflight_out: inflight_out.clone(),
                barge: barge.clone(),
                other: other.clone(),
                prompt: prompt.clone(),
            };
            thread::Builder::new()
                .name("whisper-worker".into())
                .spawn(move || match Engine::load(&cfg.whisper_model, true) {
                    Ok(engine) => {
                        let _ = ready_tx.send(Ok(()));
                        worker_loop(engine, cfg, job_rx, events, shared);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                    }
                })
                .map_err(|e| PipelineError::Thread(e.to_string()))?
        };
        ready_rx.recv().map_err(|e| PipelineError::Thread(e.to_string()))?.map_err(PipelineError::Whisper)?;

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
                inflight,
                barge: barge.clone(),
                split: split_others.clone(),
                other: other.clone(),
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
        Ok(Self { captures, threads, prompt, split_others })
    }

    /// Someone else started talking on the call (the Meet extension says so): end the other
    /// side's current line there and start a new one, even if there was no pause between them.
    pub fn split_others(&self) {
        self.split_others.store(true, Ordering::Relaxed);
    }

    /// Replace Whisper's initial prompt; the next utterance uses it.
    pub fn set_prompt(&self, prompt: Option<String>) {
        if let Ok(mut p) = self.prompt.lock() {
            *p = prompt;
        }
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
    inflight: Arc<AtomicBool>,
    barge: Arc<AtomicBool>,
    /// Loopback only: the speaker changed, cut the segment (see `Pipeline::split_others`).
    split: Arc<AtomicBool>,
    other: Arc<OtherSide>,
    jobs: SyncSender<Job>,
}

/// What the loopback stream is doing, so the worker knows whether a mic line that might
/// be echo can be judged yet.
#[derive(Default)]
struct OtherSide {
    speaking: AtomicBool,
    finals_sent: AtomicU64,
}

fn stream_loop(ctx: StreamCtx, mut vad: SileroVad, mut ep: Endpointer, frames: Receiver<Vec<f32>>) {
    let mut segment_id = 0u64;
    let mut seg_start = 0u64;
    let mut last_partial = Instant::now();

    // Frames reach this thread as they are captured, so "now" minus the frames that came in
    // after a segment's audio is when that audio ended.
    let ended_at = |tail_frames: usize| unix_ms() - (tail_frames * FRAME * 1000 / SAMPLE_RATE) as i64;
    let send_final = |ctx: &StreamCtx, id: u64, start_sample: u64, audio: Vec<f32>, tail_frames: usize| {
        if ctx.source == Source::Loopback {
            ctx.other.finals_sent.fetch_add(1, Ordering::Relaxed);
        }
        let job = Job {
            source: ctx.source,
            segment_id: id,
            kind: Kind::Final,
            audio,
            start_sample,
            ended_at: ended_at(tail_frames),
        };
        let _ = ctx.jobs.send(job);
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
        let mut events = ep.push(&frame, prob);
        // Someone else took over on the call: cut there, even without a pause.
        if ctx.split.swap(false, Ordering::Relaxed) {
            events.extend(ep.split(SPLIT_LOOKBACK_FRAMES, SPLIT_MIN_FRAMES));
        }
        for ev in events {
            match ev {
                Event::Start { at_sample } => {
                    segment_id += 1;
                    seg_start = at_sample;
                    last_partial = Instant::now();
                }
                Event::End { start_sample, audio, tail_frames, .. } => {
                    send_final(&ctx, segment_id, start_sample, audio, tail_frames)
                }
                Event::Dropped => {}
            }
        }

        // Barge-in: the local user started talking, so close the other side's open segment.
        // The worker raises the flag once a mic partial is known not to be echo.
        if ctx.source == Source::Loopback && ctx.barge.swap(false, Ordering::Relaxed) && ep.is_speaking() {
            if let Some(Event::End { start_sample, audio, tail_frames, .. }) = ep.force_finalize() {
                send_final(&ctx, segment_id, start_sample, audio, tail_frames);
            }
        }
        if ctx.source == Source::Loopback {
            ctx.other.speaking.store(ep.is_speaking(), Ordering::Relaxed);
        }

        // Streaming partial over the tail of the current segment.
        if ep.is_speaking() && last_partial.elapsed() >= ctx.partial_interval && !ctx.inflight.load(Ordering::Relaxed) {
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
                    ended_at: unix_ms(),
                };
                if ctx.jobs.try_send(job).is_err() {
                    ctx.inflight.store(false, Ordering::Relaxed);
                }
                last_partial = Instant::now();
            }
        }
    }
    if let Some(Event::End { start_sample, audio, tail_frames, .. }) = ep.force_finalize() {
        send_final(&ctx, segment_id, start_sample, audio, tail_frames);
    }
}

/// Ask Whisper how likely Indonesian and English are, then apply the sticky choice.
fn detect_and_choose(engine: &Engine, tracker: &mut LangTracker, audio: &[f32], secs: f32) -> &'static str {
    match engine.detect_among(audio, &["id", "en"]) {
        Ok(p) => tracker.choose(p[0], p[1], secs),
        Err(e) => {
            tracing::warn!("language detection failed: {e}");
            tracker.last().unwrap_or("id")
        }
    }
}

struct WorkerShared {
    inflight_mic: Arc<AtomicBool>,
    inflight_out: Arc<AtomicBool>,
    barge: Arc<AtomicBool>,
    other: Arc<OtherSide>,
    prompt: Arc<Mutex<Option<String>>>,
}

fn worker_loop(
    engine: Engine,
    cfg: PipelineConfig,
    jobs: Receiver<Job>,
    events: Sender<TranscriptEvent>,
    shared: WorkerShared,
) {
    // One tracker per audio stream (mic, loopback), and the language picked for each
    // utterance in progress so its partials don't re-detect on every pass.
    let mut trackers = [LangTracker::default(), LangTracker::default()];
    let mut seg_lang: HashMap<(usize, u64), &'static str> = HashMap::new();

    let mut echo = cfg.echo.clone().map(EchoGuard::new);
    let mut other_finals_done = 0u64;
    let mut barged_seg = 0u64; // mic utterance that already closed the other side's segment

    loop {
        // Wake up regularly even without jobs: held mic lines wait on the other side.
        let job = match jobs.recv_timeout(Duration::from_millis(150)) {
            Ok(job) => job,
            Err(RecvTimeoutError::Timeout) => {
                if !release_held(&mut echo, &shared, other_finals_done, &events) {
                    return;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if job.source == Source::Loopback && job.kind == Kind::Final {
            other_finals_done += 1;
        }
        let src = match job.source {
            Source::Mic => 0,
            Source::Loopback => 1,
        };
        let secs = job.audio.len() as f32 / SAMPLE_RATE as f32;
        let key = (src, job.segment_id);
        let lang: Option<&'static str> = match cfg.language {
            LangMode::Id => Some("id"),
            LangMode::En => Some("en"),
            LangMode::Auto => Some(match job.kind {
                Kind::Partial => match seg_lang.get(&key) {
                    Some(l) => *l,
                    None if secs >= MIN_DETECT_SECS => {
                        let l = detect_and_choose(&engine, &mut trackers[src], &job.audio, secs);
                        seg_lang.insert(key, l);
                        l
                    }
                    None => trackers[src].last().unwrap_or("id"),
                },
                Kind::Final => {
                    let l = if secs >= MIN_DETECT_SECS {
                        detect_and_choose(&engine, &mut trackers[src], &job.audio, secs)
                    } else {
                        seg_lang.get(&key).copied().or(trackers[src].last()).unwrap_or("id")
                    };
                    seg_lang.remove(&key);
                    l
                }
            }),
        };
        if seg_lang.len() > 64 {
            seg_lang.clear(); // utterances dropped before their final never clean up
        }

        let start_ms = job.start_sample * 1000 / SAMPLE_RATE as u64;
        let mk = |text: String, dropped: Option<String>| TranscriptEvent {
            source: job.source,
            kind: job.kind,
            segment_id: job.segment_id,
            start_ms,
            text,
            dropped,
            ended_at: job.ended_at,
            duration_ms: job.audio.len() as u64 * 1000 / SAMPLE_RATE as u64,
        };

        let prompt = shared.prompt.lock().ok().and_then(|p| p.clone());
        let result = match job.kind {
            Kind::Partial => engine.transcribe(&job.audio, lang, 1, prompt.as_deref()),
            Kind::Final => engine.transcribe(&job.audio, lang, cfg.final_beam, prompt.as_deref()),
        };
        if job.kind == Kind::Partial {
            match job.source {
                Source::Mic => shared.inflight_mic.store(false, Ordering::Relaxed),
                Source::Loopback => shared.inflight_out.store(false, Ordering::Relaxed),
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
        let (mut text, rejected) = judge_all(&segments, &cfg.filter);
        let mut parroted = !text.is_empty() && prompt.as_deref().is_some_and(|p| echoes_prompt(&text, p));
        if parroted && job.kind == Kind::Final && level >= min_rms {
            let base = cfg.language.base_prompt();
            if let Some(again) =
                listen_again(&engine, &job.audio, lang, cfg.final_beam, prompt.as_deref(), base, &cfg.filter)
            {
                tracing::debug!("prompt echo \"{text}\" heard again without it: {again}");
                text = again;
                parroted = false;
            }
        }

        let ev = if level < min_rms && job.kind == Kind::Final {
            let all = segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
            (!all.is_empty()).then(|| mk(all, Some(format!("below energy gate (rms {level:.4})"))))
        } else if !text.is_empty() {
            let why = || format!("repeats the prompt: {:?}, {secs:.1} s, {}", job.source, lang.unwrap_or("auto"));
            Some(mk(text, parroted.then(why)))
        } else if job.kind == Kind::Final && !rejected.is_empty() {
            Some(mk(String::new(), Some(rejected.join(" | "))))
        } else {
            None
        };
        let end_ms = start_ms + job.audio.len() as u64 * 1000 / SAMPLE_RATE as u64;
        let other_talking = shared.other.speaking.load(Ordering::Relaxed);
        let mut out = Vec::new();
        match (echo.as_mut(), job.source, ev) {
            (Some(g), Source::Loopback, ev) => {
                let heard = ev.as_ref().filter(|e| e.dropped.is_none()).map_or("", |e| e.text.as_str());
                let end = (job.kind == Kind::Final).then_some(end_ms);
                out.extend(g.hear_other(job.segment_id, start_ms, end, heard));
                out.extend(ev);
            }
            (Some(g), Source::Mic, Some(ev)) if ev.dropped.is_none() => match job.kind {
                Kind::Partial => out.extend((!g.is_echo(&ev.text, start_ms, end_ms)).then_some(ev)),
                Kind::Final => out.extend(g.mic_final(ev, end_ms, other_talking, Instant::now())),
            },
            (_, _, ev) => out.extend(ev),
        }

        // A mic partial with real words (not echo) means the local user is talking. While the
        // other side talks, wait until there is something of theirs to rule out echo with.
        let speaking_up = job.source == Source::Mic
            && job.kind == Kind::Partial
            && out.iter().any(|e| e.dropped.is_none() && !e.text.trim().is_empty())
            && (!other_talking || echo.as_ref().is_none_or(|g| g.heard_near(start_ms, end_ms)));
        if cfg.barge_in && speaking_up && barged_seg != job.segment_id {
            barged_seg = job.segment_id;
            shared.barge.store(true, Ordering::Relaxed);
        }

        for ev in out {
            if events.send(ev).is_err() {
                return;
            }
        }
        if !release_held(&mut echo, &shared, other_finals_done, &events) {
            return;
        }
    }
    if let Some(g) = echo.as_mut() {
        for ev in g.release(true, Instant::now()) {
            let _ = events.send(ev);
        }
    }
}

/// Second try at an utterance whose text only repeated `prompt`: the same audio with the
/// glossary but without the `base` example sentence. Returns the text when that hears
/// something other than its own (smaller) prompt, which means someone said it.
pub fn listen_again(
    engine: &Engine,
    audio: &[f32],
    lang: Option<&str>,
    beam: i32,
    prompt: Option<&str>,
    base: Option<&str>,
    filter: &FilterConfig,
) -> Option<String> {
    let retry = prompt.and_then(|p| retry_prompt(p, base));
    let segments = engine
        .transcribe(audio, lang, beam, retry.as_deref())
        .map_err(|e| tracing::error!("transcribe retry: {e}"))
        .ok()?;
    let (text, _) = judge_all(&segments, filter);
    let echo = retry.as_deref().is_some_and(|p| echoes_prompt(&text, p));
    (!text.is_empty() && !echo).then_some(text)
}

/// Text of the segments the filter keeps, and why the others were dropped.
pub fn judge_all(segments: &[Segment], filter: &FilterConfig) -> (String, Vec<String>) {
    let mut kept = Vec::new();
    let mut rejected = Vec::new();
    for s in segments {
        let stats = SegmentStats { no_speech_prob: s.no_speech_prob, avg_logprob: s.avg_logprob, vad_speech: true };
        match judge(&s.text, stats, filter) {
            Verdict::Keep => kept.push(s.text.clone()),
            Verdict::Drop(why) => rejected.push(format!("{why}: {}", s.text)),
        }
    }
    (trim_ellipsis(&kept.join(" ")), rejected)
}

/// Drop the "..." Whisper puts where audio was cut mid-sentence: at the end of every partial
/// (transcribed while the speaker is still talking) and of finals split at a pause.
fn trim_ellipsis(text: &str) -> String {
    let dots = |c: char| c == '.' || c == '…' || c.is_whitespace();
    // Only an actual ellipsis counts, not a full stop or ".NET".
    let is_ellipsis = |run: &str| run.contains('…') || run.matches('.').count() >= 2;
    let t = text.trim();
    let rest = t.trim_start_matches(dots);
    let t = if is_ellipsis(&t[..t.len() - rest.len()]) { rest } else { t };
    let cut = t.trim_end_matches(dots);
    let t = if is_ellipsis(&t[cut.len()..]) { cut } else { t };
    t.to_string()
}

/// Send held mic lines that can be judged now. Returns false once nobody is listening.
fn release_held(
    echo: &mut Option<EchoGuard>,
    shared: &WorkerShared,
    other_finals_done: u64,
    events: &Sender<TranscriptEvent>,
) -> bool {
    let Some(g) = echo.as_mut().filter(|g| g.holding()) else {
        return true;
    };
    let settled = !shared.other.speaking.load(Ordering::Relaxed)
        && shared.other.finals_sent.load(Ordering::Relaxed) == other_finals_done;
    g.release(settled, Instant::now()).into_iter().all(|ev| events.send(ev).is_ok())
}

#[cfg(test)]
mod tests {
    use super::trim_ellipsis;

    #[test]
    fn trims_cut_off_ellipses_only() {
        assert_eq!(trim_ellipsis("Ya, jadi kenapa sih..."), "Ya, jadi kenapa sih");
        assert_eq!(trim_ellipsis(" ...dan nanti kita deploy… "), "dan nanti kita deploy");
        assert_eq!(trim_ellipsis("Contohnya. . ."), "Contohnya");
        assert_eq!(trim_ellipsis("..."), "");
        assert_eq!(trim_ellipsis("Oke, selesai."), "Oke, selesai.");
        assert_eq!(trim_ellipsis(".NET 8 sudah rilis?"), ".NET 8 sudah rilis?");
        assert_eq!(trim_ellipsis("Versi 3.5 ya"), "Versi 3.5 ya");
        assert_eq!(trim_ellipsis("Jadi... itu dia"), "Jadi... itu dia");
    }
}
