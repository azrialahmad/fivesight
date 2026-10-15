//! Echo de-duplication.
//!
//! Without headphones the microphone also hears the call coming out of the speakers, so
//! whatever the other side says shows up a second time as a "me" line. Real echo
//! cancellation needs both signals aligned to the sample, and two WASAPI clocks drift
//! apart over a long meeting. Matching on text is cheaper and does not care about drift:
//! a mic line whose words were mostly just said by the other side is dropped.
//!
//! The mic usually finalizes first (shorter end-of-speech silence), so a mic final that
//! overlaps the other side still talking is held back until their text is in, or until
//! a deadline passes.

use std::time::{Duration, Instant};

use crate::TranscriptEvent;

#[derive(Debug, Clone)]
pub struct EchoConfig {
    /// Share of the mic line's words that must appear in the other side's speech.
    pub min_overlap: f32,
    /// How far apart (ms) the two lines may be and still count as the same speech.
    pub slack_ms: u64,
    /// Longest a mic final is held while waiting for the other side's text.
    pub max_hold: Duration,
    /// Other-side lines older than this (ms before the newest one) are forgotten.
    pub keep_ms: u64,
}

impl Default for EchoConfig {
    fn default() -> Self {
        Self { min_overlap: 0.6, slack_ms: 1500, max_hold: Duration::from_secs(6), keep_ms: 30_000 }
    }
}

/// Something the other side said. `end_ms` is `None` while the utterance is in progress.
struct Heard {
    segment_id: u64,
    start_ms: u64,
    end_ms: Option<u64>,
    words: Vec<String>,
}

struct Held {
    ev: TranscriptEvent,
    end_ms: u64,
    deadline: Instant,
}

pub struct EchoGuard {
    cfg: EchoConfig,
    heard: Vec<Heard>,
    held: Vec<Held>,
}

impl EchoGuard {
    pub fn new(cfg: EchoConfig) -> Self {
        Self { cfg, heard: Vec::new(), held: Vec::new() }
    }

    /// Record the other side's text (a partial replaces earlier text of the same utterance).
    /// Returns held mic finals that now turn out to be echo, already marked as dropped.
    pub fn hear_other(
        &mut self,
        segment_id: u64,
        start_ms: u64,
        end_ms: Option<u64>,
        text: &str,
    ) -> Vec<TranscriptEvent> {
        let words = words(text);
        match self.heard.iter_mut().find(|h| h.segment_id == segment_id) {
            Some(h) => {
                h.end_ms = end_ms;
                if !words.is_empty() || end_ms.is_some() {
                    h.words = words;
                }
            }
            None => self.heard.push(Heard { segment_id, start_ms, end_ms, words }),
        }
        let newest = self.heard.iter().map(|h| h.start_ms).max().unwrap_or(0);
        let keep_ms = self.cfg.keep_ms;
        self.heard.retain(|h| h.end_ms.is_none_or(|e| e + keep_ms >= newest));

        let mut echoes = Vec::new();
        let mut i = 0;
        while i < self.held.len() {
            let h = &self.held[i];
            if let Some(score) = self.echo_score(&h.ev.text, h.ev.start_ms, h.end_ms) {
                echoes.push(mark_echo(self.held.remove(i).ev, score));
            } else {
                i += 1;
            }
        }
        echoes
    }

    /// Whether a mic line looks like the other side's speech picked up by the mic.
    pub fn is_echo(&self, text: &str, start_ms: u64, end_ms: u64) -> bool {
        self.echo_score(text, start_ms, end_ms).is_some()
    }

    /// Judge a mic final. Echo comes back marked as dropped, a clean line comes back as
    /// is, and a line that can't be judged yet (the other side is mid-utterance) is held.
    pub fn mic_final(
        &mut self,
        ev: TranscriptEvent,
        end_ms: u64,
        other_talking: bool,
        now: Instant,
    ) -> Option<TranscriptEvent> {
        if let Some(score) = self.echo_score(&ev.text, ev.start_ms, end_ms) {
            return Some(mark_echo(ev, score));
        }
        if other_talking && ev.dropped.is_none() && !ev.text.trim().is_empty() {
            self.held.push(Held { ev, end_ms, deadline: now + self.cfg.max_hold });
            return None;
        }
        Some(ev)
    }

    /// Let go of held mic finals: all of them once the other side has settled (stopped
    /// talking and its last final is transcribed), otherwise only those past the deadline.
    pub fn release(&mut self, other_settled: bool, now: Instant) -> Vec<TranscriptEvent> {
        let (out, keep): (Vec<_>, Vec<_>) = self.held.drain(..).partition(|h| other_settled || now >= h.deadline);
        self.held = keep;
        out.into_iter().map(|h| h.ev).collect()
    }

    pub fn holding(&self) -> bool {
        !self.held.is_empty()
    }

    /// Whether the other side said anything around this time that a mic line could be
    /// compared against. Without it, [`is_echo`](Self::is_echo) can't tell echo apart.
    pub fn heard_near(&self, start_ms: u64, end_ms: u64) -> bool {
        self.theirs(start_ms, end_ms).next().is_some()
    }

    fn theirs(&self, start_ms: u64, end_ms: u64) -> impl Iterator<Item = &str> {
        let slack = self.cfg.slack_ms;
        self.heard
            .iter()
            .filter(move |h| h.start_ms <= end_ms + slack && h.end_ms.is_none_or(|e| e + slack >= start_ms))
            .flat_map(|h| h.words.iter().map(String::as_str))
    }

    fn echo_score(&self, text: &str, start_ms: u64, end_ms: u64) -> Option<f32> {
        let mine = words(text);
        if mine.is_empty() {
            return None;
        }
        let theirs: Vec<&str> = self.theirs(start_ms, end_ms).collect();
        if theirs.is_empty() {
            return None;
        }
        let found = mine.iter().filter(|w| theirs.iter().any(|t| same_word(w, t))).count();
        let score = found as f32 / mine.len() as f32;
        (score >= self.cfg.min_overlap).then_some(score)
    }
}

fn mark_echo(mut ev: TranscriptEvent, score: f32) -> TranscriptEvent {
    ev.dropped = Some(format!("echo of the other side ({:.0}% of words)", score * 100.0));
    ev
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// Equal, or one edit apart for longer words (Whisper often spells an echo slightly
/// differently from the clean loopback copy).
fn same_word(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().min(b.len()) < 4 || a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()] <= 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;
    use fs_audio::Source;

    fn mic(text: &str, start_ms: u64) -> TranscriptEvent {
        TranscriptEvent {
            source: Source::Mic,
            kind: Kind::Final,
            segment_id: 1,
            start_ms,
            text: text.into(),
            dropped: None,
            ended_at: 0,
            duration_ms: 0,
        }
    }

    #[test]
    fn drops_echo_of_finished_line() {
        let mut g = EchoGuard::new(EchoConfig::default());
        g.hear_other(1, 1000, Some(4000), "Can everyone see my screen now?");
        let ev = g.mic_final(mic("can everyone see my screen", 1150), 4100, false, Instant::now()).unwrap();
        assert!(ev.dropped.unwrap().starts_with("echo"));
    }

    #[test]
    fn keeps_own_words() {
        let mut g = EchoGuard::new(EchoConfig::default());
        g.hear_other(1, 1000, Some(4000), "Can everyone see my screen now?");
        let ev = g.mic_final(mic("yes I can, go ahead", 4200), 5500, false, Instant::now()).unwrap();
        assert!(ev.dropped.is_none());
    }

    #[test]
    fn ignores_lines_far_apart_in_time() {
        let mut g = EchoGuard::new(EchoConfig::default());
        g.hear_other(1, 1000, Some(4000), "let's ship it on friday");
        let ev = g.mic_final(mic("let's ship it on friday", 60_000), 62_000, false, Instant::now()).unwrap();
        assert!(ev.dropped.is_none());
    }

    #[test]
    fn tolerates_small_misspellings() {
        let mut g = EchoGuard::new(EchoConfig::default());
        g.hear_other(1, 0, Some(3000), "deployment ke staging sudah selesai");
        assert!(g.is_echo("deploymen ke stagging sudah selesai", 100, 3100));
    }

    #[test]
    fn holds_until_other_side_text_arrives() {
        let mut g = EchoGuard::new(EchoConfig::default());
        let now = Instant::now();
        assert!(g.mic_final(mic("we should move the deadline", 500), 3000, true, now).is_none());
        assert!(g.release(false, now).is_empty());
        let echoes = g.hear_other(7, 400, Some(3200), "I think we should move the deadline");
        assert_eq!(echoes.len(), 1);
        assert!(echoes[0].dropped.is_some());
        assert!(!g.holding());
    }

    #[test]
    fn releases_held_line_when_other_side_settles_or_deadline_passes() {
        let mut g = EchoGuard::new(EchoConfig::default());
        let now = Instant::now();
        assert!(g.mic_final(mic("sorry, quick question", 500), 2000, true, now).is_none());
        assert!(g.hear_other(7, 400, Some(3200), "the numbers look fine to me").is_empty());
        let out = g.release(true, now);
        assert_eq!(out.len(), 1);
        assert!(out[0].dropped.is_none());

        assert!(g.mic_final(mic("one more thing", 5000), 6000, true, now).is_none());
        assert!(g.release(false, now + Duration::from_secs(1)).is_empty());
        assert_eq!(g.release(false, now + Duration::from_secs(7)).len(), 1);
    }

    #[test]
    fn partial_text_counts_while_other_side_is_talking() {
        let mut g = EchoGuard::new(EchoConfig::default());
        g.hear_other(3, 2000, None, "so the plan for next sprint");
        assert!(g.is_echo("the plan for next", 2200, 4000));
        // The final replaces the partial's words.
        g.hear_other(3, 2000, Some(6000), "so the plan for next sprint is the importer");
        assert!(g.is_echo("next sprint is the importer", 4000, 6100));
    }

    #[test]
    fn forgets_old_lines() {
        let mut g = EchoGuard::new(EchoConfig { keep_ms: 10_000, ..EchoConfig::default() });
        g.hear_other(1, 0, Some(2000), "good morning everyone");
        g.hear_other(2, 20_000, Some(21_000), "next topic");
        assert_eq!(g.heard.len(), 1);
    }
}
