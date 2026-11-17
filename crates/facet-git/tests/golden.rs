//! Golden tests: parsers run against byte-exact output captured from real git.
//!
//! Each fixture in `tests/golden/` was produced by the command noted beside it. To add a
//! parser, capture its real output into a new fixture and assert the full parsed value here.

use facet_git::parse::{parse_commit_summary, CommitSummary};

/// `git --no-pager -c color.ui=never log -1 -z --no-show-signature
///  --format=%H%x00%h%x00%an%x00%at%x00%s HEAD`
/// in a repo whose only commit has a non-ASCII subject and a body (the body must not leak in).
#[test]
fn log_commit_summary() {
    let raw = include_bytes!("golden/log_commit_summary.bin");
    assert_eq!(
        parse_commit_summary(raw).unwrap(),
        CommitSummary {
            oid: "6d447b2e771578b2779214d187215b9ab5865e4c".into(),
            short_oid: "6d447b2".into(),
            author: "Ada Lovelace".into(),
            author_time: 1_700_000_000,
            subject: "Add \"Niveau été\" lighting pass — 光".into(),
        }
    );
}
