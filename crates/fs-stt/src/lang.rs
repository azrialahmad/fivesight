//! Language choice for Indonesian + English meetings.
//!
//! Whisper's own auto-detect picks from ~100 languages on every short utterance, so
//! Indonesian sentences with English words often come back as Malay, Javanese and so on.
//! Here detection is restricted to the two languages that matter and made sticky.

/// Bilingual prompt that nudges Whisper to keep English terms inside Indonesian speech.
///
/// Whisper sometimes says its prompt back on short or unclear audio, and a line that only
/// repeats the prompt is dropped, taking any real words with it. So this is one Indonesian
/// sentence with the English inside it: no stand-alone English clause (the old "Let's review
/// the sprint backlog" came back 34 times in one hour-long meeting) and no greeting
/// or opener that people say on their own.
pub const BILINGUAL_PROMPT: &str = "Jadi PR-nya udah di-review, tinggal deploy ke staging terus update ticket-nya.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LangMode {
    /// Choose between Indonesian and English for each utterance.
    #[default]
    Auto,
    Id,
    En,
}

impl LangMode {
    /// `"id"`, `"en"`; anything else (including `"auto"`) is [`LangMode::Auto`].
    pub fn parse(s: &str) -> Self {
        match s {
            "id" => Self::Id,
            "en" => Self::En,
            _ => Self::Auto,
        }
    }

    /// The prompt sentence that sets the language style: none when only English is spoken.
    pub fn base_prompt(self) -> Option<&'static str> {
        match self {
            Self::En => None,
            _ => Some(BILINGUAL_PROMPT),
        }
    }
}

/// Shortest audio, in seconds, whose detection result is trusted.
pub const MIN_DETECT_SECS: f32 = 1.2;
/// A language already in use is only dropped when the other one wins by this share.
const SWITCH_SHARE: f32 = 0.65;
/// If id + en together get less than this probability, the audio is neither (or is noise).
const MIN_TOTAL: f32 = 0.05;

/// Remembers the last language on one audio stream and applies the stickiness rules.
#[derive(Debug, Default)]
pub struct LangTracker {
    last: Option<&'static str>,
}

impl LangTracker {
    pub fn last(&self) -> Option<&'static str> {
        self.last
    }

    /// Pick `"id"` or `"en"` from the detector's probabilities for those two languages.
    pub fn choose(&mut self, p_id: f32, p_en: f32, audio_secs: f32) -> &'static str {
        let total = p_id + p_en;
        let pick = if audio_secs < MIN_DETECT_SECS || total < MIN_TOTAL {
            // Too little evidence: keep going in the current language (Indonesian to start).
            self.last.unwrap_or("id")
        } else {
            let id_share = p_id / total;
            match self.last {
                Some("id") => {
                    if 1.0 - id_share > SWITCH_SHARE {
                        "en"
                    } else {
                        "id"
                    }
                }
                Some(_) => {
                    if id_share > SWITCH_SHARE {
                        "id"
                    } else {
                        "en"
                    }
                }
                None => {
                    if id_share >= 0.5 {
                        "id"
                    } else {
                        "en"
                    }
                }
            }
        };
        self.last = Some(pick);
        pick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes() {
        assert_eq!(LangMode::parse("id"), LangMode::Id);
        assert_eq!(LangMode::parse("en"), LangMode::En);
        assert_eq!(LangMode::parse("auto"), LangMode::Auto);
        assert_eq!(LangMode::parse("ms"), LangMode::Auto);
    }

    #[test]
    fn bilingual_prompt_is_one_mixed_sentence() {
        let p = BILINGUAL_PROMPT;
        assert_eq!(p.matches(['.', '!', '?']).count(), 1, "{p}");
        assert!(p.ends_with('.'));
        // Indonesian carries the sentence, English terms take Indonesian suffixes.
        for w in ["udah", "tinggal", "PR-nya", "di-review", "deploy", "staging"] {
            assert!(p.contains(w), "missing {w}");
        }
        assert_eq!(LangMode::En.base_prompt(), None);
        assert_eq!(LangMode::Auto.base_prompt(), Some(p));
    }

    #[test]
    fn first_utterance_follows_the_detector() {
        assert_eq!(LangTracker::default().choose(0.2, 0.7, 3.0), "en");
        assert_eq!(LangTracker::default().choose(0.7, 0.2, 3.0), "id");
    }

    #[test]
    fn short_audio_keeps_previous_language() {
        let mut t = LangTracker::default();
        t.choose(0.9, 0.05, 3.0);
        assert_eq!(t.choose(0.1, 0.8, 0.6), "id");
    }

    #[test]
    fn short_audio_with_no_history_defaults_to_indonesian() {
        assert_eq!(LangTracker::default().choose(0.1, 0.8, 0.5), "id");
    }

    #[test]
    fn weak_evidence_does_not_flip_language() {
        let mut t = LangTracker::default();
        t.choose(0.9, 0.05, 3.0);
        // English wins only 55 percent: below the 65 percent needed to switch.
        assert_eq!(t.choose(0.45, 0.55, 3.0), "id");
    }

    #[test]
    fn clear_evidence_flips_in_both_directions() {
        let mut t = LangTracker::default();
        t.choose(0.9, 0.05, 3.0);
        assert_eq!(t.choose(0.1, 0.8, 3.0), "en");
        assert_eq!(t.choose(0.8, 0.1, 3.0), "id");
    }

    #[test]
    fn neither_language_present_keeps_current() {
        // Malay/Javanese-looking audio: almost no mass on id or en.
        let mut t = LangTracker::default();
        t.choose(0.6, 0.3, 3.0);
        assert_eq!(t.choose(0.01, 0.02, 3.0), "id");
    }
}
