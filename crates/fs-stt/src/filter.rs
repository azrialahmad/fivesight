//! Hallucination defense for Whisper output.
//!
//! Improves on the old pipeline: adds repetition and repeated n-gram checks,
//! requires VAD agreement, and the phrase list is configurable.

#[derive(Debug, Clone)]
pub struct FilterConfig {
    pub no_speech_prob_drop: f32,
    pub avg_logprob_drop: f32,
    pub max_repetition_ratio: f32,
    pub short_text_max_words: usize,
    pub blocked_phrases: Vec<String>,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            no_speech_prob_drop: 0.6,
            avg_logprob_drop: -1.0,
            max_repetition_ratio: 2.4,
            short_text_max_words: 6,
            blocked_phrases: ["terimakasih", "terima kasih", "selamat menikmati"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SegmentStats {
    pub no_speech_prob: f32,
    pub avg_logprob: f32,
    /// Whether the VAD saw speech in this segment's audio.
    pub vad_speech: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Keep,
    Drop(&'static str),
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

/// Total words divided by unique words. Loops like "a a a a a" score high.
fn repetition_ratio(words: &[&str]) -> f32 {
    if words.is_empty() {
        return 1.0;
    }
    let mut uniq: Vec<&str> = words.to_vec();
    uniq.sort_unstable();
    uniq.dedup();
    words.len() as f32 / uniq.len() as f32
}

/// True when a 1-3 word phrase repeats back to back four or more times.
fn has_repeated_ngram(words: &[&str]) -> bool {
    for n in 1..=3usize {
        if words.len() < n * 4 {
            continue;
        }
        let mut run = 1;
        let mut i = n;
        while i + n <= words.len() {
            if words[i..i + n] == words[i - n..i] {
                run += 1;
                if run >= 4 {
                    return true;
                }
            } else {
                run = 1;
            }
            i += n;
        }
    }
    false
}

pub fn judge(text: &str, st: SegmentStats, cfg: &FilterConfig) -> Verdict {
    let norm = normalize(text);
    let words: Vec<&str> = norm.split_whitespace().collect();
    if words.is_empty() {
        return Verdict::Drop("empty");
    }
    if !st.vad_speech {
        return Verdict::Drop("no vad speech");
    }
    if st.no_speech_prob > cfg.no_speech_prob_drop {
        return Verdict::Drop("no_speech_prob");
    }
    if st.avg_logprob < cfg.avg_logprob_drop {
        return Verdict::Drop("avg_logprob");
    }
    if words.len() >= 8 && repetition_ratio(&words) > cfg.max_repetition_ratio {
        return Verdict::Drop("repetitive");
    }
    if has_repeated_ngram(&words) {
        return Verdict::Drop("repeated ngram");
    }
    if words.len() <= cfg.short_text_max_words {
        let squashed: String = words.concat();
        for p in &cfg.blocked_phrases {
            let p: String = normalize(p).split_whitespace().collect();
            if p.is_empty() {
                continue;
            }
            if squashed.len() % p.len() == 0 && squashed == p.repeat(squashed.len() / p.len()) {
                return Verdict::Drop("blocked phrase");
            }
        }
    }
    Verdict::Keep
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: SegmentStats = SegmentStats {
        no_speech_prob: 0.1,
        avg_logprob: -0.3,
        vad_speech: true,
    };

    #[test]
    fn keeps_normal_speech() {
        assert_eq!(
            judge("kita review ticket itu besok pagi", OK, &FilterConfig::default()),
            Verdict::Keep
        );
    }

    #[test]
    fn drops_blocked_and_repeated_phrase() {
        let c = FilterConfig::default();
        assert!(matches!(judge("Terima kasih.", OK, &c), Verdict::Drop(_)));
        assert!(matches!(
            judge("terima kasih terima kasih", OK, &c),
            Verdict::Drop(_)
        ));
    }

    #[test]
    fn keeps_phrase_inside_real_sentence() {
        assert_eq!(
            judge(
                "terima kasih atas update kemarin pak",
                OK,
                &FilterConfig::default()
            ),
            Verdict::Keep
        );
    }

    #[test]
    fn drops_low_confidence() {
        let c = FilterConfig::default();
        let hi = SegmentStats {
            no_speech_prob: 0.9,
            ..OK
        };
        let lo = SegmentStats {
            avg_logprob: -1.5,
            ..OK
        };
        let nv = SegmentStats {
            vad_speech: false,
            ..OK
        };
        assert_eq!(judge("hello there", hi, &c), Verdict::Drop("no_speech_prob"));
        assert_eq!(judge("hello there", lo, &c), Verdict::Drop("avg_logprob"));
        assert_eq!(judge("hello there", nv, &c), Verdict::Drop("no vad speech"));
    }

    #[test]
    fn drops_loops() {
        let c = FilterConfig::default();
        assert!(matches!(
            judge("yeah yeah yeah yeah yeah yeah yeah yeah yeah", OK, &c),
            Verdict::Drop(_)
        ));
        assert!(matches!(
            judge("we go to the we go to the we go to the we go to the", OK, &c),
            Verdict::Drop(_)
        ));
    }
}
