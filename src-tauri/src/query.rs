//! The search query language.
//!
//! Turns what the user typed into `(Filters, free_text)`. Everything it
//! produces is a `Filters` the chip row could equally have produced, so
//! `type:code` and clicking the Code chip compile to identical SQL and there is
//! only one filtering path to reason about.
//!
//! ```text
//! type:code app:Code is:pinned folder:work "exact phrase" rest
//! ```
//!
//! | Operator | Effect |
//! |---|---|
//! | `type:` / `t:` | content type, or a chip alias (`link`, `text`, `image`, `code`, `cred`) |
//! | `app:` | source application |
//! | `folder:` / `f:` | folder by name |
//! | `is:pinned` `is:note` `is:clip` `is:credential` `is:unfiled` | flags |
//! | `sort:used` | rank by use count instead of recency |
//! | `"..."` | a phrase, kept together as free text |
//! | anything else | free text, passed to FTS |
//!
//! There is deliberately no `tag:`: `tags`/`item_tags` exist in the schema with
//! nothing behind them, and an operator that silently matches nothing is worse
//! than one that does not exist.
//!
//! Repeating an operator ORs within it and ANDs across it, which is what the
//! chips already do. An unknown operator is *not* an error: `http://x` must not
//! be read as an operator, and a typo should search rather than silently return
//! nothing.
//!
//! Pure and synchronous. Names are resolved to ids in SQL, not here, so this
//! stays a function of its input and nothing else.

use crate::db::model::Filters;

/// What `parse` produces: the filters, plus whatever was left for FTS.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedQuery {
    pub filters: Filters,
    /// The words with every operator removed. Empty means "no text search",
    /// which is the recent-first listing path.
    pub text: String,
}

/// Chip aliases, so the words in the UI work in the query box too. One chip can
/// stand for several content types, exactly as `TYPE_CHIPS` does on the
/// frontend -- these two lists mean the same thing and are edited together.
fn expand_type(word: &str) -> Vec<String> {
    let w = word.to_ascii_lowercase();
    let group = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect();
    match w.as_str() {
        "link" => group(&["url", "email"]),
        "text" => group(&["text", "json", "markdown"]),
        "code" => group(&["code", "path", "hex_color"]),
        "cred" | "credential" | "password" => group(&["credential"]),
        "image" | "img" => group(&["image"]),
        "note" => group(&["markdown"]),
        // Anything else is taken as a literal content_type. A value that matches
        // no row simply returns nothing, which is the honest answer to
        // `type:nonsense`.
        other => vec![other.to_string()],
    }
}

pub fn parse(input: &str) -> ParsedQuery {
    let mut out = ParsedQuery::default();
    let mut words: Vec<String> = Vec::new();

    for token in tokenize(input) {
        match token {
            Token::Phrase(p) => words.push(p),
            Token::Word(w) => {
                match split_operator(&w) {
                    Some((op, value)) if !value.is_empty() => {
                        if !apply(&mut out.filters, &op, value) {
                            words.push(w);
                        }
                    }
                    _ => words.push(w),
                }
            }
        }
    }

    out.text = words.join(" ").trim().to_string();
    out
}

/// Returns false when the operator is not one we know, so the caller can put
/// the word back into the free text rather than dropping it.
fn apply(filters: &mut Filters, op: &str, value: &str) -> bool {
    match op {
        "type" | "t" => filters.content_types.extend(expand_type(value)),
        "app" => filters.source_apps.push(value.to_string()),
        "folder" | "f" => filters.folder_names.push(value.to_string()),
        "sort" => match value.to_ascii_lowercase().as_str() {
            "used" | "uses" | "count" => filters.most_used = true,
            // `sort:recent` is the default; accepting it explicitly means a
            // saved search can state its intent.
            "recent" | "new" => filters.most_used = false,
            _ => return false,
        },
        "is" => match value.to_ascii_lowercase().as_str() {
            "pinned" => filters.pinned_only = true,
            "note" => filters.kinds.push("note".to_string()),
            "clip" => filters.kinds.push("clip".to_string()),
            "credential" | "cred" => filters.content_types.push("credential".to_string()),
            "unfiled" => filters.unfiled_only = true,
            _ => return false,
        },
        _ => return false,
    }
    true
}

/// `key:value`, with the colon not at either end. A bare `http://host` splits at
/// the first colon into `http` + `//host`, which `apply` then rejects as an
/// unknown operator and hands back as text -- which is why `apply` reports
/// success rather than just mutating.
fn split_operator(word: &str) -> Option<(String, &str)> {
    let idx = word.find(':')?;
    if idx == 0 || idx + 1 >= word.len() {
        return None;
    }
    Some((word[..idx].to_ascii_lowercase(), &word[idx + 1..]))
}

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Phrase(String),
}

/// Splits on whitespace, except inside double quotes. A quoted run becomes one
/// phrase; an unterminated quote runs to the end of the input rather than being
/// discarded, because the user is very likely still typing it.
fn tokenize(input: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_quotes = false;

    for ch in input.chars() {
        match ch {
            '"' => {
                if in_quotes {
                    if !buf.trim().is_empty() {
                        out.push(Token::Phrase(buf.trim().to_string()));
                    }
                    buf.clear();
                    in_quotes = false;
                } else {
                    if !buf.trim().is_empty() {
                        out.push(Token::Word(buf.trim().to_string()));
                    }
                    buf.clear();
                    in_quotes = true;
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                if !buf.trim().is_empty() {
                    out.push(Token::Word(buf.trim().to_string()));
                }
                buf.clear();
            }
            c => buf.push(c),
        }
    }

    if !buf.trim().is_empty() {
        let t = buf.trim().to_string();
        out.push(if in_quotes { Token::Phrase(t) } else { Token::Word(t) });
    }
    out
}

/// Merges typed filters with the ones the chip row already set. Chips and text
/// both narrow, so every list is concatenated and every flag is OR-ed.
pub fn merge(chips: &Filters, typed: &Filters) -> Filters {
    let cat = |a: &[String], b: &[String]| -> Vec<String> {
        let mut v = a.to_vec();
        v.extend_from_slice(b);
        v
    };

    Filters {
        kinds: cat(&chips.kinds, &typed.kinds),
        content_types: cat(&chips.content_types, &typed.content_types),
        source_apps: cat(&chips.source_apps, &typed.source_apps),
        pinned_only: chips.pinned_only || typed.pinned_only,
        folder_ids: cat(&chips.folder_ids, &typed.folder_ids),
        unfiled_only: chips.unfiled_only || typed.unfiled_only,
        folder_names: cat(&chips.folder_names, &typed.folder_names),
        most_used: chips.most_used || typed.most_used,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_left_alone() {
        let p = parse("hello world");
        assert_eq!(p.text, "hello world");
        assert!(p.filters.content_types.is_empty());
    }

    #[test]
    fn operators_are_lifted_out_of_the_text() {
        let p = parse("type:code app:Code deploy script");
        assert_eq!(p.text, "deploy script");
        assert_eq!(p.filters.content_types, ["code", "path", "hex_color"]);
        assert_eq!(p.filters.source_apps, ["Code"]);
    }

    #[test]
    fn flags() {
        let p = parse("is:pinned is:note is:unfiled sort:used");
        assert!(p.filters.pinned_only);
        assert!(p.filters.unfiled_only);
        assert!(p.filters.most_used);
        assert_eq!(p.filters.kinds, ["note"]);
        assert_eq!(p.text, "");
    }

    /// `tag:` is not an operator, so it must search rather than silently match
    /// nothing. This is the guard against re-adding it without the storage.
    #[test]
    fn tag_is_not_an_operator_yet() {
        let p = parse("tag:work");
        assert_eq!(p.text, "tag:work");
    }

    /// The case that matters most: a pasted URL must not be eaten as an
    /// operator, because searching your history for a link is the common case.
    #[test]
    fn a_url_is_text_not_an_operator() {
        let p = parse("https://example.com/x");
        assert_eq!(p.text, "https://example.com/x");
        assert!(p.filters.source_apps.is_empty());
        assert!(p.filters.content_types.is_empty());
    }

    #[test]
    fn unknown_operators_fall_back_to_text() {
        let p = parse("wat:huh is:nonsense");
        assert_eq!(p.text, "wat:huh is:nonsense");
        assert!(!p.filters.pinned_only);
    }

    #[test]
    fn quoted_phrases_survive_whole() {
        let p = parse("\"exact phrase here\" type:text");
        assert_eq!(p.text, "exact phrase here");
        assert_eq!(p.filters.content_types, ["text", "json", "markdown"]);
    }

    #[test]
    fn unterminated_quote_still_searches() {
        let p = parse("\"half typed");
        assert_eq!(p.text, "half typed");
    }

    #[test]
    fn repeated_operators_accumulate() {
        let p = parse("app:Code app:chrome folder:work folder:home");
        assert_eq!(p.filters.source_apps, ["Code", "chrome"]);
        assert_eq!(p.filters.folder_names, ["work", "home"]);
    }

    #[test]
    fn empty_and_whitespace() {
        assert_eq!(parse("").text, "");
        assert_eq!(parse("   ").text, "");
        // A dangling operator is what you have mid-typing; it must not filter
        // everything away before you finish the word.
        assert_eq!(parse("type:").text, "type:");
    }

    #[test]
    fn merge_combines_chips_and_text() {
        let chips = Filters { pinned_only: true, source_apps: vec!["a".into()], ..Default::default() };
        let typed = parse("app:b is:unfiled").filters;
        let m = merge(&chips, &typed);
        assert_eq!(m.source_apps, ["a", "b"]);
        assert!(m.pinned_only && m.unfiled_only);
    }
}
