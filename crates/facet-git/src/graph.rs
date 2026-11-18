//! Lane assignment for the commit graph: a pure function of the commit list, computed
//! incrementally as pages of `git log --date-order` arrive (children always come before
//! their parents in that order).
//!
//! Each row describes what to draw in its own band, so the view never looks at other rows:
//! the commit's node, the lines from the top edge to the node or straight through, and the
//! lines from the node (or straight through) to the bottom edge.

use std::collections::HashMap;

use crate::oid::Oid;
use crate::parse::LogCommit;

/// One line segment within a row. Lanes are columns, left to right from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// Lane where the segment starts (top edge for [`Half::Top`], node height for
    /// [`Half::Bottom`]).
    pub from: u16,
    /// Lane where it ends (node height for [`Half::Top`], bottom edge for [`Half::Bottom`]).
    pub to: u16,
    pub half: Half,
    /// Color index; the view maps it onto its palette.
    pub color: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    /// Top edge → middle of the row.
    Top,
    /// Middle of the row → bottom edge.
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// Lane of the commit's node.
    pub lane: u16,
    pub color: u16,
    /// More than one parent.
    pub merge: bool,
    /// Boxed rather than a `Vec`: rows live as long as the history, so no spare capacity.
    pub segments: Box<[Segment]>,
    /// Lanes this row touches (the graph column is at least this wide here).
    pub width: u16,
}

/// An open lane (which commit it heads for is kept in `GraphBuilder::expected`).
#[derive(Debug, Clone)]
struct Lane {
    color: u16,
}

/// Incremental layout state; feed commits in log order.
#[derive(Debug, Default)]
pub struct GraphBuilder {
    lanes: Vec<Option<Lane>>,
    next_color: u16,
    /// Lanes by the commit they expect; one commit can be expected by several lanes when
    /// several children were drawn before it (they all converge on its node).
    expected: HashMap<Oid, Vec<u16>>,
}

impl GraphBuilder {
    fn new_color(&mut self) -> u16 {
        let color = self.next_color;
        self.next_color = self.next_color.wrapping_add(1);
        color
    }

    fn free_lane(&mut self) -> u16 {
        match self.lanes.iter().position(Option::is_none) {
            Some(free) => free as u16,
            None => {
                self.lanes.push(None);
                (self.lanes.len() - 1) as u16
            }
        }
    }

    fn occupy(&mut self, lane: u16, expects: &Oid, color: u16) {
        self.lanes[lane as usize] = Some(Lane { color });
        self.expected.entry(*expects).or_default().push(lane);
    }

    /// Lay out the next commit.
    pub fn push(&mut self, commit: &LogCommit) -> GraphRow {
        let mut segments = Vec::new();
        let mut matches = self.expected.remove(&commit.oid).unwrap_or_default();
        matches.sort_unstable();

        // The node goes in the leftmost lane that was waiting for it, or a new lane (a
        // branch tip nothing has pointed at yet).
        let (lane, color) = match matches.first() {
            Some(&first) => {
                let color = self.lanes[first as usize]
                    .as_ref()
                    .map_or(0, |lane| lane.color);
                (first, color)
            }
            None => {
                let lane = self.free_lane();
                (lane, self.new_color())
            }
        };

        // Top half: converging lanes run into the node, the rest straight down.
        for (index, slot) in self.lanes.iter().enumerate() {
            let Some(slot) = slot else { continue };
            let index = index as u16;
            let to = if matches.contains(&index) {
                lane
            } else {
                index
            };
            segments.push(Segment {
                from: index,
                to,
                half: Half::Top,
                color: slot.color,
            });
        }
        for &matched in &matches {
            self.lanes[matched as usize] = None;
        }

        // Bottom half: the first parent always continues in the node's lane (lines heading
        // for the same commit run side by side and converge on its row, which keeps a
        // branch's line straight). Further parents of a merge join the lane already heading
        // for them, or open a new one.
        let mut opened: Vec<u16> = Vec::new();
        for (number, parent) in commit.parents.iter().enumerate() {
            if number > 0 {
                let existing = self
                    .expected
                    .get(parent)
                    .and_then(|lanes| lanes.iter().min().copied());
                if let Some(existing) = existing {
                    let color = self.lanes[existing as usize]
                        .as_ref()
                        .map_or(color, |lane| lane.color);
                    segments.push(Segment {
                        from: lane,
                        to: existing,
                        half: Half::Bottom,
                        color,
                    });
                    continue;
                }
            }
            let (target, target_color) = if number == 0 {
                (lane, color)
            } else {
                // Lanes taken by earlier parents are occupied, so this is a fresh one.
                let free = self.free_lane();
                (free, self.new_color())
            };
            self.occupy(target, parent, target_color);
            opened.push(target);
            segments.push(Segment {
                from: lane,
                to: target,
                half: Half::Bottom,
                color: target_color,
            });
        }

        // Lanes that pass by (or that this commit joined) continue to the bottom edge.
        for (index, slot) in self.lanes.iter().enumerate() {
            let index = index as u16;
            if opened.contains(&index) {
                continue;
            }
            if let Some(slot) = slot {
                segments.push(Segment {
                    from: index,
                    to: index,
                    half: Half::Bottom,
                    color: slot.color,
                });
            }
        }

        while matches!(self.lanes.last(), Some(None)) {
            self.lanes.pop();
        }

        let width = segments
            .iter()
            .map(|segment| segment.from.max(segment.to))
            .chain(std::iter::once(lane))
            .max()
            .unwrap_or(0)
            + 1;
        GraphRow {
            lane,
            color,
            merge: commit.parents.len() > 1,
            segments: segments.into_boxed_slice(),
            width,
        }
    }

    /// Lanes still open (waiting for commits not yet seen).
    pub fn open_lanes(&self) -> usize {
        self.lanes.iter().filter(|lane| lane.is_some()).count()
    }
}

/// Lay out a whole list at once.
pub fn layout(commits: &[LogCommit]) -> Vec<GraphRow> {
    let mut builder = GraphBuilder::default();
    commits.iter().map(|commit| builder.push(commit)).collect()
}

/// A compact text picture of the layout, one line per row, for tests: `*` is the node,
/// `|` a lane passing by, `/` `\` lanes joining or leaving, `M` a merge node.
pub fn ascii(rows: &[GraphRow]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let width = row.width as usize;
            let mut cells = vec![' '; width];
            for segment in &row.segments {
                let (from, to) = (segment.from as usize, segment.to as usize);
                // Top half: from the lane at the top edge down to the node; bottom half: from
                // the node down to the lane at the bottom edge.
                let mark = match (segment.half, from.cmp(&to)) {
                    (_, std::cmp::Ordering::Equal) => '|',
                    (Half::Top, std::cmp::Ordering::Less)
                    | (Half::Bottom, std::cmp::Ordering::Less) => '\\',
                    _ => '/',
                };
                let at = if segment.half == Half::Top { from } else { to };
                if cells[at] == ' ' || cells[at] == '|' {
                    cells[at] = mark;
                }
            }
            cells[row.lane as usize] = if row.merge { 'M' } else { '*' };
            cells.into_iter().collect::<String>().trim_end().to_owned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(oid: &str, parents: &[&str]) -> LogCommit {
        LogCommit {
            oid: Oid::fake(oid),
            parents: crate::oid::Parents::from_vec(parents.iter().map(|p| Oid::fake(p)).collect()),
            author: "".into(),
            time: 0,
            subject: "".into(),
        }
    }

    #[test]
    fn a_linear_history_is_one_lane() {
        let rows = layout(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
        assert!(rows.iter().all(|row| row.lane == 0 && row.width == 1));
        assert!(rows.iter().all(|row| row.color == rows[0].color));
        // The root ends its lane: nothing continues below it.
        assert!(!rows[2].segments.iter().any(|s| s.half == Half::Bottom));
        assert_eq!(ascii(&rows), ["*", "*", "*"]);
    }

    #[test]
    fn a_merge_opens_a_lane_that_converges_on_the_fork_point() {
        //   M        merge of main (c) and topic (t2)
        //   | *      t2
        //   * |      c
        //   | *      t1
        //   *        b   (fork point: parent of c and t1)
        //   *        a
        let rows = layout(&[
            commit("m", &["c", "t2"]),
            commit("t2", &["t1"]),
            commit("c", &["b"]),
            commit("t1", &["b"]),
            commit("b", &["a"]),
            commit("a", &[]),
        ]);
        let lanes: Vec<u16> = rows.iter().map(|row| row.lane).collect();
        assert_eq!(lanes, [0, 1, 0, 1, 0, 0]);
        assert!(rows[0].merge);
        // The merge leaves towards lane 1 at the bottom.
        assert!(rows[0].segments.contains(&Segment {
            from: 0,
            to: 1,
            half: Half::Bottom,
            color: rows[1].color
        }));
        // The topic lane keeps its own color; main keeps the first one.
        assert_ne!(rows[1].color, rows[0].color);
        assert_eq!(rows[2].color, rows[0].color);
        // At the fork point lane 1 converges into lane 0 and closes.
        assert!(rows[4].segments.contains(&Segment {
            from: 1,
            to: 0,
            half: Half::Top,
            color: rows[1].color
        }));
        assert_eq!(rows[5].width, 1);
        assert_eq!(ascii(&rows), ["M\\", "|*", "*|", "|*", "*/", "*"]);
    }

    #[test]
    fn two_branch_tips_run_side_by_side_until_their_common_parent() {
        let rows = layout(&[
            commit("x", &["base"]),
            commit("y", &["base"]),
            commit("base", &[]),
        ]);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[1].lane, 1);
        assert_ne!(rows[0].color, rows[1].color);
        // Both lanes converge on the common parent, which sits in the leftmost one.
        assert_eq!(rows[2].lane, 0);
        assert!(rows[2].segments.contains(&Segment {
            from: 1,
            to: 0,
            half: Half::Top,
            color: rows[1].color
        }));
        assert_eq!(ascii(&rows), ["*", "|*", "*/"]);
    }

    #[test]
    fn a_merge_parent_already_on_screen_joins_its_lane() {
        // `m` merges `side`, which another tip (`s2`) also descends from.
        let rows = layout(&[
            commit("s2", &["side"]),
            commit("m", &["main", "side"]),
            commit("side", &["main"]),
            commit("main", &[]),
        ]);
        assert_eq!(rows[1].lane, 1);
        assert!(rows[1].segments.contains(&Segment {
            from: 1,
            to: 0,
            half: Half::Bottom,
            color: rows[0].color
        }));
        assert_eq!(rows[2].lane, 0);
        assert_eq!(rows[3].lane, 0);
    }

    #[test]
    fn freed_lanes_are_reused_and_octopus_merges_get_distinct_lanes() {
        let rows = layout(&[
            commit("o", &["a", "b", "c"]),
            commit("c", &["root"]),
            commit("b", &["root"]),
            commit("a", &["root"]),
            commit("root", &[]),
        ]);
        let lanes: Vec<u16> = rows.iter().map(|row| row.lane).collect();
        assert_eq!(lanes, [0, 2, 1, 0, 0]);
        let targets: Vec<u16> = rows[0]
            .segments
            .iter()
            .filter(|s| s.half == Half::Bottom)
            .map(|s| s.to)
            .collect();
        assert_eq!(targets, [0, 1, 2]);

        // A later tip reuses the leftmost free lane.
        let mut builder = GraphBuilder::default();
        for commit in [commit("p", &["q"]), commit("t", &["q"]), commit("q", &[])] {
            builder.push(&commit);
        }
        assert_eq!(builder.open_lanes(), 0);
        let row = builder.push(&commit("new-tip", &[]));
        assert_eq!(row.lane, 0);
    }

    #[test]
    fn incremental_and_whole_layouts_agree() {
        let commits = [
            commit("m", &["c", "t2"]),
            commit("t2", &["t1"]),
            commit("c", &["b"]),
            commit("t1", &["b"]),
            commit("b", &["a"]),
            commit("a", &[]),
        ];
        let whole = layout(&commits);
        let mut builder = GraphBuilder::default();
        let pieces: Vec<GraphRow> = commits.iter().map(|c| builder.push(c)).collect();
        assert_eq!(whole, pieces);
    }
}
