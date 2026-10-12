//! Voice-activity endpointing.
//!
//! [`Endpointer`] turns a stream of fixed-size audio frames plus a per-frame speech
//! probability (from Silero or any other model) into speech segments.
//! It owns the audio buffer, so callers get ready-to-transcribe segments, and it can
//! hand out a snapshot of the in-progress segment for streaming partials.

use std::collections::VecDeque;

#[cfg(feature = "silero")]
pub mod silero;

/// Samples per frame. Silero VAD at 16 kHz expects 512 (32 ms).
pub const FRAME: usize = 512;
pub const SAMPLE_RATE: usize = 16_000;

#[derive(Debug, Clone)]
pub struct EndpointConfig {
    /// Frame counts as speech at or above this probability.
    pub threshold: f32,
    /// Hysteresis: once speaking, a frame only counts as silence below `threshold - hysteresis`.
    pub hysteresis: f32,
    /// Consecutive speech frames needed to open a segment.
    pub start_frames: usize,
    /// Consecutive silent frames needed to close a segment.
    pub end_frames: usize,
    /// Audio kept from before the detected start, so the first word is not clipped.
    pub preroll_frames: usize,
    /// Segments with less speech than this are dropped (coughs, clicks).
    pub min_speech_frames: usize,
    /// A segment is split once it reaches this length.
    pub max_segment_frames: usize,
}

fn ms_to_frames(ms: usize) -> usize {
    (ms * SAMPLE_RATE / 1000).div_ceil(FRAME)
}

impl EndpointConfig {
    /// `end_silence_ms`: 800 for the mic, about 1100 for remote audio (old pipeline's values).
    pub fn with_end_silence_ms(end_silence_ms: usize) -> Self {
        Self {
            threshold: 0.5,
            hysteresis: 0.15,
            start_frames: 3,
            end_frames: ms_to_frames(end_silence_ms),
            preroll_frames: ms_to_frames(200),
            min_speech_frames: ms_to_frames(500),
            max_segment_frames: ms_to_frames(30_000),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A segment opened. `at_sample` is the absolute sample index where its audio begins.
    Start { at_sample: u64 },
    /// A segment closed and is ready for the final transcription pass.
    End {
        start_sample: u64,
        audio: Vec<f32>,
        /// True when closed because of `max_segment_frames`, not silence.
        forced: bool,
    },
    /// The segment was too short and has been discarded.
    Dropped,
}

#[derive(PartialEq, Eq)]
enum State {
    Idle,
    Speaking,
}

pub struct Endpointer {
    cfg: EndpointConfig,
    state: State,
    preroll: VecDeque<Vec<f32>>,
    pending_start: Vec<Vec<f32>>,
    buf: Vec<f32>,
    seg_start_sample: u64,
    speech_frames: usize,
    silence_run: usize,
    frames_seen: u64,
}

impl Endpointer {
    pub fn new(cfg: EndpointConfig) -> Self {
        Self {
            cfg,
            state: State::Idle,
            preroll: VecDeque::new(),
            pending_start: Vec::new(),
            buf: Vec::new(),
            seg_start_sample: 0,
            speech_frames: 0,
            silence_run: 0,
            frames_seen: 0,
        }
    }

    pub fn is_speaking(&self) -> bool {
        self.state == State::Speaking
    }

    /// Audio of the in-progress segment so far (empty when idle). Used for streaming partials.
    pub fn current_audio(&self) -> &[f32] {
        &self.buf
    }

    /// Feed one frame of exactly [`FRAME`] samples with its speech probability.
    pub fn push(&mut self, frame: &[f32], prob: f32) -> Vec<Event> {
        debug_assert_eq!(frame.len(), FRAME);
        let mut events = Vec::new();
        let idx = self.frames_seen;
        self.frames_seen += 1;

        match self.state {
            State::Idle => {
                if prob >= self.cfg.threshold {
                    self.pending_start.push(frame.to_vec());
                    if self.pending_start.len() >= self.cfg.start_frames {
                        let pre = self.preroll.len();
                        let first = idx + 1 - self.pending_start.len() as u64 - pre as u64;
                        self.seg_start_sample = first * FRAME as u64;
                        self.buf.clear();
                        for f in self.preroll.drain(..) {
                            self.buf.extend_from_slice(&f);
                        }
                        for f in self.pending_start.drain(..) {
                            self.buf.extend_from_slice(&f);
                        }
                        self.speech_frames = self.cfg.start_frames;
                        self.silence_run = 0;
                        self.state = State::Speaking;
                        events.push(Event::Start { at_sample: self.seg_start_sample });
                    }
                } else {
                    // A blip that did not reach start_frames becomes ordinary pre-roll.
                    let pending = std::mem::take(&mut self.pending_start);
                    for f in pending {
                        self.push_preroll(f);
                    }
                    self.push_preroll(frame.to_vec());
                }
            }
            State::Speaking => {
                self.buf.extend_from_slice(frame);
                if prob >= self.cfg.threshold - self.cfg.hysteresis {
                    self.silence_run = 0;
                    if prob >= self.cfg.threshold {
                        self.speech_frames += 1;
                    }
                } else {
                    self.silence_run += 1;
                }

                if self.silence_run >= self.cfg.end_frames {
                    events.push(self.close(false));
                } else if self.buf.len() >= self.cfg.max_segment_frames * FRAME {
                    events.push(self.close(true));
                    if self.state == State::Idle {
                        // Speech continues across the split: reopen straight away.
                        self.state = State::Speaking;
                        self.seg_start_sample = (idx + 1) * FRAME as u64;
                        self.speech_frames = 0;
                        self.silence_run = 0;
                        events.push(Event::Start { at_sample: self.seg_start_sample });
                    }
                }
            }
        }
        events
    }

    /// Close the active segment now (barge-in, or the stream ended).
    pub fn force_finalize(&mut self) -> Option<Event> {
        (self.state == State::Speaking).then(|| self.close(false))
    }

    fn push_preroll(&mut self, f: Vec<f32>) {
        self.preroll.push_back(f);
        while self.preroll.len() > self.cfg.preroll_frames {
            self.preroll.pop_front();
        }
    }

    fn close(&mut self, forced: bool) -> Event {
        let mut audio = std::mem::take(&mut self.buf);
        // Trim trailing silence, but keep a short tail so the last word is not clipped.
        let keep_tail = self.cfg.preroll_frames.min(self.silence_run);
        let trim = (self.silence_run - keep_tail) * FRAME;
        audio.truncate(audio.len().saturating_sub(trim));

        let long_enough = self.speech_frames >= self.cfg.min_speech_frames;
        let start_sample = self.seg_start_sample;
        self.state = State::Idle;
        self.silence_run = 0;
        self.speech_frames = 0;
        self.preroll.clear();
        self.pending_start.clear();

        if long_enough {
            Event::End { start_sample, audio, forced }
        } else {
            Event::Dropped
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ep: &mut Endpointer, pattern: &[(usize, f32)]) -> Vec<Event> {
        let frame = vec![0.1f32; FRAME];
        let mut out = Vec::new();
        for &(n, p) in pattern {
            for _ in 0..n {
                out.extend(ep.push(&frame, p));
            }
        }
        out
    }

    fn cfg() -> EndpointConfig {
        EndpointConfig::with_end_silence_ms(800)
    }

    #[test]
    fn silence_produces_nothing() {
        let mut ep = Endpointer::new(cfg());
        assert!(run(&mut ep, &[(200, 0.02)]).is_empty());
    }

    #[test]
    fn one_utterance_start_then_end() {
        let mut ep = Endpointer::new(cfg());
        // 1 s silence, 2 s speech (62 frames), 1.5 s silence
        let ev = run(&mut ep, &[(31, 0.0), (62, 0.9), (47, 0.0)]);
        assert!(matches!(ev[0], Event::Start { .. }));
        let Event::End { audio, forced, .. } = &ev[1] else { panic!("{ev:?}") };
        assert!(!forced);
        // ~2 s speech + 200 ms preroll + 200 ms tail
        let secs = audio.len() as f32 / SAMPLE_RATE as f32;
        assert!((2.2..2.8).contains(&secs), "{secs}");
        assert_eq!(ev.len(), 2);
    }

    #[test]
    fn preroll_included_in_start_sample() {
        let mut ep = Endpointer::new(cfg());
        let ev = run(&mut ep, &[(31, 0.0), (62, 0.9), (47, 0.0)]);
        let Event::Start { at_sample } = ev[0] else { panic!() };
        // Speech begins at frame 31; start should be ~7 frames (200 ms) earlier.
        assert_eq!(at_sample, (31 - cfg().preroll_frames as u64) * FRAME as u64);
    }

    #[test]
    fn short_pause_does_not_split() {
        let mut ep = Endpointer::new(cfg());
        // 0.4 s pause is below the 0.8 s end silence.
        let ev = run(&mut ep, &[(40, 0.9), (12, 0.0), (40, 0.9), (47, 0.0)]);
        assert_eq!(ev.iter().filter(|e| matches!(e, Event::End { .. })).count(), 1);
    }

    #[test]
    fn long_pause_splits_in_two() {
        let mut ep = Endpointer::new(cfg());
        let ev = run(&mut ep, &[(40, 0.9), (47, 0.0), (40, 0.9), (47, 0.0)]);
        assert_eq!(ev.iter().filter(|e| matches!(e, Event::End { .. })).count(), 2);
    }

    #[test]
    fn click_is_not_a_segment() {
        let mut ep = Endpointer::new(cfg());
        // Two speech frames never reach start_frames (3).
        assert!(run(&mut ep, &[(2, 0.95), (100, 0.0)]).is_empty());
    }

    #[test]
    fn too_short_speech_is_dropped() {
        let mut ep = Endpointer::new(cfg());
        // 5 speech frames (160 ms) < 500 ms minimum.
        let ev = run(&mut ep, &[(5, 0.9), (47, 0.0)]);
        assert_eq!(ev.last(), Some(&Event::Dropped));
    }

    #[test]
    fn hysteresis_keeps_borderline_frames_as_speech() {
        let mut ep = Endpointer::new(cfg());
        // 0.40 is below 0.5 but above 0.35, so it must not count toward the end silence.
        let ev = run(&mut ep, &[(40, 0.9), (60, 0.40), (47, 0.0)]);
        let Event::End { audio, .. } = ev.iter().find(|e| matches!(e, Event::End { .. })).unwrap() else { panic!() };
        assert!(audio.len() > 100 * FRAME - 8 * FRAME);
    }

    #[test]
    fn long_speech_is_force_split_and_continues() {
        let mut c = cfg();
        c.max_segment_frames = 100;
        let mut ep = Endpointer::new(c);
        let ev = run(&mut ep, &[(250, 0.9), (47, 0.0)]);
        let forced = ev.iter().filter(|e| matches!(e, Event::End { forced: true, .. })).count();
        assert_eq!(forced, 2);
        assert!(matches!(ev.last(), Some(Event::End { forced: false, .. } | Event::Dropped)));
    }

    #[test]
    fn force_finalize_closes_active_segment() {
        let mut ep = Endpointer::new(cfg());
        run(&mut ep, &[(60, 0.9)]);
        assert!(ep.is_speaking());
        assert!(matches!(ep.force_finalize(), Some(Event::End { .. })));
        assert!(!ep.is_speaking());
        assert!(ep.force_finalize().is_none());
    }

    #[test]
    fn current_audio_grows_for_partials() {
        let mut ep = Endpointer::new(cfg());
        run(&mut ep, &[(10, 0.9)]);
        let a = ep.current_audio().len();
        run(&mut ep, &[(10, 0.9)]);
        assert!(ep.current_audio().len() > a);
    }
}
