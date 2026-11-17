//! Diff decorations computed off the UI thread: syntax colors (syntect) and the word-level
//! highlight inside changed lines. The diff is shown immediately in plain colors; decorations
//! arrive a moment later and are simply painted on top.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::OnceLock;

use eframe::egui::Color32;
use facet_git::parse::{FileDiff, LineKind};
use facet_git::CancellationToken;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::filekind::extension;
use crate::textdiff::intraline;

/// Lines are shown (and highlighted) up to this many bytes; the rest is elided. A minified
/// bundle on one line must not turn into a megabyte of glyphs.
pub const MAX_LINE_BYTES: usize = 2000;
/// Decorate at most this many diff lines; beyond that the diff stays plain. Keeps a 10 MB
/// diff from costing seconds of CPU and tens of megabytes of spans.
const LINE_BUDGET: usize = 20_000;

/// A run of one color inside a line, ending at byte `end` of the line's display text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub end: u32,
    /// `None`: the theme's plain text color (the view uses its own).
    pub color: Option<Color32>,
}

/// Per-line decorations for one [`FileDiff`], indexed like [`FileDiff::lines`].
#[derive(Debug, Default)]
pub struct Decorations {
    /// The appearance the syntax colors were made for.
    pub dark: bool,
    /// Syntax spans per line; an empty slice means "no syntax information".
    pub syntax: Vec<Box<[Span]>>,
    /// Byte ranges (in the display text) that changed within a changed line.
    pub emphasis: Vec<Box<[Range<u32>]>>,
}

/// The text of a diff line as it is displayed: lossy UTF-8, without the CR of a CRLF line
/// ending, cut at [`MAX_LINE_BYTES`]. The bool says whether it was cut.
pub fn display_text(bytes: &[u8]) -> (Cow<'_, str>, bool) {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    let (bytes, cut) = if bytes.len() > MAX_LINE_BYTES {
        (&bytes[..MAX_LINE_BYTES], true)
    } else {
        (bytes, false)
    };
    let text = match String::from_utf8_lossy(bytes) {
        // A cut in the middle of a character leaves a replacement char at the end; drop it.
        Cow::Owned(mut text) if cut && text.ends_with('\u{FFFD}') => {
            text.pop();
            Cow::Owned(text)
        }
        other => other,
    };
    (text, cut)
}

struct Assets {
    syntaxes: SyntaxSet,
    themes: ThemeSet,
}

fn assets() -> &'static Assets {
    static ASSETS: OnceLock<Assets> = OnceLock::new();
    ASSETS.get_or_init(|| Assets {
        syntaxes: SyntaxSet::load_defaults_newlines(),
        themes: ThemeSet::load_defaults(),
    })
}

/// File extensions the bundled grammars do not list themselves, mapped to one that fits.
const ALIASES: &[(&str, &str)] = &[
    ("hlsl", "c"),
    ("glsl", "c"),
    ("usf", "c"),
    ("ush", "c"),
    ("cginc", "c"),
    ("shader", "c"),
    ("compute", "c"),
    ("inl", "cpp"),
    ("ixx", "cpp"),
    ("ts", "js"),
    ("tsx", "js"),
    ("jsx", "js"),
    ("mjs", "js"),
    ("cjs", "js"),
    ("uproject", "json"),
    ("uplugin", "json"),
    ("asmdef", "json"),
    ("meta", "yaml"),
    ("unity", "yaml"),
    ("prefab", "yaml"),
    ("asset", "yaml"),
    ("mat", "yaml"),
    ("anim", "yaml"),
    ("controller", "yaml"),
    ("csproj", "xml"),
    ("vcxproj", "xml"),
    ("props", "xml"),
    ("targets", "xml"),
    ("uxml", "xml"),
    ("svg", "xml"),
];

fn find_syntax<'a>(
    syntaxes: &'a SyntaxSet,
    path: &str,
    first_line: &str,
) -> Option<&'a SyntaxReference> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let by_extension = extension(path).and_then(|ext| {
        syntaxes.find_syntax_by_extension(&ext).or_else(|| {
            ALIASES
                .iter()
                .find(|(alias, _)| *alias == ext)
                .and_then(|(_, target)| syntaxes.find_syntax_by_extension(target))
        })
    });
    by_extension
        // Whole-name matches: `Makefile`, `CMakeLists.txt`, `.bashrc`, …
        .or_else(|| syntaxes.find_syntax_by_extension(name))
        .or_else(|| syntaxes.find_syntax_by_first_line(first_line))
        .filter(|syntax| syntax.name != "Plain Text")
}

/// Compute decorations for `diff`. Runs on a worker thread; checks `cancel` between hunks.
pub fn decorate(
    diff: &FileDiff,
    path: &str,
    dark: bool,
    cancel: &CancellationToken,
) -> Decorations {
    let count = diff.lines.len();
    let mut decorations = Decorations {
        dark,
        syntax: vec![Box::default(); count],
        emphasis: vec![Box::default(); count],
    };
    let text = |index: usize| display_text(diff.line_bytes(&diff.lines[index])).0;

    // --- word-level changes: pair each run of removed lines with the added run after it ---
    'hunks: for hunk in &diff.hunks {
        let end = hunk.lines.end as usize;
        let mut index = hunk.lines.start as usize;
        while index < end {
            if index >= LINE_BUDGET || cancel.is_cancelled() {
                break 'hunks;
            }
            if diff.lines[index].kind != LineKind::Removed {
                index += 1;
                continue;
            }
            let removed = index;
            while index < end && diff.lines[index].kind == LineKind::Removed {
                index += 1;
            }
            let added = index;
            while index < end && diff.lines[index].kind == LineKind::Added {
                index += 1;
            }
            for pair in 0..(added - removed).min(index - added) {
                let (old, new) = (removed + pair, added + pair);
                if let Some((old_ranges, new_ranges)) = intraline(&text(old), &text(new)) {
                    decorations.emphasis[old] = old_ranges.into();
                    decorations.emphasis[new] = new_ranges.into();
                }
            }
        }
    }

    // --- syntax colors ---------------------------------------------------------------------
    let assets = assets();
    let first_line = diff.lines.first().map(|_| text(0)).unwrap_or_default();
    let Some(syntax) = find_syntax(&assets.syntaxes, path, &first_line) else {
        return decorations;
    };
    let theme_name = if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    let Some(theme) = assets.themes.themes.get(theme_name) else {
        return decorations;
    };

    for hunk in &diff.hunks {
        if hunk.lines.start as usize >= LINE_BUDGET || cancel.is_cancelled() {
            break;
        }
        // Each hunk starts from a clean parser state, once for the old file's lines and
        // once for the new file's lines; context lines run through both to keep them in
        // step. (A hunk that starts inside a block comment is mis-colored until it ends;
        // parsing the whole file to avoid that is not worth it for a diff.)
        let mut old_side = HighlightLines::new(syntax, theme);
        let mut new_side = HighlightLines::new(syntax, theme);
        for index in hunk.lines.start as usize..(hunk.lines.end as usize).min(LINE_BUDGET) {
            let mut line = text(index).into_owned();
            line.push('\n');
            let spans = match diff.lines[index].kind {
                LineKind::Removed => highlight(&mut old_side, &line, theme, &assets.syntaxes),
                LineKind::Added => highlight(&mut new_side, &line, theme, &assets.syntaxes),
                LineKind::Context => {
                    let _ = highlight(&mut old_side, &line, theme, &assets.syntaxes);
                    highlight(&mut new_side, &line, theme, &assets.syntaxes)
                }
            };
            decorations.syntax[index] = spans.into();
        }
    }
    decorations
}

fn highlight(
    state: &mut HighlightLines<'_>,
    line: &str,
    theme: &Theme,
    syntaxes: &SyntaxSet,
) -> Vec<Span> {
    let Ok(regions) = state.highlight_line(line, syntaxes) else {
        return Vec::new();
    };
    let plain = theme.settings.foreground;
    let content_len = line.len() - 1; // without the newline that was appended for the parser
    let mut spans: Vec<Span> = Vec::new();
    let mut offset = 0usize;
    for (style, piece) in regions {
        offset += piece.len();
        let end = offset.min(content_len) as u32;
        let color = (Some(style.foreground) != plain)
            .then(|| Color32::from_rgb(style.foreground.r, style.foreground.g, style.foreground.b));
        match spans.last_mut() {
            Some(last) if last.color == color => last.end = end,
            Some(last) if last.end == end => {}
            _ => spans.push(Span { end, color }),
        }
    }
    // A line that is entirely plain needs no spans at all.
    if spans.iter().all(|span| span.color.is_none()) {
        spans.clear();
    }
    spans
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use facet_git::parse::parse_file_diff;

    fn diff(patch: &str) -> FileDiff {
        parse_file_diff(patch.as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn display_text_strips_cr_and_cuts_long_lines() {
        assert_eq!(display_text(b"hello\r"), (Cow::Borrowed("hello"), false));
        assert_eq!(display_text(b"plain"), (Cow::Borrowed("plain"), false));
        let long = vec![b'x'; MAX_LINE_BYTES + 50];
        let (text, cut) = display_text(&long);
        assert!(cut);
        assert_eq!(text.len(), MAX_LINE_BYTES);
        // Cutting inside a multi-byte character must not leave garbage behind.
        let mut multibyte = vec![b'a'; MAX_LINE_BYTES - 1];
        multibyte.extend_from_slice("é".as_bytes());
        multibyte.extend_from_slice(b"tail");
        let (text, cut) = display_text(&multibyte);
        assert!(cut);
        assert_eq!(text.len(), MAX_LINE_BYTES - 1);
        // Invalid UTF-8 is shown, not dropped.
        assert_eq!(display_text(b"a\xffb").0, "a\u{FFFD}b");
    }

    #[test]
    fn rust_code_gets_colors_and_word_emphasis() {
        let diff = diff("1\t1\tsrc/lib.rs\0\0diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-    let count = 1; // old\n+    let total = 1; // old\n }\n");
        let decorations = decorate(&diff, "src/lib.rs", true, &CancellationToken::new());
        assert_eq!(decorations.syntax.len(), 4);
        // `fn` is a keyword: the first line starts with a colored span.
        let first = &decorations.syntax[0];
        assert!(first.len() > 1, "{first:?}");
        assert!(first[0].color.is_some());
        assert_eq!(first.last().unwrap().end, "fn main() {".len() as u32);

        // Only the identifier is emphasized on both sides.
        assert_eq!(&*decorations.emphasis[1], &[8..13]);
        assert_eq!(&*decorations.emphasis[2], &[8..13]);
        assert!(decorations.emphasis[0].is_empty());

        // The light theme uses different colors.
        let light = decorate(&diff, "src/lib.rs", false, &CancellationToken::new());
        assert_ne!(light.syntax[0][0].color, first[0].color);
    }

    #[test]
    fn unknown_types_stay_plain_but_keep_word_emphasis() {
        let diff = diff("1\t1\tnotes.xyz\0\0diff --git a/notes.xyz b/notes.xyz\n--- a/notes.xyz\n+++ b/notes.xyz\n@@ -1 +1 @@\n-the quick brown fox\n+the quick red fox\n");
        let decorations = decorate(&diff, "notes.xyz", true, &CancellationToken::new());
        assert!(decorations.syntax.iter().all(|spans| spans.is_empty()));
        assert_eq!(&*decorations.emphasis[0], &[10..15]);
        assert_eq!(&*decorations.emphasis[1], &[10..13]);
    }

    #[test]
    fn aliases_resolve() {
        let syntaxes = &assets().syntaxes;
        for path in [
            "a.hlsl",
            "Game.uproject",
            "Player.prefab",
            "x.tsx",
            "CMakeLists.txt",
        ] {
            // CMake has no bundled grammar; everything else must resolve.
            let found = find_syntax(syntaxes, path, "");
            if path != "CMakeLists.txt" {
                assert!(found.is_some(), "{path}");
            }
        }
        assert!(find_syntax(syntaxes, "script", "#!/bin/bash").is_some());
        assert!(find_syntax(syntaxes, "data.unknown", "nothing special").is_none());
    }
}
