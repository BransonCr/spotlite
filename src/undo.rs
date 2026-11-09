//! The undo tree of where you went: each tab or page you open is a node, a child of the place
//! you came from. Backspace goes back to the parent. Going somewhere new from an older place
//! starts a new branch, and the old branch stays: Ctrl+Backspace shows the whole tree, and a
//! click (or ⏎) goes to any node, even on another branch.

use crate::app::Loc;
use crate::state::SavedTab;
use std::time::Instant;

/// Nodes kept at most; past this the tree starts again from where you are.
const MAX_NODES: usize = 400;

#[derive(Clone, Debug)]
pub struct Node {
    pub loc: Loc,
    /// What the tree shows: a glyph and a name ("◉ Radiohead").
    pub label: String,
    /// What opens the page again if its tab was closed (None for the fixed tabs).
    pub reopen: Option<SavedTab>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// The last visit.
    pub at: Instant,
}

pub struct NavTree {
    pub nodes: Vec<Node>,
    pub cur: usize,
}

/// One row of the drawn tree, in the style of `git log --graph`: each branch has its own lane
/// (column); the node's dot sits in its lane, between `before` and `after`.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub node: usize,
    /// The lanes left of the dot ("│ ").
    pub before: String,
    /// The lanes right of the dot: "│" for a branch passing by, "─╯" where a branch
    /// that starts here ends ("─┴─╯" for two).
    pub after: String,
}

impl NavTree {
    pub fn new(loc: Loc, label: String) -> Self {
        NavTree { nodes: vec![Node { loc, label, reopen: None, parent: None, children: Vec::new(), at: Instant::now() }], cur: 0 }
    }

    #[cfg(test)]
    pub fn current(&self) -> &Node {
        &self.nodes[self.cur]
    }

    /// You went to `loc`: the child with that place if there is one, else a new child (a new
    /// branch when the current node has children already).
    pub fn visit(&mut self, loc: Loc, label: String, reopen: Option<SavedTab>) {
        if self.nodes[self.cur].loc == loc {
            self.nodes[self.cur].at = Instant::now();
            return;
        }
        let found = self.nodes[self.cur].children.iter().copied().find(|&c| self.nodes[c].loc == loc);
        let next = match found {
            Some(c) => {
                // The newest branch is drawn first.
                let kids = &mut self.nodes[self.cur].children;
                kids.retain(|&k| k != c);
                kids.push(c);
                c
            }
            None => {
                if self.nodes.len() >= MAX_NODES {
                    let cur = self.nodes[self.cur].clone();
                    *self = NavTree::new(cur.loc, cur.label);
                    self.nodes[0].reopen = cur.reopen;
                }
                let id = self.nodes.len();
                let parent = self.cur;
                self.nodes.push(Node { loc, label: label.clone(), reopen: reopen.clone(), parent: Some(parent), children: Vec::new(), at: Instant::now() });
                self.nodes[parent].children.push(id);
                id
            }
        };
        self.cur = next;
        self.nodes[next].at = Instant::now();
        self.nodes[next].label = label;
        if reopen.is_some() {
            self.nodes[next].reopen = reopen;
        }
    }

    /// Back: the nearest parent that `usable` accepts (its tab is open, or it can open again).
    /// Moves there and returns it; None at the root.
    pub fn back(&mut self, usable: impl Fn(&Node) -> bool) -> Option<&Node> {
        let mut at = self.nodes[self.cur].parent;
        while let Some(i) = at {
            if usable(&self.nodes[i]) {
                self.cur = i;
                self.nodes[i].at = Instant::now();
                return Some(&self.nodes[i]);
            }
            at = self.nodes[i].parent;
        }
        None
    }

    /// Moves to node `i` (from the tree view).
    pub fn jump(&mut self, i: usize) -> Option<&Node> {
        let node = self.nodes.get_mut(i)?;
        node.at = Instant::now();
        self.cur = i;
        Some(&self.nodes[i])
    }

    /// The tree as a graph, newest place at the top, like `git log --graph`. Each branch keeps
    /// one lane; where branches split, the older place shows `●─╯`. Lanes free up as branches
    /// end, so the graph is only as wide as the branches open at one time.
    pub fn rows(&self) -> Vec<Row> {
        // lanes[c]: the node lane c waits for (the parent of the last node drawn in it).
        let mut lanes: Vec<Option<usize>> = Vec::new();
        let mut out = Vec::new();
        // A child always comes after its parent in `nodes`, so newest first is children first.
        for id in (0..self.nodes.len()).rev() {
            let waiting: Vec<usize> = (0..lanes.len()).filter(|&c| lanes[c] == Some(id)).collect();
            let col = match waiting.first() {
                Some(&c) => c,
                // A branch tip: the first free lane.
                None => match lanes.iter().position(Option::is_none) {
                    Some(c) => c,
                    None => {
                        lanes.push(None);
                        lanes.len() - 1
                    }
                },
            };
            let joins = &waiting[waiting.len().min(1)..];
            let last_join = joins.last().copied();
            let mut before = String::new();
            for lane in &lanes[..col] {
                before.push_str(if lane.is_some() { "│ " } else { "  " });
            }
            let mut after = String::new();
            for (c, lane) in lanes.iter().enumerate().skip(col + 1) {
                let across = last_join.is_some_and(|l| c <= l);
                let (mark, gap) = if joins.contains(&c) {
                    (if Some(c) == last_join { "╯" } else { "┴" }, "─")
                } else if across {
                    (if lane.is_some() { "┼" } else { "─" }, "─")
                } else {
                    (if lane.is_some() { "│" } else { " " }, " ")
                };
                after.push_str(gap);
                after.push_str(mark);
            }
            out.push(Row { node: id, before, after: after.trim_end().to_owned() });
            for &c in joins {
                lanes[c] = None;
            }
            lanes[col] = self.nodes[id].parent;
            while lanes.last() == Some(&None) {
                lanes.pop();
            }
        }
        out
    }
}

/// "now", "40s", "5m", "2h": how long ago.
pub fn ago(at: Instant) -> String {
    let s = at.elapsed().as_secs();
    match s {
        0..=4 => "now".into(),
        5..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        _ => format!("{}h", s / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Tab;

    fn page(k: &str) -> Loc {
        Loc::Page(k.into())
    }

    #[test]
    fn back_then_somewhere_new_makes_a_branch() {
        let mut t = NavTree::new(Loc::Base(Tab::Search), "Search".into());
        t.visit(page("a"), "A".into(), None);
        t.visit(page("b"), "B".into(), None);
        assert_eq!(t.back(|_| true).map(|n| n.loc.clone()), Some(page("a")));
        t.visit(page("c"), "C".into(), None);
        // a has two branches; b is still there.
        let a = t.nodes[t.cur].parent.unwrap();
        assert_eq!(t.nodes[a].children.len(), 2);
        let rows = t.rows();
        let labels: Vec<_> = rows.iter().map(|r| format!("{}●{} {}", r.before, r.after, t.nodes[r.node].label)).collect();
        assert_eq!(labels, ["● C", "│ ● B", "●─╯ A", "● Search"]);
        // Jump to b on the other branch, then back goes to a.
        let b = rows.iter().find(|r| t.nodes[r.node].label == "B").unwrap().node;
        t.jump(b);
        assert_eq!(t.current().label, "B");
        assert_eq!(t.back(|_| true).map(|n| n.label.clone()), Some("A".into()));
    }

    #[test]
    fn lanes_pass_by_and_free_up() {
        // Search → a → b; back to a → c; back to Search → d → e.
        let mut t = NavTree::new(Loc::Base(Tab::Search), "S".into());
        t.visit(page("a"), "a".into(), None);
        t.visit(page("b"), "b".into(), None);
        t.back(|_| true);
        t.visit(page("c"), "c".into(), None);
        t.back(|_| true);
        t.back(|_| true);
        t.visit(page("d"), "d".into(), None);
        t.visit(page("e"), "e".into(), None);
        let lines: Vec<String> =
            t.rows().iter().map(|r| format!("{}●{} {}", r.before, r.after, t.nodes[r.node].label)).collect();
        assert_eq!(lines, ["● e", "● d", "│ ● c", "│ │ ● b", "│ ●─╯ a", "●─╯ S"]);
    }

    #[test]
    fn going_to_a_known_child_reuses_it() {
        let mut t = NavTree::new(Loc::Base(Tab::Search), "Search".into());
        t.visit(page("a"), "A".into(), None);
        t.back(|_| true);
        t.visit(page("a"), "A".into(), None);
        assert_eq!(t.nodes.len(), 2);
        t.visit(page("a"), "A".into(), None);
        assert_eq!(t.nodes.len(), 2, "the same place twice is one node");
    }

    #[test]
    fn back_skips_places_that_cannot_open() {
        let mut t = NavTree::new(Loc::Base(Tab::Search), "Search".into());
        t.visit(page("gone"), "Gone".into(), None);
        t.visit(page("b"), "B".into(), None);
        assert_eq!(t.back(|n| n.loc != page("gone")).map(|n| n.label.clone()), Some("Search".into()));
        assert!(t.back(|_| true).is_none(), "nothing above the root");
    }

    #[test]
    fn a_full_tree_starts_again_from_here() {
        let mut t = NavTree::new(Loc::Base(Tab::Search), "Search".into());
        for i in 0..MAX_NODES + 5 {
            t.visit(page(&i.to_string()), i.to_string(), None);
        }
        assert!(t.nodes.len() <= MAX_NODES);
        assert_eq!(t.current().label, (MAX_NODES + 4).to_string());
    }
}
