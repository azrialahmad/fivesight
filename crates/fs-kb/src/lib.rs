//! fs-kb: turn the user's own files (Markdown, text, PDF) into searchable passages.
//! Storage and search live in fs-store; this crate only reads and splits.

use std::path::Path;

/// Larger files are almost always exports or logs that would drown out everything else.
pub const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_CHUNKS: usize = 4000;
/// About a page of prose: big enough to carry context, small enough that several fit in a prompt.
const TARGET_CHARS: usize = 1200;

#[derive(Debug, thiserror::Error)]
pub enum KbError {
    #[error("can't read {0}: {1}")]
    Io(String, std::io::Error),
    #[error("{0} isn't a supported file (use .md, .txt or .pdf)")]
    Unsupported(String),
    #[error("{0} is larger than 25 MB")]
    TooLarge(String),
    #[error("couldn't read text from the PDF {0}. Scanned PDFs without a text layer aren't supported yet")]
    Pdf(String),
    #[error("{0} has no text in it")]
    Empty(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Markdown,
    Text,
    Pdf,
}

impl Kind {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "md" | "markdown" | "mdx" => Some(Kind::Markdown),
            "txt" | "text" | "rst" | "org" | "csv" | "log" => Some(Kind::Text),
            "pdf" => Some(Kind::Pdf),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Markdown => "md",
            Kind::Text => "txt",
            Kind::Pdf => "pdf",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Extracted {
    pub title: String,
    pub kind: Kind,
    pub text: String,
}

/// Read a file into plain text with a title (the first Markdown heading, or the file name).
pub fn extract(path: &Path) -> Result<Extracted, KbError> {
    let shown = path.display().to_string();
    let kind = Kind::from_path(path).ok_or_else(|| KbError::Unsupported(shown.clone()))?;
    let meta = std::fs::metadata(path).map_err(|e| KbError::Io(shown.clone(), e))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(KbError::TooLarge(shown));
    }
    let text = match kind {
        Kind::Pdf => {
            let p = path.to_path_buf();
            // The PDF parser can panic on malformed files; a bad file must not take the app down.
            let out = std::panic::catch_unwind(move || pdf_extract::extract_text(&p));
            match out {
                Ok(Ok(t)) => t,
                _ => return Err(KbError::Pdf(shown)),
            }
        }
        _ => {
            let bytes = std::fs::read(path).map_err(|e| KbError::Io(shown.clone(), e))?;
            decode(&bytes)
        }
    };
    let text = normalize(&text);
    if text.trim().is_empty() {
        return Err(KbError::Empty(shown));
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled").to_string();
    let title = match kind {
        Kind::Markdown => first_heading(&text).unwrap_or(stem),
        _ => stem,
    };
    Ok(Extracted { title, kind, text })
}

/// UTF-8 (with or without BOM) or UTF-16 as Notepad saves it; anything else lossily.
fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let utf16 = |le: bool, b: &[u8]| {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(true, rest);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(false, rest);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn normalize(t: &str) -> String {
    t.replace("\r\n", "\n").replace('\r', "\n").replace('\u{0}', "")
}

fn first_heading(md: &str) -> Option<String> {
    md.lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("# ").map(|h| h.trim().to_string()))
        .filter(|h| !h.is_empty())
}

fn heading(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes).then(|| t[hashes..].trim()).filter(|h| !h.is_empty())
}

/// Split text into passages of about `TARGET_CHARS`, on paragraph boundaries where possible.
/// Each passage starts with the nearest Markdown heading above it, so a hit carries its context.
pub fn chunk(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut section: Option<String> = None;
    let mut current_section: Option<String> = None;

    let flush = |current: &mut String, current_section: &Option<String>, out: &mut Vec<String>| {
        let body = current.trim();
        if !body.is_empty() {
            match current_section {
                Some(h) if !body.starts_with('#') => out.push(format!("{h}\n{body}")),
                _ => out.push(body.to_string()),
            }
        }
        current.clear();
    };

    for para in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        if let Some(h) = para.lines().next().and_then(heading) {
            // A new section starts a new passage so passages don't straddle topics.
            flush(&mut current, &current_section, &mut out);
            section = Some(h.to_string());
        }
        if current.is_empty() {
            current_section = section.clone();
        }
        if current.chars().count() + para.chars().count() > TARGET_CHARS && !current.is_empty() {
            flush(&mut current, &current_section, &mut out);
            current_section = section.clone();
        }
        if para.chars().count() > TARGET_CHARS {
            for piece in split_long(para) {
                current.push_str(&piece);
                flush(&mut current, &current_section, &mut out);
            }
            continue;
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(para);
        if out.len() >= MAX_CHUNKS {
            break;
        }
    }
    flush(&mut current, &current_section, &mut out);
    out.truncate(MAX_CHUNKS);
    out
}

/// Cut an over-long paragraph (PDFs often have no blank lines) at sentence or word breaks.
fn split_long(p: &str) -> Vec<String> {
    let chars: Vec<char> = p.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let hard_end = (start + TARGET_CHARS).min(chars.len());
        let mut end = hard_end;
        if hard_end < chars.len() {
            let window = &chars[start..hard_end];
            let min = TARGET_CHARS / 2;
            if let Some(i) = window.iter().rposition(|c| matches!(c, '.' | '!' | '?' | '\n')).filter(|i| *i >= min) {
                end = start + i + 1;
            } else if let Some(i) = window.iter().rposition(|c| c.is_whitespace()).filter(|i| *i >= min) {
                end = start + i + 1;
            }
        }
        let piece: String = chars[start..end].iter().collect();
        if !piece.trim().is_empty() {
            out.push(piece.trim().to_string());
        }
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fs-kb-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::File::create(&p).unwrap().write_all(bytes).unwrap();
        p
    }

    #[test]
    fn markdown_title_comes_from_the_first_heading() {
        let p = tmp("runbook.md", "Intro line\n\n# Payments runbook\n\nRetry waits 30 s.".as_bytes());
        let e = extract(&p).unwrap();
        assert_eq!(e.title, "Payments runbook");
        assert_eq!(e.kind, Kind::Markdown);
    }

    #[test]
    fn text_title_is_the_file_name_and_encodings_decode() {
        let p = tmp("notes.txt", b"\xEF\xBB\xBFhalo dunia");
        let e = extract(&p).unwrap();
        assert_eq!(e.title, "notes");
        assert_eq!(e.text, "halo dunia");
        // UTF-16 LE with BOM, as Windows Notepad can save.
        let mut b = vec![0xFF, 0xFE];
        for u in "rapat".encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(extract(&tmp("u16.txt", &b)).unwrap().text, "rapat");
    }

    #[test]
    fn rejects_unsupported_empty_and_broken_files() {
        assert!(matches!(extract(&tmp("a.exe", b"MZ")), Err(KbError::Unsupported(_))));
        assert!(matches!(extract(&tmp("blank.md", b"  \n\n ")), Err(KbError::Empty(_))));
        assert!(matches!(extract(&tmp("broken.pdf", b"not a pdf at all")), Err(KbError::Pdf(_))));
        assert!(matches!(extract(Path::new("C:/definitely/missing.md")), Err(KbError::Io(..))));
    }

    #[test]
    fn chunks_keep_their_section_heading() {
        let md = "# Payments\n\nRetry waits 30 s.\n\n## Escalation\n\nPage the on-call.\n\nThen post in #incidents.";
        let c = chunk(md);
        assert_eq!(c.len(), 2, "{c:?}");
        assert!(c[0].starts_with("# Payments") && c[0].contains("Retry waits"));
        assert!(c[1].starts_with("## Escalation") && c[1].contains("Page the on-call") && c[1].contains("#incidents"));
    }

    #[test]
    fn long_text_is_split_near_the_target_at_sentence_breaks() {
        let sentence = "Deploy ke staging dulu sebelum production. ";
        let text = sentence.repeat(120); // one huge paragraph, like PDF output
        let c = chunk(&text);
        assert!(c.len() >= 4, "{}", c.len());
        for piece in &c {
            assert!(piece.chars().count() <= TARGET_CHARS, "{}", piece.len());
            assert!(piece.ends_with('.'), "cut mid-sentence: {piece:?}");
        }
        let total: usize = c.iter().map(|p| p.matches("Deploy").count()).sum();
        assert_eq!(total, 120, "no text may be lost");
    }

    #[test]
    fn paragraphs_are_packed_together_until_the_target() {
        let para = "word ".repeat(50); // ~250 chars
        let text = vec![para.trim(); 10].join("\n\n");
        let c = chunk(&text);
        assert!(c.len() >= 2 && c.len() <= 3, "{}", c.len());
        assert!(c.iter().all(|p| p.chars().count() <= TARGET_CHARS));
    }

    /// A minimal one-page PDF with a text layer, xref offsets computed so parsers accept it.
    fn tiny_pdf(text: &str) -> Vec<u8> {
        let stream = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes(),
        );
        out
    }

    #[test]
    fn reads_text_from_a_pdf() {
        let p = tmp("spec.pdf", &tiny_pdf("Checkout retry window is thirty seconds"));
        let e = extract(&p).unwrap();
        assert_eq!(e.kind, Kind::Pdf);
        assert_eq!(e.title, "spec");
        assert!(e.text.contains("thirty seconds"), "{:?}", e.text);
    }

    #[test]
    fn handles_non_ascii_without_panicking() {
        let text = "Rapat ✓ — résumé 日本語 ".repeat(200);
        let c = chunk(&text);
        assert!(!c.is_empty());
    }
}
