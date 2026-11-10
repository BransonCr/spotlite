//! The genre picker of Randomize: ten quick picks, a search, and a folder tree of about a hundred
//! genres and their sub-genres (rock › metal › doom metal). Every name here is one Spotify's
//! search knows as `genre:"…"`.

use std::collections::HashSet;

/// Shown first, before the tree.
pub const QUICK: [&str; 10] =
    ["pop", "hip hop", "rock", "indie rock", "electronic", "r&b", "metal", "jazz", "house", "k-pop"];

/// The tree, two spaces per level. A folder is a genre too: its first row inside randomizes it.
/// `name = term`: Spotify's search knows the genre by another name.
const TREE: &str = "\
rock
  classic rock
  hard rock
  psychedelic rock
  progressive rock
  garage rock
  alternative rock
    grunge
    emo
    post-grunge
  indie rock
    shoegaze
    dream pop
    post-rock
    math rock
  punk
    pop punk
    post-punk
    hardcore punk
    ska punk
  metal
    heavy metal
    thrash metal
    death metal
    black metal
    doom metal
    metalcore
    nu metal
    power metal
  j-rock
pop
  dance pop
  synthpop
  electropop
  indie pop
    bedroom pop
    chamber pop
  art pop
  hyperpop
  k-pop
  j-pop
  city pop
  latin pop
hip hop
  trap
    drill
    rage
  boom bap
  conscious hip hop
  gangster rap
  underground hip hop
  cloud rap
  emo rap
  phonk
    drift phonk
  lo-fi hip hop = lo-fi
  uk hip hop
electronic
  house
    deep house
    tech house
    progressive house
  techno
    minimal techno
    hard techno
  trance
  drum and bass = dnb
    liquid funk
    jungle
  dubstep
  uk garage
  breakbeat = breaks
  hardstyle
  future bass
  ambient
  idm
  synthwave
  vaporwave
  chillwave
r&b
  neo soul
  alternative r&b
  soul
  funk
  disco
  motown
jazz
  bebop
  cool jazz
  jazz fusion
  smooth jazz
  acid jazz
  swing
country
  bluegrass
  americana
  outlaw country
folk
  indie folk
  folk rock
  singer-songwriter
latin
  reggaeton
  salsa
  bachata
  cumbia
  bossa nova
reggae
  dub
  dancehall
  ska
blues
  delta blues
  chicago blues
  blues rock
classical
  baroque
  opera
  minimalism
  soundtrack
world
  afrobeats
  amapiano
  flamenco
  bollywood
";

#[derive(Debug, Clone)]
pub struct Genre {
    pub name: &'static str,
    pub depth: usize,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

/// All genres of the tree, in tree order.
pub fn all() -> Vec<Genre> {
    let mut out: Vec<Genre> = Vec::new();
    // The last genre seen at each depth: the parent of the next one a level deeper.
    let mut at_depth: Vec<usize> = Vec::new();
    for line in TREE.lines().filter(|l| !l.trim().is_empty()) {
        let depth = (line.len() - line.trim_start().len()) / 2;
        at_depth.truncate(depth);
        let parent = at_depth.last().copied();
        let id = out.len();
        let name = line.split(" = ").next().unwrap_or(line).trim();
        out.push(Genre { name, depth, parent, children: Vec::new() });
        if let Some(p) = parent {
            out[p].children.push(id);
        }
        at_depth.push(id);
    }
    out
}

/// What Spotify's search calls a genre: the name, except where the tree says `name = term`
/// ("drum and bass" is `dnb` there). Any other text stays as it is.
pub fn term(name: &str) -> String {
    TREE.lines()
        .filter_map(|l| l.trim().split_once(" = "))
        .find(|(n, _)| *n == name)
        .map_or_else(|| name.to_owned(), |(_, t)| t.to_owned())
}

/// "rock › metal › doom metal"
pub fn path(genres: &[Genre], id: usize) -> String {
    let mut parts = vec![genres[id].name];
    let mut at = genres[id].parent;
    while let Some(p) = at {
        parts.push(genres[p].name);
        at = genres[p].parent;
    }
    parts.reverse();
    parts.join(" › ")
}

/// One row of the picker.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// A section title; the cursor skips it.
    Header(&'static str),
    /// A quick pick.
    Quick(&'static str),
    /// A genre of the tree. A folder (it has sub-genres) opens and closes.
    Node(usize),
    /// Inside an open folder, first: the folder's own genre, all of it.
    All(usize),
    /// A search result.
    Match(usize),
    /// What was typed, when it is not a genre of the tree: any genre Spotify knows works.
    Typed(String),
}

impl Row {
    pub fn selectable(&self) -> bool {
        !matches!(self, Row::Header(_))
    }
}

/// The picker's state: the search text, the open folders, the cursor.
#[derive(Debug, Default)]
pub struct Picker {
    pub query: String,
    pub open: HashSet<usize>,
    pub cursor: usize,
}

/// What a row does when chosen.
#[derive(Debug, PartialEq)]
pub enum Choice {
    Randomize(String),
    /// A folder opened or closed.
    Toggled,
    Nothing,
}

impl Picker {
    pub fn new() -> Self {
        // The first quick pick, under its title.
        Picker { cursor: 1, ..Default::default() }
    }

    /// The rows shown now.
    pub fn rows(&self, genres: &[Genre]) -> Vec<Row> {
        let q = self.query.trim().to_lowercase();
        if !q.is_empty() {
            let mut matches: Vec<usize> = (0..genres.len()).filter(|&i| genres[i].name.contains(&q)).collect();
            // Names that start with the text first, then the shorter.
            matches.sort_by_key(|&i| (!genres[i].name.starts_with(&q), genres[i].name.len()));
            // The best match first; what was typed, as it is, after the matches.
            let mut rows: Vec<Row> = matches.into_iter().map(Row::Match).collect();
            if !genres.iter().any(|g| g.name == q) {
                rows.push(Row::Typed(q));
            }
            return rows;
        }
        let mut rows = vec![Row::Header("Popular")];
        rows.extend(QUICK.iter().map(|g| Row::Quick(g)));
        rows.push(Row::Header("All genres"));
        let mut stack: Vec<usize> = genres.iter().enumerate().filter(|(_, g)| g.parent.is_none()).map(|(i, _)| i).rev().collect();
        while let Some(i) = stack.pop() {
            rows.push(Row::Node(i));
            if self.open.contains(&i) {
                rows.push(Row::All(i));
                stack.extend(genres[i].children.iter().rev());
            }
        }
        rows
    }

    /// Moves the cursor by `delta` selectable rows.
    pub fn move_by(&mut self, genres: &[Genre], delta: isize) {
        let rows = self.rows(genres);
        let mut i = self.cursor.min(rows.len().saturating_sub(1)) as isize;
        let mut left = delta.abs();
        let step = delta.signum();
        while left > 0 {
            let next = i + step;
            if next < 0 || next >= rows.len() as isize {
                break;
            }
            i = next;
            if rows[i as usize].selectable() {
                left -= 1;
            }
        }
        // Never rest on a title.
        while (i as usize) < rows.len() && !rows[i as usize].selectable() {
            i += 1;
        }
        self.cursor = (i as usize).min(rows.len().saturating_sub(1));
    }

    /// The search changed: the cursor goes to the first result.
    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.cursor = if self.query.trim().is_empty() { 1 } else { 0 };
    }

    /// The genre of row `i`, if it is one (a folder is its own genre).
    pub fn genre_at(&self, genres: &[Genre], i: usize) -> Option<String> {
        match self.rows(genres).get(i)? {
            Row::Quick(g) => Some((*g).to_owned()),
            Row::Typed(g) => Some(g.clone()),
            Row::Node(g) | Row::All(g) | Row::Match(g) => Some(genres[*g].name.to_owned()),
            Row::Header(_) => None,
        }
    }

    /// ⏎ or a click on row `i`.
    pub fn choose(&mut self, genres: &[Genre], i: usize) -> Choice {
        let rows = self.rows(genres);
        self.cursor = i.min(rows.len().saturating_sub(1));
        match rows.get(i) {
            Some(Row::Quick(g)) => Choice::Randomize((*g).to_owned()),
            Some(Row::Typed(g)) => Choice::Randomize(g.clone()),
            Some(Row::All(g)) | Some(Row::Match(g)) => Choice::Randomize(genres[*g].name.to_owned()),
            Some(Row::Node(g)) if genres[*g].children.is_empty() => Choice::Randomize(genres[*g].name.to_owned()),
            Some(Row::Node(g)) => {
                if !self.open.remove(g) {
                    self.open.insert(*g);
                }
                Choice::Toggled
            }
            _ => Choice::Nothing,
        }
    }

    /// → opens the folder under the cursor; ← closes it, or the folder it is in.
    pub fn open_close(&mut self, genres: &[Genre], open: bool) {
        let rows = self.rows(genres);
        let (Some(Row::Node(g)) | Some(Row::All(g))) = rows.get(self.cursor).cloned() else { return };
        let is_all = matches!(rows.get(self.cursor), Some(Row::All(_)));
        if open {
            if !genres[g].children.is_empty() && !is_all {
                self.open.insert(g);
            }
            return;
        }
        // The folder to close: this one if it is open, else the one it is in.
        let target = if is_all || self.open.contains(&g) { Some(g) } else { genres[g].parent };
        if let Some(t) = target {
            self.open.remove(&t);
            if let Some(at) = self.rows(genres).iter().position(|r| *r == Row::Node(t)) {
                self.cursor = at;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_has_about_a_hundred_genres_and_no_doubles() {
        let g = all();
        assert!((95..=130).contains(&g.len()), "{}", g.len());
        let names: HashSet<_> = g.iter().map(|g| g.name).collect();
        assert_eq!(names.len(), g.len(), "a genre twice");
        let doom = g.iter().position(|g| g.name == "doom metal").unwrap();
        assert_eq!(path(&g, doom), "rock › metal › doom metal");
        assert!(QUICK.iter().all(|q| names.contains(q)), "quick picks are in the tree");
        assert_eq!(term("drum and bass"), "dnb");
        assert_eq!(term("shoegaze"), "shoegaze");
    }

    #[test]
    fn folders_open_and_randomize_their_own_genre() {
        let g = all();
        let mut p = Picker::new();
        assert_eq!(p.rows(&g)[p.cursor], Row::Quick("pop"));
        assert_eq!(p.choose(&g, 1), Choice::Randomize("pop".into()));
        // Open rock, then metal inside it.
        let rock = p.rows(&g).iter().position(|r| matches!(r, Row::Node(i) if g[*i].name == "rock")).unwrap();
        assert_eq!(p.choose(&g, rock), Choice::Toggled);
        let rows = p.rows(&g);
        assert!(matches!(rows[rock + 1], Row::All(i) if g[i].name == "rock"));
        assert_eq!(p.choose(&g, rock + 1), Choice::Randomize("rock".into()));
        let metal = p.rows(&g).iter().position(|r| matches!(r, Row::Node(i) if g[*i].name == "metal")).unwrap();
        p.cursor = metal;
        p.open_close(&g, true);
        let doom = p.rows(&g).iter().position(|r| matches!(r, Row::Node(i) if g[*i].name == "doom metal")).unwrap();
        assert_eq!(p.choose(&g, doom), Choice::Randomize("doom metal".into()));
        // ← on doom metal closes metal and goes to it.
        p.open_close(&g, false);
        assert!(matches!(p.rows(&g)[p.cursor], Row::Node(i) if g[i].name == "metal"));
        // The cursor skips titles.
        p.cursor = 1;
        p.move_by(&g, -1);
        assert_eq!(p.cursor, 1);
        p.cursor = 10;
        p.move_by(&g, 1);
        assert!(matches!(p.rows(&g)[p.cursor], Row::Node(_)));
    }

    #[test]
    fn search_finds_genres_and_takes_any_text() {
        let g = all();
        let mut p = Picker::new();
        p.set_query("phonk".into());
        let rows = p.rows(&g);
        assert_eq!(p.cursor, 0);
        assert!(matches!(rows[0], Row::Match(i) if g[i].name == "phonk"), "{rows:?}");
        assert!(rows.iter().any(|r| matches!(r, Row::Match(i) if g[*i].name == "drift phonk")));
        p.set_query("Jersey Club".into());
        assert_eq!(p.choose(&g, 0), Choice::Randomize("jersey club".into()));
    }
}
