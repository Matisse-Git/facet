//! Building patches for partial (hunk / line) staging, unstaging and discarding.
//!
//! Given the parsed diff of one file and a set of selected changed lines, [`build_patch`]
//! produces a patch for `git apply`. Git stays the one that edits the index or the file; this
//! module only decides which lines go into the patch.
//!
//! Two directions exist, depending on which side of the diff the patch is applied to:
//!
//! * [`PatchDirection::Forward`] — applied to the *old* side (staging from a worktree diff:
//!   `git apply --cached`). An unselected removal stays as context, an unselected addition is
//!   left out.
//! * [`PatchDirection::Reverse`] — applied with `--reverse` to the *new* side (unstaging from a
//!   staged diff, discarding from a worktree diff). An unselected addition stays as context, an
//!   unselected removal is left out.

use crate::parse::{DiffLine, FileDiff, LineKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchDirection {
    Forward,
    Reverse,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatchError {
    #[error("no changed lines are selected")]
    NothingSelected,
    #[error("binary files cannot be staged in parts")]
    Binary,
    /// The selection splits a change to the final newline of the file from the line it belongs
    /// to; no patch can express that.
    #[error(
        "these lines change the end of the file together with a neighbouring line; \
         select both, or use the whole hunk"
    )]
    EndOfFile,
}

const NO_NEWLINE: &[u8] = b"\\ No newline at end of file\n";

/// Build a patch for `path` containing the changed lines for which `selected(line_index)` is
/// true (indices into [`FileDiff::lines`]). Hunks without a selected line are left out.
pub fn build_patch(
    diff: &FileDiff,
    path: &str,
    direction: PatchDirection,
    selected: impl Fn(usize) -> bool,
) -> Result<Vec<u8>, PatchError> {
    if diff.binary {
        return Err(PatchError::Binary);
    }

    let mut body = Vec::new();
    // new-minus-old line count of the hunks emitted so far.
    let mut delta = 0i64;
    let mut total_old = 0u32;
    let mut total_new = 0u32;

    for hunk in &diff.hunks {
        let mut out: Vec<(LineKind, &DiffLine)> = Vec::new();
        let mut picked = false;
        for index in hunk.lines.start as usize..hunk.lines.end as usize {
            let line = &diff.lines[index];
            if line.kind == LineKind::Context {
                out.push((LineKind::Context, line));
            } else if selected(index) {
                picked = true;
                out.push((line.kind, line));
            } else {
                let stays = matches!(
                    (direction, line.kind),
                    (PatchDirection::Forward, LineKind::Removed)
                        | (PatchDirection::Reverse, LineKind::Added)
                );
                if stays {
                    out.push((LineKind::Context, line));
                }
            }
        }
        if !picked {
            continue;
        }

        // A line without a trailing newline must be the last line of its side.
        let (mut old_ended, mut new_ended) = (false, false);
        let (mut old_count, mut new_count) = (0u32, 0u32);
        for (kind, line) in &out {
            let (old, new) = sides(*kind);
            if (old && old_ended) || (new && new_ended) {
                return Err(PatchError::EndOfFile);
            }
            old_count += u32::from(old);
            new_count += u32::from(new);
            if line.no_newline {
                old_ended |= old;
                new_ended |= new;
            }
        }

        // The side the patch is applied to keeps the numbering git gave it; the other side's
        // start is derived (git treats it as a hint, but keep it right anyway).
        let (old_start, new_start) = match direction {
            PatchDirection::Forward => (
                hunk.old_start,
                derived_start(hunk.old_start, old_count, new_count, delta),
            ),
            PatchDirection::Reverse => (
                derived_start(hunk.new_start, new_count, old_count, -delta),
                hunk.new_start,
            ),
        };
        delta += i64::from(new_count) - i64::from(old_count);
        total_old += old_count;
        total_new += new_count;

        body.extend_from_slice(
            format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@\n").as_bytes(),
        );
        for (kind, line) in out {
            body.push(match kind {
                LineKind::Context => b' ',
                LineKind::Added => b'+',
                LineKind::Removed => b'-',
            });
            body.extend_from_slice(diff.line_bytes(line));
            body.push(b'\n');
            if line.no_newline {
                body.extend_from_slice(NO_NEWLINE);
            }
        }
    }

    if body.is_empty() {
        return Err(PatchError::NothingSelected);
    }

    // A file only stays "new" / "deleted" in the patch while that side is still empty.
    let creates = diff.new_file && total_old == 0;
    let deletes = diff.deleted_file && total_new == 0;
    let old_name = header_path("a/", path);
    let new_name = header_path("b/", path);

    let mut patch = Vec::with_capacity(body.len() + 2 * path.len() + 96);
    patch.extend_from_slice(format!("diff --git {old_name} {new_name}\n").as_bytes());
    if creates {
        let mode = diff.new_mode.unwrap_or(0o100644);
        patch.extend_from_slice(format!("new file mode {mode:o}\n").as_bytes());
    } else if deletes {
        let mode = diff.old_mode.unwrap_or(0o100644);
        patch.extend_from_slice(format!("deleted file mode {mode:o}\n").as_bytes());
    }
    let old_line = if creates { "/dev/null" } else { &old_name };
    let new_line = if deletes { "/dev/null" } else { &new_name };
    patch.extend_from_slice(format!("--- {old_line}\n+++ {new_line}\n").as_bytes());
    patch.extend_from_slice(&body);
    Ok(patch)
}

/// Whether a line of this kind exists in the old and in the new file.
fn sides(kind: LineKind) -> (bool, bool) {
    match kind {
        LineKind::Context => (true, true),
        LineKind::Added => (false, true),
        LineKind::Removed => (true, false),
    }
}

/// The start line of the other side of a hunk. In unified diffs a range with a count of zero
/// names the line *before* the position, otherwise the first line.
fn derived_start(anchor_start: u32, anchor_count: u32, count: u32, shift: i64) -> u32 {
    let position = i64::from(anchor_start) + i64::from(anchor_count == 0) + shift;
    let start = if count == 0 { position - 1 } else { position };
    start.max(0) as u32
}

/// `a/path` as git writes it in patch headers: C-quoted when it contains characters that
/// would otherwise end or confuse the name.
fn header_path(prefix: &str, path: &str) -> String {
    let needs_quotes = path
        .bytes()
        .any(|b| b < 0x20 || b == 0x7f || b == b'"' || b == b'\\');
    if !needs_quotes {
        return format!("{prefix}{path}");
    }
    let mut quoted = format!("\"{prefix}");
    for ch in path.chars() {
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\t' => quoted.push_str("\\t"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                quoted.push_str(&format!("\\{:03o}", c as u32));
            }
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_file_diff;

    fn diff(text: &str) -> FileDiff {
        parse_file_diff(text.as_bytes().to_vec()).unwrap()
    }

    fn patch(
        diff: &FileDiff,
        direction: PatchDirection,
        lines: &[usize],
    ) -> Result<String, PatchError> {
        build_patch(diff, "f.txt", direction, |i| lines.contains(&i))
            .map(|bytes| String::from_utf8(bytes).unwrap())
    }

    // Lines: 0 ` a`, 1 `-b`, 2 `+B`, 3 ` c`, 4 `-d`, 5 `+D`, 6 `+E`, 7 ` f`
    const TWO_CHANGES: &str = "3\t2\tf.txt\0\0diff --git a/f.txt b/f.txt\nindex 1..2 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,5 +1,6 @@\n a\n-b\n+B\n c\n-d\n+D\n+E\n f\n";

    #[test]
    fn forward_keeps_unselected_removals_as_context() {
        let diff = diff(TWO_CHANGES);
        assert_eq!(
            patch(&diff, PatchDirection::Forward, &[1, 2]).unwrap(),
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,5 +1,5 @@\n a\n-b\n+B\n c\n d\n f\n"
        );
        // Only an addition: nothing is removed, the old side is untouched.
        assert_eq!(
            patch(&diff, PatchDirection::Forward, &[6]).unwrap(),
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,5 +1,6 @@\n a\n b\n c\n d\n+E\n f\n"
        );
    }

    #[test]
    fn reverse_keeps_unselected_additions_as_context() {
        let diff = diff(TWO_CHANGES);
        assert_eq!(
            patch(&diff, PatchDirection::Reverse, &[1, 2]).unwrap(),
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,6 +1,6 @@\n a\n-b\n+B\n c\n D\n E\n f\n"
        );
        assert_eq!(
            patch(&diff, PatchDirection::Reverse, &[4]).unwrap(),
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,7 +1,6 @@\n a\n B\n c\n-d\n D\n E\n f\n"
        );
    }

    #[test]
    fn everything_selected_reproduces_the_hunk() {
        let diff = diff(TWO_CHANGES);
        let all = [1, 2, 4, 5, 6];
        for direction in [PatchDirection::Forward, PatchDirection::Reverse] {
            assert_eq!(
                patch(&diff, direction, &all).unwrap(),
                "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,5 +1,6 @@\n a\n-b\n+B\n c\n-d\n+D\n+E\n f\n"
            );
        }
    }

    #[test]
    fn later_hunks_are_renumbered_after_a_partial_one() {
        // Hunk 1 adds two lines (lines 1,2); hunk 2 replaces one (lines 5,6).
        let diff = diff("3\t1\tf.txt\0\0diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,4 @@\n a\n+x\n+y\n b\n@@ -10,2 +12,2 @@\n j\n-k\n+K\n");
        // Forward, taking only `+x` and the second hunk: new side shifts by one, not two.
        assert_eq!(
            patch(&diff, PatchDirection::Forward, &[1, 5, 6]).unwrap(),
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,3 @@\n a\n+x\n b\n@@ -10,2 +11,2 @@\n j\n-k\n+K\n"
        );
        // Reverse, same selection: the new side is authoritative, the old side is derived.
        assert_eq!(
            patch(&diff, PatchDirection::Reverse, &[1, 5, 6]).unwrap(),
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,4 @@\n a\n+x\n y\n b\n@@ -11,2 +12,2 @@\n j\n-k\n+K\n"
        );
        // A hunk with nothing selected is left out entirely.
        assert_eq!(
            patch(&diff, PatchDirection::Forward, &[5, 6]).unwrap(),
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -10,2 +10,2 @@\n j\n-k\n+K\n"
        );
    }

    #[test]
    fn new_and_deleted_files() {
        let new = diff("3\t0\tf.txt\0\0diff --git a/f.txt b/f.txt\nnew file mode 100755\n--- /dev/null\n+++ b/f.txt\n@@ -0,0 +1,3 @@\n+one\n+two\n+three\n");
        // Staging part of a new file still creates it.
        assert_eq!(
            patch(&new, PatchDirection::Forward, &[1]).unwrap(),
            "diff --git a/f.txt b/f.txt\nnew file mode 100755\n--- /dev/null\n+++ b/f.txt\n@@ -0,0 +1,1 @@\n+two\n"
        );
        // Unstaging part of a staged new file leaves the file in the index: a modification.
        assert_eq!(
            patch(&new, PatchDirection::Reverse, &[1]).unwrap(),
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,3 @@\n one\n+two\n three\n"
        );
        assert!(patch(&new, PatchDirection::Reverse, &[0, 1, 2])
            .unwrap()
            .contains("new file mode 100755\n--- /dev/null\n+++ b/f.txt\n@@ -0,0 +1,3 @@"));

        let deleted = diff("0\t2\tf.txt\0\0diff --git a/f.txt b/f.txt\ndeleted file mode 100644\n--- a/f.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\n-two\n");
        // Staging part of a deletion keeps the file: a modification.
        assert_eq!(
            patch(&deleted, PatchDirection::Forward, &[0]).unwrap(),
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,1 @@\n-one\n two\n"
        );
        assert_eq!(
            patch(&deleted, PatchDirection::Reverse, &[1]).unwrap(),
            "diff --git a/f.txt b/f.txt\ndeleted file mode 100644\n--- a/f.txt\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-two\n"
        );
    }

    #[test]
    fn end_of_file_newline() {
        // Lines: 0 ` a`, 1 `-last` (no newline), 2 `+last`, 3 `+more` (no newline)
        let diff = diff("2\t1\tf.txt\0\0diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,3 @@\n a\n-last\n\\ No newline at end of file\n+last\n+more\n\\ No newline at end of file\n");
        // The whole hunk keeps both markers.
        assert_eq!(
            patch(&diff, PatchDirection::Forward, &[1, 2, 3]).unwrap(),
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,3 @@\n a\n-last\n\\ No newline at end of file\n+last\n+more\n\\ No newline at end of file\n"
        );
        // `+more` alone would have to follow a line that has no newline: not expressible.
        assert_eq!(
            patch(&diff, PatchDirection::Forward, &[3]),
            Err(PatchError::EndOfFile)
        );
        // In reverse the same selection is fine: `last` (with newline) is already there.
        assert_eq!(
            patch(&diff, PatchDirection::Reverse, &[3]).unwrap(),
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,3 @@\n a\n last\n+more\n\\ No newline at end of file\n"
        );
        assert_eq!(
            patch(&diff, PatchDirection::Reverse, &[1]),
            Err(PatchError::EndOfFile)
        );
    }

    #[test]
    fn nothing_selected_and_binary() {
        let text = diff(TWO_CHANGES);
        assert_eq!(
            patch(&text, PatchDirection::Forward, &[]),
            Err(PatchError::NothingSelected)
        );
        // Context lines cannot be "selected".
        assert_eq!(
            patch(&text, PatchDirection::Forward, &[0, 3]),
            Err(PatchError::NothingSelected)
        );
        let binary = diff(
            "-\t-\tb.bin\0\0diff --git a/b.bin b/b.bin\nBinary files a/b.bin and b/b.bin differ\n",
        );
        assert_eq!(
            patch(&binary, PatchDirection::Forward, &[0]),
            Err(PatchError::Binary)
        );
    }

    #[test]
    fn crlf_bytes_survive() {
        let diff = diff("1\t1\tf.txt\0\0diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,2 @@\n a\r\n-b\r\n+B\r\n");
        assert_eq!(
            patch(&diff, PatchDirection::Forward, &[1, 2]).unwrap(),
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,2 @@\n a\r\n-b\r\n+B\r\n"
        );
    }

    #[test]
    fn header_paths() {
        assert_eq!(header_path("a/", "src/my file.rs"), "a/src/my file.rs");
        assert_eq!(header_path("b/", "été/光.txt"), "b/été/光.txt");
        assert_eq!(header_path("a/", "odd\"name"), "\"a/odd\\\"name\"");
        assert_eq!(header_path("a/", "tab\there"), "\"a/tab\\there\"");
        assert_eq!(header_path("a/", "é\"x"), "\"a/é\\\"x\"");
    }
}
