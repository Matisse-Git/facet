//! Merge conflicts: the three versions of a conflicted file (stages 1–3 of the index), the
//! conflict markers git wrote into the working copy, and resolving them — whole file to one
//! side, chunk by chunk, or with an external merge tool.

use std::path::Path;

use crate::{Git, GitError};

/// One side of a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// Stage 2: the branch being merged into (HEAD). During a rebase this is the upstream.
    Ours,
    /// Stage 3: the commit being merged, picked or replayed.
    Theirs,
}

/// A conflicted file's versions from the index; `None` where that side has no file (it was
/// added on one side only, or deleted on the other).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConflictVersions {
    pub base: Option<Vec<u8>>,
    pub ours: Option<Vec<u8>>,
    pub theirs: Option<Vec<u8>>,
}

/// Read stages 1, 2 and 3 of `path` at once (G5).
pub async fn conflict_versions(
    git: &Git,
    root: &Path,
    path: &str,
) -> Result<ConflictVersions, GitError> {
    let stage = |n: u8| {
        git.command(["cat-file", "blob", &format!(":{n}:{path}")])
            .current_dir(root)
            .background()
            .output()
    };
    let (base, ours, theirs) = tokio::join!(stage(1), stage(2), stage(3));
    let blob = |output: crate::GitOutput| output.success().then_some(output.stdout);
    Ok(ConflictVersions {
        base: blob(base?),
        ours: blob(ours?),
        theirs: blob(theirs?),
    })
}

/// Resolve a whole file to one side and stage the result. If that side deleted the file,
/// resolving means deleting it.
pub async fn take_side(
    git: &Git,
    root: &Path,
    path: &str,
    side: Side,
    side_exists: bool,
) -> Result<(), GitError> {
    let spec = format!(":(literal){path}");
    if side_exists {
        let flag = match side {
            Side::Ours => "--ours",
            Side::Theirs => "--theirs",
        };
        git.command(["checkout", flag, "--", &spec])
            .current_dir(root)
            .run()
            .await?;
        git.command(["add", "--", &spec])
            .current_dir(root)
            .run()
            .await
            .map(drop)
    } else {
        git.command(["rm", "--quiet", "--", &spec])
            .current_dir(root)
            .run()
            .await
            .map(drop)
    }
}

/// Write `contents` as the resolved file and stage it.
pub async fn resolve_with(
    git: &Git,
    root: &Path,
    path: &str,
    contents: &[u8],
) -> Result<(), GitError> {
    tokio::fs::write(root.join(path), contents).await?;
    git.command(["add", "--", &format!(":(literal){path}")])
        .current_dir(root)
        .run()
        .await
        .map(drop)
}

/// A stretch of a conflicted working-copy file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Region {
    /// Text both sides agree on.
    Common(String),
    Conflict(Chunk),
}

/// One `<<<<<<< … >>>>>>>` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub ours: String,
    /// Present with `merge.conflictStyle = diff3` / `zdiff3`.
    pub base: Option<String>,
    pub theirs: String,
    pub ours_label: String,
    pub theirs_label: String,
    /// The block exactly as written, markers included, to keep it unresolved.
    pub raw: String,
}

/// How to resolve one chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Choice {
    Ours,
    Theirs,
    /// Both, ours first.
    OursThenTheirs,
    /// Both, theirs first.
    TheirsThenOurs,
}

impl Choice {
    pub const ALL: [Self; 4] = [
        Self::Ours,
        Self::Theirs,
        Self::OursThenTheirs,
        Self::TheirsThenOurs,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ours => "Take Ours",
            Self::Theirs => "Take Theirs",
            Self::OursThenTheirs => "Both (Ours First)",
            Self::TheirsThenOurs => "Both (Theirs First)",
        }
    }
}

fn marker(line: &str, ch: char) -> Option<&str> {
    let body = line.trim_end_matches(['\r', '\n']);
    let rest = body.strip_prefix(&*ch.to_string().repeat(7))?;
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix(' ')
    }
}

/// Split a conflicted file into common text and conflict chunks. `None` when the file has no
/// well-formed markers (already resolved by hand, or a binary file).
pub fn parse_markers(text: &str) -> Option<Vec<Region>> {
    enum State {
        Common,
        Ours,
        Base,
        Theirs,
    }
    let mut regions = Vec::new();
    let mut common = String::new();
    let mut chunk = Chunk {
        ours: String::new(),
        base: None,
        theirs: String::new(),
        ours_label: String::new(),
        theirs_label: String::new(),
        raw: String::new(),
    };
    let mut state = State::Common;
    for line in text.split_inclusive('\n') {
        match state {
            State::Common => {
                if let Some(label) = marker(line, '<') {
                    if !common.is_empty() {
                        regions.push(Region::Common(std::mem::take(&mut common)));
                    }
                    chunk.ours_label = label.to_owned();
                    chunk.raw.push_str(line);
                    state = State::Ours;
                } else {
                    common.push_str(line);
                }
                continue;
            }
            State::Ours | State::Base => {
                chunk.raw.push_str(line);
                if marker(line, '|').is_some() {
                    chunk.base = Some(String::new());
                    state = State::Base;
                } else if marker(line, '=') == Some("") {
                    state = State::Theirs;
                } else if marker(line, '<').is_some() || marker(line, '>').is_some() {
                    return None;
                } else if matches!(state, State::Ours) {
                    chunk.ours.push_str(line);
                } else if let Some(base) = &mut chunk.base {
                    base.push_str(line);
                }
            }
            State::Theirs => {
                chunk.raw.push_str(line);
                if let Some(label) = marker(line, '>') {
                    chunk.theirs_label = label.to_owned();
                    let done = std::mem::replace(
                        &mut chunk,
                        Chunk {
                            ours: String::new(),
                            base: None,
                            theirs: String::new(),
                            ours_label: String::new(),
                            theirs_label: String::new(),
                            raw: String::new(),
                        },
                    );
                    regions.push(Region::Conflict(done));
                    state = State::Common;
                } else if marker(line, '<').is_some() || marker(line, '=').is_some() {
                    return None;
                } else {
                    chunk.theirs.push_str(line);
                }
            }
        }
    }
    if !matches!(state, State::Common) {
        return None;
    }
    if !common.is_empty() {
        regions.push(Region::Common(common));
    }
    regions
        .iter()
        .any(|region| matches!(region, Region::Conflict(_)))
        .then_some(regions)
}

/// The file with each chunk resolved by its choice (in chunk order); chunks without a choice
/// keep their markers.
pub fn apply_choices(regions: &[Region], choices: &[Option<Choice>]) -> String {
    let mut out = String::new();
    let mut index = 0;
    for region in regions {
        match region {
            Region::Common(text) => out.push_str(text),
            Region::Conflict(chunk) => {
                match choices.get(index).copied().flatten() {
                    None => out.push_str(&chunk.raw),
                    Some(Choice::Ours) => out.push_str(&chunk.ours),
                    Some(Choice::Theirs) => out.push_str(&chunk.theirs),
                    Some(Choice::OursThenTheirs) => {
                        out.push_str(&chunk.ours);
                        out.push_str(&chunk.theirs);
                    }
                    Some(Choice::TheirsThenOurs) => {
                        out.push_str(&chunk.theirs);
                        out.push_str(&chunk.ours);
                    }
                }
                index += 1;
            }
        }
    }
    out
}

/// Whether `text` still contains a conflict block.
pub fn has_markers(text: &str) -> bool {
    parse_markers(text).is_some()
}

/// Give every chunk its base (what the common ancestor had there) when git wrote the markers
/// without one — the default `merge.conflictStyle = merge`. The three index stages are merged
/// again with `git merge-file --zdiff3` (temp files in `scratch`, removed afterwards) and a
/// base is copied onto a working-copy chunk only when the re-merge produced a chunk with the
/// same ours and theirs text, in order; chunks edited by hand since simply stay without a base.
/// Never writes the working copy or the index.
pub async fn fill_bases(
    git: &Git,
    root: &Path,
    scratch: &Path,
    path: &str,
    regions: &mut [Region],
) -> Result<(), GitError> {
    let missing = regions
        .iter()
        .any(|region| matches!(region, Region::Conflict(chunk) if chunk.base.is_none()));
    if !missing {
        return Ok(());
    }
    let versions = conflict_versions(git, root, path).await?;
    let (Some(ours), Some(theirs)) = (&versions.ours, &versions.theirs) else {
        return Ok(());
    };
    tokio::fs::create_dir_all(scratch).await?;
    let files = [
        ("ours", ours.as_slice()),
        ("base", versions.base.as_deref().unwrap_or_default()),
        ("theirs", theirs.as_slice()),
    ];
    let mut paths = Vec::new();
    for (name, data) in files {
        let file = scratch.join(name);
        tokio::fs::write(&file, data).await?;
        paths.push(file.to_string_lossy().into_owned());
    }
    // `-p`: the result goes to stdout; the exit code is the number of conflicts.
    let output = git
        .command([
            "merge-file",
            "-p",
            "--zdiff3",
            "-L",
            "ours",
            "-L",
            "base",
            "-L",
        ])
        .args([
            "theirs",
            paths[0].as_str(),
            paths[1].as_str(),
            paths[2].as_str(),
        ])
        .current_dir(root)
        .background()
        .output()
        .await;
    let _ = tokio::fs::remove_dir_all(scratch).await;
    let output = output?;
    let merged = String::from_utf8_lossy(&output.stdout);
    let Some(merged) = parse_markers(&merged) else {
        return Ok(());
    };
    copy_bases(&merged, regions);
    Ok(())
}

/// Copy bases from `from` onto the chunks of `to` that have the same sides, in order. Line
/// endings are ignored when comparing (the index holds what autocrlf turned into LF).
fn copy_bases(from: &[Region], to: &mut [Region]) {
    let same = |a: &str, b: &str| {
        a.chars()
            .filter(|c| *c != '\r')
            .eq(b.chars().filter(|c| *c != '\r'))
    };
    let mut source = from.iter().filter_map(|region| match region {
        Region::Conflict(chunk) => chunk.base.as_ref().map(|base| (chunk, base)),
        Region::Common(_) => None,
    });
    for region in to.iter_mut() {
        let Region::Conflict(chunk) = region else {
            continue;
        };
        if chunk.base.is_some() {
            continue;
        }
        // Look ahead for the matching chunk; skip re-merged chunks that have no partner.
        let mut lookahead = source.clone();
        while let Some((other, base)) = lookahead.next() {
            if same(&other.ours, &chunk.ours) && same(&other.theirs, &chunk.theirs) {
                chunk.base = Some(base.clone());
                source = lookahead;
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MERGE: &str = "top\n<<<<<<< HEAD\nours 1\nours 2\n=======\ntheirs\n>>>>>>> topic\nmid\n\
                         <<<<<<< HEAD\na\n=======\nb\n>>>>>>> topic\n";
    const DIFF3: &str = "<<<<<<< ours\r\nmine\r\n||||||| base\r\nold\r\n=======\r\nyours\r\n\
                         >>>>>>> theirs\r\nend\r\n";

    #[test]
    fn merge_style_chunks() {
        let regions = parse_markers(MERGE).unwrap();
        assert_eq!(regions.len(), 4);
        let Region::Conflict(chunk) = &regions[1] else {
            panic!("expected a chunk")
        };
        assert_eq!(chunk.ours, "ours 1\nours 2\n");
        assert_eq!(chunk.theirs, "theirs\n");
        assert_eq!(
            (chunk.ours_label.as_str(), chunk.theirs_label.as_str()),
            ("HEAD", "topic")
        );
        assert_eq!(chunk.base, None);
    }

    #[test]
    fn diff3_style_keeps_base_and_crlf() {
        let regions = parse_markers(DIFF3).unwrap();
        let Region::Conflict(chunk) = &regions[0] else {
            panic!("expected a chunk")
        };
        assert_eq!(chunk.base.as_deref(), Some("old\r\n"));
        assert_eq!(chunk.ours, "mine\r\n");
        assert_eq!(
            apply_choices(&regions, &[Some(Choice::Theirs)]),
            "yours\r\nend\r\n"
        );
    }

    #[test]
    fn choices_resolve_in_order_and_unresolved_keep_markers() {
        let regions = parse_markers(MERGE).unwrap();
        let both = apply_choices(
            &regions,
            &[Some(Choice::OursThenTheirs), Some(Choice::Theirs)],
        );
        assert_eq!(both, "top\nours 1\nours 2\ntheirs\nmid\nb\n");
        let partial = apply_choices(&regions, &[None, Some(Choice::TheirsThenOurs)]);
        assert!(partial.starts_with("top\n<<<<<<< HEAD\n"));
        assert!(partial.ends_with("mid\nb\na\n"));
        assert!(has_markers(&partial));
        assert!(!has_markers(&both));
    }

    #[test]
    fn bases_are_copied_onto_matching_chunks_only() {
        let merged = parse_markers(
            "<<<<<<< ours\na\n||||||| base\nold a\n=======\nb\n>>>>>>> theirs\n\
             <<<<<<< ours\nx\n||||||| base\nold x\n=======\ny\n>>>>>>> theirs\n",
        )
        .unwrap();
        // The working copy: CRLF, the first chunk edited by hand, the second as git left it.
        let mut working = parse_markers(
            "<<<<<<< HEAD\r\nedited\r\n=======\r\nb\r\n>>>>>>> topic\r\n\
             <<<<<<< HEAD\r\nx\r\n=======\r\ny\r\n>>>>>>> topic\r\n",
        )
        .unwrap();
        copy_bases(&merged, &mut working);
        let bases: Vec<Option<&str>> = working
            .iter()
            .filter_map(|region| match region {
                Region::Conflict(chunk) => Some(chunk.base.as_deref()),
                Region::Common(_) => None,
            })
            .collect();
        assert_eq!(bases, [None, Some("old x\n")]);
    }

    #[test]
    fn malformed_or_absent_markers() {
        assert!(parse_markers("plain\ntext\n").is_none());
        assert!(parse_markers("<<<<<<< a\nx\n=======\ny\n").is_none());
        assert!(parse_markers("<<<<<<< a\nx\n<<<<<<< b\n").is_none());
        // Seven `=` inside ordinary text is not a marker outside a chunk.
        assert!(parse_markers("=======\n").is_none());
    }
}
