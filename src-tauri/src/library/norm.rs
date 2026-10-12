//! Text normalization: Unicode form, sort keys, and safe filenames.
//!
//! The Unicode part is not theoretical. This collection was partly assembled on
//! macOS, whose filesystem hands back decomposed (NFD) filenames, while the tags
//! inside those same files are precomposed (NFC). The folder `Björk` and the tag
//! `Björk` are therefore different byte strings, and anything that compares them
//! naively ends up with two Björks in the grid and two Björk directories on B2.
//! Every path and every display string gets forced to NFC on the way in.

use unicode_normalization::{is_nfc_quick, IsNormalized, UnicodeNormalization};

/// Force a string to Unicode NFC.
pub fn nfc(s: &str) -> String {
    // The quick check is a cheap scan that lets the common all-ASCII case skip
    // allocation entirely — this runs over every path and tag of 2000+ files.
    match is_nfc_quick(s.chars()) {
        IsNormalized::Yes => s.to_string(),
        _ => s.nfc().collect(),
    }
}

/// True when two strings are the same text under NFC, regardless of how the
/// filesystem or tagger happened to encode them.
pub fn eq_nfc(a: &str, b: &str) -> bool {
    nfc(a) == nfc(b)
}

/// Sort/search key: NFC, lowercased, with a leading article dropped.
///
/// "The Strokes" files under S. The collection contains both `The Beatles` and
/// `the Mountain Goats`, so the article match has to be case-insensitive. Only
/// "the" is stripped — that is the rule Henry asked for, and widening it to
/// "a"/"an" would move `A Silver Mt. Zion` under S, which he did not ask for.
pub fn sort_key(s: &str) -> String {
    let lowered = nfc(s).to_lowercase();
    let trimmed = lowered.trim();
    match trimmed.strip_prefix("the ") {
        Some(rest) => rest.trim_start().to_string(),
        None => trimmed.to_string(),
    }
}

/// Characters that are illegal or hostile in a path component on at least one of
/// Linux, macOS, Windows, or B2.
const UNSAFE: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Turn arbitrary tag text into a path component that is valid everywhere.
///
/// `:` becomes `_` rather than being dropped, matching the convention already
/// present in this collection (`Late Night Tales_ Metronomy`, `New History
/// Warfare Vol.2_ Judges`) so renames don't churn names that are already fine.
pub fn sanitize_filename(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in nfc(s).chars() {
        if ch == ':' {
            out.push('_');
        } else if UNSAFE.contains(&ch) || (ch as u32) < 0x20 {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }

    // Collapse runs of whitespace introduced by the replacements above.
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");

    // Trailing dots and spaces are silently dropped by some filesystems, which
    // would make a rename appear to succeed and then not match on the next scan.
    let trimmed = collapsed.trim_end_matches(['.', ' ']).trim_start();

    // Leave room inside the common 255-byte component limit. Truncate on a char
    // boundary, not a byte one.
    let mut result = String::new();
    for ch in trimmed.chars() {
        if result.len() + ch.len_utf8() > 200 {
            break;
        }
        result.push(ch);
    }

    let result = result.trim_end_matches(['.', ' ']).to_string();
    if result.is_empty() {
        "Untitled".to_string()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfc_collapses_the_two_bjorks() {
        // Exactly the split present in the collection: decomposed folder name
        // vs precomposed tag.
        let decomposed = "Bjo\u{0308}rk";
        let precomposed = "Bj\u{00f6}rk";
        assert_ne!(decomposed, precomposed, "test is meaningless if these match");
        assert_eq!(nfc(decomposed), nfc(precomposed));
        assert!(eq_nfc(decomposed, precomposed));
        assert_eq!(sort_key(decomposed), sort_key(precomposed));
    }

    #[test]
    fn article_stripping_is_case_insensitive() {
        // Both spellings are present in the collection.
        assert_eq!(sort_key("The Beatles"), "beatles");
        assert_eq!(sort_key("the Mountain Goats"), "mountain goats");
        assert_eq!(sort_key("The Strokes"), "strokes");
    }

    #[test]
    fn article_stripping_only_matches_a_whole_word() {
        // "Theatre" must not become "atre".
        assert_eq!(sort_key("Theatre of Hate"), "theatre of hate");
        assert_eq!(sort_key("Them"), "them");
    }

    #[test]
    fn only_the_is_stripped() {
        assert_eq!(sort_key("A Silver Mt. Zion"), "a silver mt. zion");
        assert_eq!(sort_key("An Horse"), "an horse");
    }

    #[test]
    fn sanitize_matches_existing_collection_convention() {
        // These are the real names on disk; sanitizing them must be a no-op so
        // renames don't churn folders that are already correct.
        assert_eq!(
            sanitize_filename("Late Night Tales_ Metronomy"),
            "Late Night Tales_ Metronomy"
        );
        // And the tag form converts to that same convention.
        assert_eq!(
            sanitize_filename("Late Night Tales: Metronomy"),
            "Late Night Tales_ Metronomy"
        );
    }

    #[test]
    fn sanitize_rejects_path_separators() {
        assert_eq!(sanitize_filename("AC/DC"), "AC DC");
        assert!(!sanitize_filename("../../etc/passwd").contains('/'));
    }

    #[test]
    fn sanitize_strips_trailing_dots_and_spaces() {
        assert_eq!(sanitize_filename("Album.  "), "Album");
        assert_eq!(sanitize_filename("  Album"), "Album");
    }

    #[test]
    fn sanitize_preserves_non_latin_names() {
        // The collection has 佐井好子, ランプ幻想, Árstíðir, F♯A♯∞.
        assert_eq!(sanitize_filename("ランプ幻想"), "ランプ幻想");
        assert_eq!(sanitize_filename("F♯A♯∞ (CD Release)"), "F♯A♯∞ (CD Release)");
    }

    #[test]
    fn sanitize_never_returns_empty() {
        assert_eq!(sanitize_filename("///"), "Untitled");
        assert_eq!(sanitize_filename(""), "Untitled");
    }

    #[test]
    fn sanitize_truncates_on_a_char_boundary() {
        let long = "あ".repeat(200);
        let out = sanitize_filename(&long);
        assert!(out.len() <= 200);
        assert!(!out.is_empty());
    }
}
