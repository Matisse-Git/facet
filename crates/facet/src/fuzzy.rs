//! Tiny fuzzy matcher for the command palette: case-insensitive subsequence matching that
//! favors word starts and consecutive runs.

/// A successful match: higher `score` is better; `positions` are the matched char indices in
/// the candidate (for highlighting).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    pub positions: Vec<usize>,
}

const BASE: i32 = 10;
const CONSECUTIVE: i32 = 18;
const WORD_START: i32 = 24;
const GAP: i32 = 1;

/// Match `query` against `candidate`. Whitespace in the query is ignored. An empty query
/// matches everything with score 0.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<Match> {
    let needle: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if needle.is_empty() {
        return Some(Match {
            score: 0,
            positions: Vec::new(),
        });
    }
    let haystack: Vec<char> = candidate.chars().collect();
    let lower: Vec<char> = haystack
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();

    // Try every possible starting position for the first character and keep the best greedy
    // alignment. Candidates are short (command titles), so this is plenty fast.
    let mut best: Option<Match> = None;
    for start in 0..lower.len() {
        if lower[start] != needle[0] {
            continue;
        }
        if let Some(found) = align_from(&needle, &haystack, &lower, start) {
            if best.as_ref().is_none_or(|b| found.score > b.score) {
                best = Some(found);
            }
        }
    }
    best
}

fn align_from(needle: &[char], haystack: &[char], lower: &[char], start: usize) -> Option<Match> {
    let mut positions = Vec::with_capacity(needle.len());
    let mut score = 0;
    let mut cursor = start;
    let mut previous: Option<usize> = None;

    for &wanted in needle {
        // Prefer the next word start that matches; otherwise the next occurrence at all.
        let next_any = (cursor..lower.len()).find(|&i| lower[i] == wanted)?;
        let next_word = (cursor..lower.len())
            .find(|&i| lower[i] == wanted && is_word_start(haystack, i))
            .filter(|_| previous.is_none_or(|p| next_any != p + 1));
        let at = next_word.unwrap_or(next_any);

        score += BASE;
        if is_word_start(haystack, at) {
            score += WORD_START;
        }
        match previous {
            Some(p) if at == p + 1 => score += CONSECUTIVE,
            Some(p) => score -= GAP * (at - p - 1) as i32,
            None => score -= GAP * at as i32,
        }
        positions.push(at);
        previous = Some(at);
        cursor = at + 1;
    }
    // Slightly prefer shorter candidates so exact-ish matches win ties.
    score -= (haystack.len() / 8) as i32;
    Some(Match { score, positions })
}

fn is_word_start(haystack: &[char], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    let (prev, this) = (haystack[index - 1], haystack[index]);
    !prev.is_alphanumeric() || (prev.is_lowercase() && this.is_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(query: &str, candidate: &str) -> Option<i32> {
        fuzzy_match(query, candidate).map(|m| m.score)
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(score("", "Open Repository…"), Some(0));
        assert_eq!(score("   ", "anything"), Some(0));
    }

    #[test]
    fn requires_all_characters_in_order() {
        assert!(score("opn", "Open Repository").is_some());
        assert!(
            score("yo", "Open Repository").is_none(),
            "nothing follows the final y"
        );
        assert!(score("xyz", "Open Repository").is_none());
    }

    #[test]
    fn is_case_insensitive_and_ignores_query_spaces() {
        assert_eq!(
            score("OPEN", "open repository"),
            score("open", "Open Repository")
        );
        assert_eq!(
            score("open repo", "Open Repository"),
            score("openrepo", "Open Repository")
        );
    }

    #[test]
    fn word_starts_beat_scattered_letters() {
        // "tcl" should prefer the acronym over letters buried mid-word.
        let acronym = score("tcl", "Toggle Command Log").unwrap();
        let buried = score("tcl", "Fetch all Collections").unwrap();
        assert!(acronym > buried, "{acronym} vs {buried}");
    }

    #[test]
    fn consecutive_runs_beat_gaps() {
        let run = score("theme", "Theme: Dark").unwrap();
        let gappy = score("theme", "Toggle the Hidden Element Menu Entry").unwrap();
        assert!(run > gappy, "{run} vs {gappy}");
    }

    #[test]
    fn reports_match_positions() {
        let m = fuzzy_match("tcl", "Toggle Command Log").unwrap();
        assert_eq!(m.positions, [0, 7, 15]);
        let m = fuzzy_match("dark", "Theme: Dark").unwrap();
        assert_eq!(m.positions, [7, 8, 9, 10]);
    }

    #[test]
    fn handles_unicode_candidates() {
        let m = fuzzy_match("ÉTÉ", "Niveau été").unwrap();
        assert_eq!(m.positions, [7, 8, 9]);
        assert!(fuzzy_match("nivé", "Niveau été").is_some());
    }
}
