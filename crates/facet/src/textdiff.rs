//! Word-level comparison of two lines, for the intra-line highlight in diffs.
//!
//! A removed line and the added line that replaced it are split into tokens (words, runs of
//! whitespace, single punctuation characters) and aligned by longest common subsequence. The
//! tokens outside the common subsequence are what changed.

use std::ops::Range;

/// Lines with more tokens than this are not compared (the table would be large and the
/// highlight unreadable anyway).
const MAX_TOKENS: usize = 400;
/// Below this share of common text the two lines are treated as unrelated: no highlight.
const MIN_SIMILARITY: f32 = 0.3;

/// Changed byte ranges of one line.
pub type Ranges = Vec<Range<u32>>;

/// Byte ranges that changed in the old line and in the new line. `None` when the lines are
/// too different (or too long) for a word-level highlight to help.
pub fn intraline(old: &str, new: &str) -> Option<(Ranges, Ranges)> {
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    if old_tokens.is_empty()
        || new_tokens.is_empty()
        || old_tokens.len() > MAX_TOKENS
        || new_tokens.len() > MAX_TOKENS
    {
        return None;
    }
    let old_text = |index: usize| &old[old_tokens[index].clone()];
    let new_text = |index: usize| &new[new_tokens[index].clone()];

    // Common prefix and suffix need no table.
    let mut prefix = 0;
    while prefix < old_tokens.len()
        && prefix < new_tokens.len()
        && old_text(prefix) == new_text(prefix)
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old_tokens.len() - prefix
        && suffix < new_tokens.len() - prefix
        && old_text(old_tokens.len() - 1 - suffix) == new_text(new_tokens.len() - 1 - suffix)
    {
        suffix += 1;
    }

    let rows = old_tokens.len() - prefix - suffix;
    let cols = new_tokens.len() - prefix - suffix;
    let mut old_common = vec![false; old_tokens.len()];
    let mut new_common = vec![false; new_tokens.len()];
    for index in 0..prefix {
        old_common[index] = true;
        new_common[index] = true;
    }
    for index in 0..suffix {
        old_common[old_tokens.len() - 1 - index] = true;
        new_common[new_tokens.len() - 1 - index] = true;
    }

    // Longest common subsequence of the middle part.
    let width = cols + 1;
    let mut table = vec![0u16; (rows + 1) * width];
    for row in (0..rows).rev() {
        for col in (0..cols).rev() {
            table[row * width + col] = if old_text(prefix + row) == new_text(prefix + col) {
                table[(row + 1) * width + col + 1] + 1
            } else {
                table[(row + 1) * width + col].max(table[row * width + col + 1])
            };
        }
    }
    let (mut row, mut col) = (0, 0);
    while row < rows && col < cols {
        if old_text(prefix + row) == new_text(prefix + col) {
            old_common[prefix + row] = true;
            new_common[prefix + col] = true;
            row += 1;
            col += 1;
        } else if table[(row + 1) * width + col] >= table[row * width + col + 1] {
            row += 1;
        } else {
            col += 1;
        }
    }

    // How much real (non-blank) text do the lines share?
    let visible = |text: &str| text.chars().filter(|c| !c.is_whitespace()).count();
    let shared: usize = (0..old_tokens.len())
        .filter(|index| old_common[*index])
        .map(|index| visible(old_text(index)))
        .sum();
    let longest = visible(old).max(visible(new));
    if longest == 0 || (shared as f32) < MIN_SIMILARITY * longest as f32 {
        return None;
    }

    Some((
        changed_ranges(old, &old_tokens, &old_common),
        changed_ranges(new, &new_tokens, &new_common),
    ))
}

/// Merge the changed tokens into ranges. Unchanged whitespace between two changed tokens is
/// swallowed so a rewritten phrase reads as one highlight, not a dotted line.
fn changed_ranges(text: &str, tokens: &[Range<usize>], common: &[bool]) -> Vec<Range<u32>> {
    let mut ranges: Vec<Range<u32>> = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        if common[index] {
            index += 1;
            continue;
        }
        let start = tokens[index].start;
        let mut end = tokens[index].end;
        index += 1;
        loop {
            if index < tokens.len() && !common[index] {
                end = tokens[index].end;
                index += 1;
            } else if index + 1 < tokens.len()
                && !common[index + 1]
                && text[tokens[index].clone()].chars().all(char::is_whitespace)
            {
                end = tokens[index + 1].end;
                index += 2;
            } else {
                break;
            }
        }
        ranges.push(start as u32..end as u32);
    }
    ranges
}

/// Split into words (letters, digits, `_`), whitespace runs, and single other characters.
fn tokenize(text: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut tokens = Vec::new();
    let mut current: Option<(usize, Class)> = None;
    for (offset, c) in text.char_indices() {
        let kind = class(c);
        match current {
            Some((_, running)) if running == kind && kind != Class::Other => {}
            Some((start, _)) => {
                tokens.push(start..offset);
                current = Some((offset, kind));
            }
            None => current = Some((offset, kind)),
        }
    }
    if let Some((start, _)) = current {
        tokens.push(start..text.len());
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed<'a>(text: &'a str, ranges: &[Range<u32>]) -> Vec<&'a str> {
        ranges
            .iter()
            .map(|range| &text[range.start as usize..range.end as usize])
            .collect()
    }

    #[test]
    fn tokens() {
        let text = "let x_1 = foo(a,  b);";
        let tokens: Vec<&str> = tokenize(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(
            tokens,
            ["let", " ", "x_1", " ", "=", " ", "foo", "(", "a", ",", "  ", "b", ")", ";"]
        );
        assert!(tokenize("").is_empty());
    }

    #[test]
    fn a_renamed_identifier() {
        let old = "    let count = items.len();";
        let new = "    let total = items.len();";
        let (a, b) = intraline(old, new).unwrap();
        assert_eq!(changed(old, &a), ["count"]);
        assert_eq!(changed(new, &b), ["total"]);
    }

    #[test]
    fn an_inserted_argument_and_unicode() {
        let old = "draw(mesh, shader);";
        let new = "draw(mesh, shader, été);";
        let (a, b) = intraline(old, new).unwrap();
        assert!(a.is_empty());
        assert_eq!(changed(new, &b), [", été"]);
    }

    #[test]
    fn neighbouring_changes_merge_across_blanks() {
        let old = "return the old value here";
        let new = "return a new result here";
        let (a, b) = intraline(old, new).unwrap();
        assert_eq!(changed(old, &a), ["the old value"]);
        assert_eq!(changed(new, &b), ["a new result"]);
    }

    #[test]
    fn unrelated_lines_get_no_highlight() {
        assert_eq!(intraline("import os", "fn main() { println!(); }"), None);
        assert_eq!(intraline("", "anything"), None);
        let long = "word ".repeat(MAX_TOKENS);
        assert_eq!(intraline(&long, &long), None);
    }

    #[test]
    fn identical_lines_have_no_changes() {
        let (a, b) = intraline("same line", "same line").unwrap();
        assert!(a.is_empty() && b.is_empty());
    }

    #[test]
    fn whitespace_only_change() {
        let old = "foo(a,b)";
        let new = "foo(a, b)";
        let (a, b) = intraline(old, new).unwrap();
        assert!(a.is_empty());
        assert_eq!(changed(new, &b), [" "]);
    }
}
