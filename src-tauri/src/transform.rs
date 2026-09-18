//! Text transforms applied on the way to the clipboard.
//!
//! Pure functions over a `&str`, like `detect.rs`, so they are unit-testable
//! without a database, a clipboard or a window. The stored item is never
//! modified: a transform changes what lands on the clipboard for one paste, and
//! the history keeps what was actually copied.
//!
//! `applicable_to` is what keeps the menu short. A JSON clip offers Prettify and
//! Minify; a base64-looking blob offers Decode. Everything offers the handful
//! that always make sense. The classifier has already labelled the item, so this
//! is a match on that label plus a cheap look at the content.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transform {
    /// Collapse all runs of whitespace and trim. The "paste as plain text" of a
    /// world where we only have plain text to begin with.
    Trim,
    JoinLines,
    UpperCase,
    LowerCase,
    TitleCase,
    SnakeCase,
    KebabCase,
    JsonPretty,
    JsonMinify,
    Base64Encode,
    Base64Decode,
    UrlEncode,
    UrlDecode,
}

impl Transform {
    /// The label the menu shows. Kept here rather than in the frontend so the
    /// list of transforms and their names cannot drift apart.
    pub fn label(self) -> &'static str {
        match self {
            Transform::Trim => "Trim whitespace",
            Transform::JoinLines => "Join lines",
            Transform::UpperCase => "UPPER CASE",
            Transform::LowerCase => "lower case",
            Transform::TitleCase => "Title Case",
            Transform::SnakeCase => "snake_case",
            Transform::KebabCase => "kebab-case",
            Transform::JsonPretty => "Prettify JSON",
            Transform::JsonMinify => "Minify JSON",
            Transform::Base64Encode => "Base64 encode",
            Transform::Base64Decode => "Base64 decode",
            Transform::UrlEncode => "URL encode",
            Transform::UrlDecode => "URL decode",
        }
    }

    /// Always worth offering, whatever the content is.
    const ALWAYS: &'static [Transform] = &[
        Transform::Trim,
        Transform::JoinLines,
        Transform::UpperCase,
        Transform::LowerCase,
        Transform::TitleCase,
        Transform::SnakeCase,
        Transform::KebabCase,
        Transform::Base64Encode,
        Transform::UrlEncode,
    ];
}

/// Which transforms are worth showing for this content.
///
/// The conditional ones are checked by trying cheaply rather than by guessing
/// from the content type alone: a base64 string classifies as plain `text`, and
/// JSON that failed to classify is still JSON if it parses.
pub fn applicable_to(content: &str) -> Vec<Transform> {
    let mut out: Vec<Transform> = Vec::new();

    if looks_like_json(content) {
        out.push(Transform::JsonPretty);
        out.push(Transform::JsonMinify);
    }
    if looks_like_base64(content) {
        out.push(Transform::Base64Decode);
    }
    if content.contains('%') && apply(Transform::UrlDecode, content).is_ok_and(|d| d != content) {
        out.push(Transform::UrlDecode);
    }

    out.extend_from_slice(Transform::ALWAYS);
    out
}

pub fn apply(t: Transform, input: &str) -> Result<String, String> {
    match t {
        Transform::Trim => Ok(input.split_whitespace().collect::<Vec<_>>().join(" ")),
        Transform::JoinLines => Ok(input
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ")),
        Transform::UpperCase => Ok(input.to_uppercase()),
        Transform::LowerCase => Ok(input.to_lowercase()),
        Transform::TitleCase => Ok(title_case(input)),
        Transform::SnakeCase => Ok(delimit(input, '_')),
        Transform::KebabCase => Ok(delimit(input, '-')),
        Transform::JsonPretty => reformat_json(input, true),
        Transform::JsonMinify => reformat_json(input, false),
        Transform::Base64Encode => Ok(b64_encode(input.as_bytes())),
        Transform::Base64Decode => {
            let bytes = b64_decode(input.trim())?;
            String::from_utf8(bytes).map_err(|_| "decoded bytes are not valid UTF-8".to_string())
        }
        Transform::UrlEncode => Ok(url_encode(input)),
        Transform::UrlDecode => url_decode(input),
    }
}

fn looks_like_json(s: &str) -> bool {
    let t = s.trim();
    (t.starts_with('{') || t.starts_with('[')) && serde_json::from_str::<serde_json::Value>(t).is_ok()
}

/// Conservative on purpose. Any lowercase word is "valid base64" if its length
/// happens to fit, so a bare `Decode` on ordinary prose would produce mojibake
/// and look like a bug. Requiring length, a plausible alphabet, and a decode
/// that yields valid UTF-8 keeps the menu entry honest.
fn looks_like_base64(s: &str) -> bool {
    let t = s.trim();
    if t.len() < 8 || t.len() % 4 != 0 || t.contains(char::is_whitespace) {
        return false;
    }
    if !t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=') {
        return false;
    }
    // Prose is all letters; base64 of anything real almost never is.
    if t.bytes().all(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    b64_decode(t).is_ok_and(|bytes| String::from_utf8(bytes).is_ok())
}

fn reformat_json(input: &str, pretty: bool) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(input.trim()).map_err(|e| format!("not valid JSON: {e}"))?;
    let out = if pretty {
        serde_json::to_string_pretty(&value)
    } else {
        serde_json::to_string(&value)
    };
    out.map_err(|e| format!("could not re-encode JSON: {e}"))
}

fn title_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut new_word = true;
    for ch in input.chars() {
        if ch.is_alphanumeric() {
            if new_word {
                out.extend(ch.to_uppercase());
            } else {
                out.extend(ch.to_lowercase());
            }
            new_word = false;
        } else {
            out.push(ch);
            new_word = true;
        }
    }
    out
}

/// Splits on non-alphanumerics *and* on camelCase humps, so `parseHTTPResponse`
/// becomes `parse_http_response` rather than one long word.
///
/// Two different humps have to be recognised. `myVar` splits where a lowercase
/// meets an uppercase; `HTTPResponse` splits where a *run* of uppercase ends,
/// which is only detectable by looking one character ahead for a lowercase.
/// Handling just the first is what produces `parse_httpresponse`.
fn delimit(input: &str, sep: char) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();

    for (i, &ch) in chars.iter().enumerate() {
        if !ch.is_alphanumeric() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            continue;
        }

        // `cur` being non-empty guarantees i >= 1.
        if !cur.is_empty() && ch.is_uppercase() {
            let prev = chars[i - 1];
            let next_is_lower = chars.get(i + 1).is_some_and(|c| c.is_lowercase());
            if prev.is_lowercase() || prev.is_numeric() || (prev.is_uppercase() && next_is_lower) {
                words.push(std::mem::take(&mut cur));
            }
        }

        cur.extend(ch.to_lowercase());
    }

    if !cur.is_empty() {
        words.push(cur);
    }
    words.join(&sep.to_string())
}

/* -- base64 and percent-encoding -------------------------------------------
 *
 * Hand-rolled rather than pulled in as crates. Both are a few lines, this
 * project already hand-rolls its DIB decoding and platform calls, and a
 * clipboard manager that promises "no dependency that opens a socket" is better
 * off with a smaller dependency tree, not a larger one.
 */

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    let val = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };

    let body: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if body.len() % 4 != 0 {
        return Err("not valid base64 (length)".to_string());
    }

    let mut out = Vec::with_capacity(body.len() / 4 * 3);
    for chunk in body.chunks(4) {
        let pad = chunk.iter().filter(|&&c| c == b'=').count();
        let mut n: u32 = 0;
        for &c in chunk {
            n = (n << 6) | if c == b'=' { 0 } else { val(c).ok_or("not valid base64")? };
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

/// RFC 3986 unreserved set stays as-is; everything else is percent-encoded from
/// its UTF-8 bytes.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn url_decode(s: &str) -> Result<String, String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("truncated percent-escape".to_string());
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .map_err(|_| "bad percent-escape".to_string())?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| "bad percent-escape".to_string())?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "decoded bytes are not valid UTF-8".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(tr: Transform, input: &str) -> String {
        apply(tr, input).expect("transform should succeed")
    }

    #[test]
    fn whitespace() {
        assert_eq!(t(Transform::Trim, "  a   b \n c  "), "a b c");
        assert_eq!(t(Transform::JoinLines, "one\n\n  two  \nthree"), "one two three");
    }

    #[test]
    fn cases() {
        assert_eq!(t(Transform::UpperCase, "aB c"), "AB C");
        assert_eq!(t(Transform::LowerCase, "aB C"), "ab c");
        assert_eq!(t(Transform::TitleCase, "hello wORLD here"), "Hello World Here");
        assert_eq!(t(Transform::SnakeCase, "Hello World"), "hello_world");
        assert_eq!(t(Transform::KebabCase, "Hello World"), "hello-world");
    }

    /// camelCase must split at the humps, or `snake_case` produces one long
    /// unusable word -- the exact case someone converting an identifier hits.
    #[test]
    fn case_conversion_splits_camel_humps() {
        assert_eq!(t(Transform::SnakeCase, "parseHTTPResponse"), "parse_http_response");
        assert_eq!(t(Transform::KebabCase, "myVariableName"), "my-variable-name");
        assert_eq!(t(Transform::SnakeCase, "already_snake"), "already_snake");
    }

    /// Key order must survive. serde_json sorts keys unless `preserve_order` is
    /// on, and silently reordering a config someone is about to paste back is an
    /// edit they did not ask for.
    #[test]
    fn json_round_trip_preserves_key_order() {
        let min = t(Transform::JsonMinify, "{ \"b\" : 1, \"a\" : [1, 2] }");
        assert_eq!(min, r#"{"b":1,"a":[1,2]}"#);
        assert!(t(Transform::JsonPretty, &min).contains("\n  "));
        assert!(apply(Transform::JsonPretty, "not json").is_err());
    }

    #[test]
    fn base64_round_trip() {
        for s in ["", "a", "ab", "abc", "hello world", "pä$$ wörd 🔑"] {
            let enc = t(Transform::Base64Encode, s);
            assert_eq!(t(Transform::Base64Decode, &enc), s, "round trip for {s:?}");
        }
        assert_eq!(t(Transform::Base64Encode, "hello world"), "aGVsbG8gd29ybGQ=");
        assert!(apply(Transform::Base64Decode, "!!!!").is_err());
    }

    #[test]
    fn url_round_trip() {
        let s = "a b&c=d/ä?";
        let enc = t(Transform::UrlEncode, s);
        assert!(!enc.contains(' '));
        assert_eq!(t(Transform::UrlDecode, &enc), s);
        assert_eq!(t(Transform::UrlDecode, "a%20b"), "a b");
        assert!(apply(Transform::UrlDecode, "%2").is_err());
    }

    #[test]
    fn menu_offers_json_only_for_json() {
        let json = applicable_to(r#"{"a":1}"#);
        assert!(json.contains(&Transform::JsonPretty));
        let prose = applicable_to("just some words here");
        assert!(!prose.contains(&Transform::JsonPretty));
    }

    /// The menu must not offer Decode on ordinary prose that happens to be the
    /// right length -- the result would be mojibake and read as a bug.
    #[test]
    fn menu_offers_base64_decode_conservatively() {
        assert!(applicable_to("aGVsbG8gd29ybGQ=").contains(&Transform::Base64Decode));
        assert!(!applicable_to("hello there friend").contains(&Transform::Base64Decode));
        assert!(!applicable_to("abcdefgh").contains(&Transform::Base64Decode));
        assert!(!applicable_to("short").contains(&Transform::Base64Decode));
    }

    #[test]
    fn menu_offers_url_decode_only_when_it_changes_something() {
        assert!(applicable_to("a%20b").contains(&Transform::UrlDecode));
        assert!(!applicable_to("plain text").contains(&Transform::UrlDecode));
    }

    /// Every transform must survive arbitrary input without panicking; the
    /// content comes from the clipboard and is entirely untrusted.
    #[test]
    fn nothing_panics_on_awkward_input() {
        let long = "a".repeat(10_000);
        let inputs = ["", "   ", "\n\n", "🔑", "%", "=", "{", long.as_str()];
        for input in inputs {
            for tr in applicable_to(input) {
                let _ = apply(tr, input);
            }
        }
    }
}
