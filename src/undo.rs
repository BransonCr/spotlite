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

/// One row of the drawn tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub node: usize,
    /// The lines before the label: "│  ├─ ".
    pub prefix: String,
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

    /// The tree as rows from the root down, newest branch first, with the lines that join them.
    pub fn rows(&self) -> Vec<Row> {
        let mut out = vec![Row { node: 0, prefix: String::new() }];
        self.walk(0, "", &mut out);
        out
    }

    fn walk(&self, node: usize, indent: &str, out: &mut Vec<Row>) {
        let kids: Vec<usize> = self.nodes[node].children.iter().rev().copied().collect();
        for (i, &c) in kids.iter().enumerate() {
            let last = i + 1 == kids.len();
            out.push(Row { node: c, prefix: format!("{indent}{}", if last { "└─ " } else { "├─ " }) });
            self.walk(c, &format!("{indent}{}", if last { "   " } else { "│  " }), out);
        }
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
        let labels: Vec<_> = rows.iter().map(|r| format!("{}{}", r.prefix, t.nodes[r.node].label)).collect();
        assert_eq!(labels, ["Search", "└─ A", "   ├─ C", "   └─ B"]);
        // Jump to b on the other branch, then back goes to a.
        let b = rows.iter().find(|r| t.nodes[r.node].label == "B").unwrap().node;
        t.jump(b);
        assert_eq!(t.current().label, "B");
        assert_eq!(t.back(|_| true).map(|n| n.label.clone()), Some("A".into()));
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
