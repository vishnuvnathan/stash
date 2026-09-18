//! Content-type classification. Pure, allocation-light, and run exactly once
//! per inserted row -- never on render and never on an unchanged clipboard.

use crate::db::model::ContentType;
use once_cell::sync::Lazy;
use regex::Regex;

static RE_URL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^(?:https?|ftp|file)://\S+$|^www\.\S+\.\S+$").unwrap());

static RE_EMAIL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,63}$").unwrap()
});

static RE_HEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^#?(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$").unwrap());

static RE_WIN_PATH: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(?:[A-Za-z]:[\\/]|\\\\[^\\/]+[\\/])").unwrap());

static RE_POSIX_PATH: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?:~|\.{1,2})?/[^\s]+$").unwrap());

static RE_CODE_KEYWORD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?m)\b(?:fn|def|class|struct|impl|import|from|const|let|var|function|return|public|private|static|#include|package|interface|async|await|export)\b",
    )
    .unwrap()
});

static RE_CODE_OPERATOR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:=>|->|::|!==|===|\|\||&&)").unwrap());

/// Longest string we bother classifying in full. Past this we look at the head
/// only: a 2MB paste should not cost a full-buffer regex sweep.
const SCAN_LIMIT: usize = 8 * 1024;

pub fn classify(raw: &str) -> ContentType {
    let scan = if raw.len() > SCAN_LIMIT {
        // Truncate on a char boundary so the slice stays valid UTF-8.
        let mut end = SCAN_LIMIT;
        while end > 0 && !raw.is_char_boundary(end) {
            end -= 1;
        }
        &raw[..end]
    } else {
        raw
    };

    let trimmed = scan.trim();
    if trimmed.is_empty() {
        return ContentType::Text;
    }

    let single_line = !trimmed.contains('\n');

    if single_line {
        if RE_URL.is_match(trimmed) {
            return ContentType::Url;
        }
        if RE_EMAIL.is_match(trimmed) {
            return ContentType::Email;
        }
        if RE_HEX.is_match(trimmed) {
            return ContentType::HexColor;
        }
        if is_path(trimmed) {
            return ContentType::Path;
        }
    }

    if looks_like_json(trimmed) {
        return ContentType::Json;
    }

    if code_score(trimmed) >= 3 {
        return ContentType::Code;
    }

    ContentType::Text
}

fn is_path(s: &str) -> bool {
    if s.contains(' ') && !s.contains('\\') && !s.contains('/') {
        return false;
    }
    if RE_WIN_PATH.is_match(s) {
        return true;
    }
    // A POSIX path needs at least one separator beyond the leading one, so bare
    // "/" and "~" are not treated as paths.
    RE_POSIX_PATH.is_match(s) && s.matches('/').count() >= 1 && s.len() > 2
}

fn looks_like_json(s: &str) -> bool {
    let first = s.as_bytes().first().copied();
    if !matches!(first, Some(b'{') | Some(b'[')) {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(s).is_ok()
}

/// Heuristic score; 3 or more reads as source code. Deliberately conservative
/// so prose with the odd brace stays `text`.
fn code_score(s: &str) -> u32 {
    let mut score = 0;

    if s.contains("```") {
        score += 3;
    }

    let lines: Vec<&str> = s.lines().collect();

    let indented = lines
        .iter()
        .filter(|l| l.starts_with("  ") || l.starts_with('\t'))
        .count();
    if indented >= 2 {
        score += 1;
    }

    let terminated = lines
        .iter()
        .filter(|l| {
            let t = l.trim_end();
            t.ends_with(';') || t.ends_with('{') || t.ends_with('}')
        })
        .count();
    if terminated >= 2 {
        score += 2;
    } else if terminated == 1 {
        score += 1;
    }

    if RE_CODE_KEYWORD.is_match(s) {
        score += 1;
    }
    if RE_CODE_OPERATOR.is_match(s) {
        score += 1;
    }

    score
}

/// First non-empty line, clipped, used as the row title in the list.
pub fn derive_title(content: &str, max: usize) -> String {
    let line = content.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let clipped: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{clipped}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(classify("https://example.com/a?b=1"), ContentType::Url);
        assert_eq!(classify("www.example.com"), ContentType::Url);
        assert_eq!(classify("visit https://example.com now"), ContentType::Text);
    }

    #[test]
    fn emails_and_colors() {
        assert_eq!(classify("a.b+c@example.co.uk"), ContentType::Email);
        assert_eq!(classify("#7aa2f7"), ContentType::HexColor);
        assert_eq!(classify("7aa2f7"), ContentType::HexColor);
        assert_eq!(classify("#abc"), ContentType::HexColor);
    }

    #[test]
    fn paths() {
        assert_eq!(classify(r"C:\Users\Dell\file.txt"), ContentType::Path);
        assert_eq!(classify("/usr/local/bin/stash"), ContentType::Path);
        assert_eq!(classify("~/notes/todo.md"), ContentType::Path);
        assert_eq!(classify("/"), ContentType::Text);
    }

    #[test]
    fn json_must_parse() {
        assert_eq!(classify(r#"{"a":1,"b":[2,3]}"#), ContentType::Json);
        // Starts like JSON but does not parse -- must not be misfiled.
        assert_eq!(classify("{not json at all"), ContentType::Text);
    }

    #[test]
    fn code_versus_prose() {
        let code = "fn main() {\n    let x = 1;\n    println!(\"{}\", x);\n}";
        assert_eq!(classify(code), ContentType::Code);

        let prose = "We met at the office. It went well, all things considered.";
        assert_eq!(classify(prose), ContentType::Text);
    }

    #[test]
    fn oversized_input_is_boundary_safe() {
        let big = "\u{00e9}".repeat(SCAN_LIMIT);
        let _ = classify(&big);
    }

    #[test]
    fn titles_clip() {
        assert_eq!(derive_title("\n\nhello world\nmore", 40), "hello world");
        assert_eq!(derive_title("abcdef", 3), "ab\u{2026}");
    }
}
