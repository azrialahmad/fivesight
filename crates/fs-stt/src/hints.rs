//! Vocabulary hints for Whisper.
//!
//! Whisper spells unfamiliar names and jargon by sound ("play right" for Playwright). Words
//! that appear in its initial prompt are much more likely to come out right, so the prompt
//! is built from what the meeting is about: the user's own word list, who is on the call,
//! the meeting title and distinctive terms from the notes.

/// Whisper keeps at most 224 prompt tokens. Names and jargon run about 3 characters per
/// token, so this leaves room for the base sentence.
const MAX_CHARS: usize = 560;

/// Capitalized words that start a lot of sentences or name days and months; not worth a hint.
const COMMON: &[&str] = &[
    "a",
    "an",
    "and",
    "the",
    "i",
    "we",
    "you",
    "he",
    "she",
    "they",
    "it",
    "this",
    "that",
    "these",
    "those",
    "my",
    "our",
    "your",
    "is",
    "are",
    "was",
    "to",
    "for",
    "of",
    "in",
    "on",
    "at",
    "with",
    "if",
    "so",
    "but",
    "or",
    "not",
    "no",
    "yes",
    "ok",
    "okay",
    "oke",
    "todo",
    "done",
    "next",
    "notes",
    "note",
    "meeting",
    "untitled",
    "question",
    "questions",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "saya",
    "aku",
    "kita",
    "kami",
    "kamu",
    "anda",
    "dia",
    "mereka",
    "ini",
    "itu",
    "dan",
    "yang",
    "di",
    "ke",
    "dari",
    "untuk",
    "dengan",
    "jadi",
    "terus",
    "tapi",
    "ya",
    "tidak",
    "nggak",
    "sudah",
    "belum",
    "senin",
    "selasa",
    "rabu",
    "kamis",
    "jumat",
    "sabtu",
    "minggu",
    "you're",
    "i'm",
    "let's",
];

fn is_common(w: &str) -> bool {
    COMMON.contains(&w.to_lowercase().as_str())
}

/// Strip markdown and punctuation from the edges, keeping inner `.`, `-`, `+`, `#` (Node.js, C++).
fn clean(tok: &str) -> &str {
    let t = tok.trim_start_matches(|c: char| !c.is_alphanumeric());
    t.trim_end_matches(|c: char| !(c.is_alphanumeric() || c == '+' || c == '#'))
}

/// A word Whisper is unlikely to spell right without help.
fn distinctive(w: &str, sentence_start: bool) -> bool {
    let chars: Vec<char> = w.chars().collect();
    if chars.len() < 2 || w.contains('/') || w.contains('@') || chars.iter().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let has_letter = chars.iter().any(|c| c.is_alphabetic());
    let has_digit = chars.iter().any(|c| c.is_ascii_digit());
    let inner_upper = chars[1..].iter().any(|c| c.is_uppercase()) && chars.iter().any(|c| c.is_lowercase());
    let dotted = w.contains('.') && has_letter;
    let acronym = chars.len() <= 6 && chars.iter().all(|c| c.is_uppercase() || c.is_ascii_digit()) && has_letter;
    let capitalized = chars[0].is_uppercase() && chars[1..].iter().all(|c| !c.is_uppercase());
    if (has_letter && has_digit) || inner_upper || dotted || acronym {
        return !is_common(w);
    }
    capitalized && !sentence_start && !is_common(w)
}

/// Distinctive terms in free text (meeting notes), in order of first appearance.
/// Neighbouring capitalized words stay together, so "Budi Santoso" is one term.
pub fn terms_from(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut sentence_start = true;
        let mut run: Vec<&str> = Vec::new();
        let flush = |run: &mut Vec<&str>, out: &mut Vec<String>| {
            if !run.is_empty() {
                push_unique(out, run.join(" "));
                run.clear();
            }
        };
        for raw in line.split_whitespace() {
            let w = clean(raw);
            if w.is_empty() {
                // A bullet or heading marker: what follows starts a sentence.
                continue;
            }
            if distinctive(w, sentence_start) {
                run.push(w);
            } else {
                flush(&mut run, &mut out);
            }
            let ends = raw.ends_with(['.', '!', '?', ':', ';']) && !w.contains('.');
            if ends || raw.ends_with([',', ')']) {
                flush(&mut run, &mut out);
            }
            sentence_start = ends;
        }
        flush(&mut run, &mut out);
    }
    out
}

/// Words from the user's list: separated by commas, semicolons or new lines.
pub fn split_list(list: &str) -> Vec<String> {
    let mut out = Vec::new();
    for w in list.split([',', ';', '\n']) {
        let w = w.trim();
        if !w.is_empty() {
            push_unique(&mut out, w.to_string());
        }
    }
    out
}

fn push_unique(out: &mut Vec<String>, w: String) {
    if !out.iter().any(|x| x.eq_ignore_ascii_case(&w)) {
        out.push(w);
    }
}

/// What the meeting is about, most important first.
#[derive(Debug, Default, Clone)]
pub struct Hints {
    /// The user's own list of words to listen for.
    pub vocabulary: Vec<String>,
    /// People on the call.
    pub names: Vec<String>,
    pub title: Option<String>,
    /// Distinctive terms from the notes.
    pub terms: Vec<String>,
}

/// The initial prompt: the hints as a short glossary, then `base` (the sentence that sets
/// the language style). Lower-priority words are left out when it would get too long.
pub fn build_prompt(base: Option<&str>, h: &Hints) -> Option<String> {
    let base = base.unwrap_or("").trim();
    let mut budget = MAX_CHARS.saturating_sub(base.len());
    let mut parts: Vec<String> = Vec::new();

    if let Some(t) = h.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let p = format!("Meeting: {}.", t.trim_end_matches('.'));
        if p.len() < budget / 3 {
            budget -= p.len() + 1;
            parts.push(p);
        }
    }
    let mut seen: Vec<String> = Vec::new();
    let mut take = |label: &str, words: &[String], budget: &mut usize, parts: &mut Vec<String>| {
        let mut kept: Vec<&str> = Vec::new();
        let mut len = label.len() + 2;
        for w in words {
            let w = w.trim();
            if w.is_empty() || seen.iter().any(|s| s.eq_ignore_ascii_case(w)) {
                continue;
            }
            if len + w.len() + 2 > *budget {
                break;
            }
            len += w.len() + 2;
            seen.push(w.to_string());
            kept.push(w);
        }
        if !kept.is_empty() {
            *budget -= len;
            parts.push(format!("{label}: {}.", kept.join(", ")));
        }
    };
    take("Glossary", &h.vocabulary, &mut budget, &mut parts);
    take("People", &h.names, &mut budget, &mut parts);
    take("Terms", &h.terms, &mut budget, &mut parts);

    if !base.is_empty() {
        parts.push(base.to_string());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn normalize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whisper sometimes "hears" its own prompt in near-silence. True when the line is just a
/// piece of the prompt, at least three words long.
pub fn echoes_prompt(text: &str, prompt: &str) -> bool {
    let t = normalize(text);
    t.split(' ').count() >= 3 && normalize(prompt).contains(&t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_names_and_jargon_in_notes() {
        let notes = "- Demo of Playwright tests with Budi Santoso\n\
                     - Move the API to Node.js 22, try GPT-4o and GitHub Actions\n\
                     Next: ask Claude about the backend.\n\
                     The QA team reviews on Friday.";
        let t = terms_from(notes);
        for want in ["Playwright", "Budi Santoso", "API", "Node.js", "GPT-4o", "GitHub Actions", "Claude", "QA"] {
            assert!(t.iter().any(|x| x == want), "missing {want} in {t:?}");
        }
        for skip in ["Demo", "Move", "Next", "The", "Friday", "22"] {
            assert!(!t.iter().any(|x| x == skip), "kept {skip} in {t:?}");
        }
    }

    #[test]
    fn indonesian_sentence_starts_are_not_terms() {
        let t = terms_from("Kita deploy ke staging. Terus Andi cek dashboard Grafana.");
        assert_eq!(t, vec!["Andi", "Grafana"]);
    }

    #[test]
    fn splits_user_list() {
        assert_eq!(
            split_list("Playwright, Claude;\nbackend engineer, , claude"),
            vec!["Playwright", "Claude", "backend engineer"]
        );
    }

    #[test]
    fn prompt_has_hints_then_base() {
        let h = Hints {
            vocabulary: vec!["Playwright".into(), "backend engineer".into()],
            names: vec!["Budi Santoso".into(), "playwright".into()],
            title: Some("BE University talk".into()),
            terms: vec!["Grafana".into()],
        };
        let p = build_prompt(Some("Oke, kita mulai."), &h).unwrap();
        assert_eq!(
            p,
            "Meeting: BE University talk. Glossary: Playwright, backend engineer. People: Budi Santoso. Terms: Grafana. Oke, kita mulai."
        );
        assert_eq!(build_prompt(None, &Hints::default()), None);
        assert_eq!(build_prompt(Some("Base."), &Hints::default()).as_deref(), Some("Base."));
    }

    #[test]
    fn prompt_stays_within_whisper_limit() {
        let h = Hints { terms: (0..300).map(|i| format!("Term{i}")).collect(), ..Default::default() };
        let base = "Oke, kita mulai meeting-nya sekarang.";
        let p = build_prompt(Some(base), &h).unwrap();
        assert!(p.len() <= MAX_CHARS, "{} chars", p.len());
        assert!(p.ends_with(base));
    }

    #[test]
    fn spots_prompt_echo() {
        let p = "Glossary: Playwright, backend engineer. People: Budi Santoso.";
        assert!(echoes_prompt("Playwright, backend engineer.", p));
        assert!(!echoes_prompt("Playwright", p)); // a real one-word answer stays
        assert!(!echoes_prompt("we use Playwright for the backend", p));
    }
}
