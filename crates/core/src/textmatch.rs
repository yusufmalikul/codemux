//! Finding search text inside other text, ignoring case: where it is (to
//! highlight it) and a short piece around it (to show why a session matched).

use std::ops::Range;

/// Characters of context kept before the match in a snippet. Short, so the
/// match is still on screen when the sidebar cuts the line.
const SNIPPET_BEFORE: usize = 30;
/// Characters kept after the match. The sidebar cuts the rest with "…".
const SNIPPET_AFTER: usize = 150;

/// `query` in one line: line breaks and runs of spaces become one space,
/// and lowercase. Text shown on one line is cleaned the same way, so a
/// query typed with two spaces still matches it.
pub fn normalize_query(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Every place `query` (already lowercase) is in `text`, ignoring case, as
/// byte ranges of `text`. Places do not overlap. Empty for an empty query.
pub fn match_ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    // Lowercase `text`, and remember for each byte of the lowercase text
    // where its character starts in `text`. Lowercasing can change a
    // character's length (and even make two characters of one), so the
    // byte positions of the two strings differ.
    let mut lower = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len() + 1);
    for (i, c) in text.char_indices() {
        for l in c.to_lowercase() {
            let before = lower.len();
            lower.push(l);
            origin.resize(origin.len() + lower.len() - before, i);
        }
    }
    origin.push(text.len());
    let end_of = |lower_end: usize| {
        // The match may end inside a character that lowercased to several:
        // take the whole original character.
        let start = origin[lower_end - 1];
        start + text[start..].chars().next().map_or(0, char::len_utf8)
    };
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(query) {
        let start = from + pos;
        let end = start + query.len();
        ranges.push(origin[start]..end_of(end));
        from = end;
    }
    ranges
}

/// A short one-line piece of `text` around the first place `query`
/// (lowercase) is, or None if it is not there. Cut text at either end is
/// marked with "…".
pub fn snippet(text: &str, query: &str) -> Option<String> {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let found = match_ranges(&line, query).into_iter().next().or_else(|| {
        // The query has a line break or a double space that the one-line
        // text no longer has: find it in the original, then in the line.
        match_ranges(text, query).first().and_then(|_| match_ranges(&line, &normalize_query(query)).into_iter().next())
    })?;
    // Back up a few characters, then on to the start of a word.
    let mut start = line[..found.start].char_indices().rev().nth(SNIPPET_BEFORE.saturating_sub(1)).map_or(0, |(i, _)| i);
    if start > 0 {
        if let Some(space) = line[start..found.start].find(' ') {
            start += space + 1;
        }
    }
    let end = line[found.end..].char_indices().nth(SNIPPET_AFTER).map_or(line.len(), |(i, _)| found.end + i);
    let mut piece = String::new();
    if start > 0 {
        piece.push('…');
    }
    piece.push_str(line[start..end].trim_end());
    if end < line.len() {
        piece.push('…');
    }
    Some(piece)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found<'a>(text: &'a str, query: &str) -> Vec<&'a str> {
        match_ranges(text, query).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn finds_every_place_in_any_case() {
        assert_eq!(found("Fix Webhook retry, webhook again", "webhook"), ["Webhook", "webhook"]);
        assert_eq!(found("aaaa", "aa"), ["aa", "aa"], "places must not overlap");
        assert!(found("Fix webhook", "zebra").is_empty());
        assert!(found("Fix webhook", "").is_empty());
    }

    #[test]
    fn keeps_byte_places_right_when_lowercase_changes_length() {
        // "İ" is 2 bytes, but lowercases to "i̇" (3 bytes): later places shift.
        assert_eq!(found("İstanbul CAFÉ café", "café"), ["CAFÉ", "café"]);
        assert_eq!(found("İstanbul", "i̇stan"), ["İstan"]);
    }

    #[test]
    fn normalize_query_makes_one_lowercase_line() {
        assert_eq!(normalize_query("  Fix\n  Webhook "), "fix webhook");
    }

    #[test]
    fn snippet_shows_the_match_with_a_little_before_it() {
        let text = format!("{} the zebra has\nstripes {}", "words ".repeat(20), "more ".repeat(60));
        let s = snippet(&text, "zebra").unwrap();
        assert!(s.starts_with('…'), "{s}");
        assert!(s.ends_with('…'), "{s}");
        assert!(!s.contains('\n'), "{s}");
        let at = s.find("zebra has stripes").expect("match not in snippet");
        assert!(s[..at].chars().count() <= SNIPPET_BEFORE + 1, "too much before the match: {s}");
        // It starts at a word, not in the middle of one.
        assert!(s.starts_with("…words "), "{s}");
    }

    #[test]
    fn snippet_of_short_text_is_the_whole_text() {
        assert_eq!(snippet("tell me about Zebra\nstripes", "zebra").as_deref(), Some("tell me about Zebra stripes"));
        assert_eq!(snippet("tell me about\nZebra stripes", "about\nzebra").as_deref(), Some("tell me about Zebra stripes"));
        assert_eq!(snippet("nothing here", "zebra"), None);
    }
}
