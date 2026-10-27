//! A plugin's tabbed menu (`plugin menu`, README "Plugins"): list, radio and checklist tabs in
//! one window. Keys only; ui.rs draws it. What Enter runs comes from the window's own copy of
//! the menu, never from a list refreshed while it is open.

use crate::plugins::{plain, valid_id};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    List,
    Radio,
    /// weight: rows carry a weight (`+`/`-`); else a position (shift-↑↓ reorders)
    Checklist {
        weight: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub label: String,
    /// the radio's current choice
    pub on: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub key: String,
    pub label: String,
    pub checked: bool,
    /// position or weight; None: not placed (e.g. not linked yet), so it never moves
    pub n: Option<u32>,
    pub note: String,
    pub on: Option<String>,
    pub off: Option<String>,
    /// order: move up / down; weight: weight -1 / +1
    pub dec: Option<String>,
    pub inc: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub kind: Kind,
    pub title: String,
    pub items: Vec<Item>,
    pub rows: Vec<Check>,
}

pub const MAX_WEIGHT: u32 = 99;

/// `plugin menu` output. Lines: `tab<TAB>list|radio|checklist<TAB><title>[<TAB>order|weight]`,
/// then `item<TAB><id><TAB><label>[<TAB>on]` (list, radio) or
/// `row<TAB><key><TAB><label><TAB>1|0<TAB><n|-><TAB><note><TAB><on><TAB><off><TAB><dec><TAB><inc>`
/// (checklist; `-` = none). Bad lines are skipped; tabs left empty are dropped.
pub fn parse(text: &str) -> Vec<Tab> {
    let mut tabs: Vec<Tab> = Vec::new();
    // false after a bad `tab` line: its lines belong to nothing
    let mut open = false;
    let id = |s: &str| match s {
        "-" => Some(None),
        s if valid_id(s) => Some(Some(s.to_string())),
        _ => None,
    };
    for line in text.lines().map(|l| l.trim_end_matches('\r')) {
        let f: Vec<&str> = line.split('\t').collect();
        match f.as_slice() {
            ["tab", kind, title, rest @ ..] => {
                let kind = match (*kind, rest) {
                    ("list", []) => Some(Kind::List),
                    ("radio", []) => Some(Kind::Radio),
                    ("checklist", [] | ["order"]) => Some(Kind::Checklist { weight: false }),
                    ("checklist", ["weight"]) => Some(Kind::Checklist { weight: true }),
                    _ => None,
                };
                open = kind.is_some();
                if let Some(kind) = kind {
                    tabs.push(Tab {
                        kind,
                        title: plain(title),
                        items: Vec::new(),
                        rows: Vec::new(),
                    });
                }
            }
            ["item", i, label, rest @ ..] if open && valid_id(i) => {
                let t = tabs.last_mut().unwrap();
                if t.kind != (Kind::List) && t.kind != Kind::Radio {
                    continue;
                }
                let on = match rest {
                    [] => false,
                    ["on"] => true,
                    _ => continue,
                };
                t.items.push(Item {
                    id: i.to_string(),
                    label: plain(label),
                    on: on && t.kind == Kind::Radio,
                });
            }
            ["row", key, label, c, n, note, on, off, dec, inc] if open && valid_id(key) => {
                let t = tabs.last_mut().unwrap();
                let Kind::Checklist { weight } = t.kind else {
                    continue;
                };
                let checked = match *c {
                    "1" => true,
                    "0" => false,
                    _ => continue,
                };
                let n = match *n {
                    "-" => None,
                    n => match n.parse::<u32>() {
                        Ok(v) => Some(if weight { v.min(MAX_WEIGHT) } else { v }),
                        Err(_) => continue,
                    },
                };
                let (Some(on), Some(off), Some(dec), Some(inc)) =
                    (id(on), id(off), id(dec), id(inc))
                else {
                    continue;
                };
                if t.rows.iter().any(|r| r.key == *key) {
                    continue;
                }
                t.rows.push(Check {
                    key: key.to_string(),
                    label: plain(label),
                    checked,
                    n,
                    note: plain(note),
                    on,
                    off,
                    dec,
                    inc,
                });
            }
            _ => {}
        }
    }
    tabs.retain(|t| !t.items.is_empty() || !t.rows.is_empty());
    tabs
}

pub enum Outcome {
    Open,
    Cancel,
    /// run these `do` ids in this order
    Run(Vec<String>),
}

pub struct Menu {
    pub plugin: String,
    /// the menu as it was when the window opened: the ids Enter runs come from here
    pub tabs: Vec<Tab>,
    /// each checklist's rows as the user has edited them (other tabs: empty)
    pub now: Vec<Vec<Check>>,
    pub tab: usize,
    /// the cursor of each tab
    pub sel: Vec<usize>,
}

impl Menu {
    pub fn new(plugin: &str, tabs: Vec<Tab>) -> Menu {
        Menu {
            plugin: plugin.to_string(),
            now: tabs.iter().map(|t| t.rows.clone()).collect(),
            // a radio tab starts on its current choice, so Enter alone changes nothing
            sel: tabs
                .iter()
                .map(|t| t.items.iter().position(|i| i.on).unwrap_or(0))
                .collect(),
            tab: 0,
            tabs,
        }
    }

    fn len(&self) -> usize {
        let t = &self.tabs[self.tab];
        t.items.len() + t.rows.len()
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let t = self.tab;
        let kind = self.tabs[t].kind;
        let sel = self.sel[t];
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let n = self.tabs.len();
        match key.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Left | KeyCode::BackTab => self.tab = (t + n - 1) % n,
            KeyCode::Right | KeyCode::Tab => self.tab = (t + 1) % n,
            KeyCode::Up if shift => self.move_row(-1),
            KeyCode::Down if shift => self.move_row(1),
            KeyCode::Char('K') => self.move_row(-1),
            KeyCode::Char('J') => self.move_row(1),
            KeyCode::Up | KeyCode::Char('k') => self.sel[t] = sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.sel[t] = (sel + 1).min(self.len().saturating_sub(1))
            }
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('+' | '=') => self.weigh(true),
            KeyCode::Char('-') => self.weigh(false),
            KeyCode::Enter => {
                return match kind {
                    Kind::List | Kind::Radio => match self.tabs[t].items.get(sel) {
                        Some(i) => Outcome::Run(vec![i.id.clone()]),
                        None => Outcome::Open,
                    },
                    Kind::Checklist { .. } => {
                        let ids = self.plan(t);
                        if ids.is_empty() {
                            Outcome::Cancel
                        } else {
                            Outcome::Run(ids)
                        }
                    }
                };
            }
            _ => {}
        }
        Outcome::Open
    }

    fn orig(&self, t: usize, key: &str) -> &Check {
        self.tabs[t].rows.iter().find(|r| r.key == key).unwrap()
    }

    /// space: tick = use (or link, for a row not placed yet), untick = don't use. Only to a
    /// state the plugin has an id for, or back to where it started.
    fn toggle(&mut self) {
        let (t, i) = (self.tab, self.sel[self.tab]);
        let Some(r) = self.now[t].get(i) else { return };
        let o = self.orig(t, &r.key);
        let to = !r.checked;
        if to == o.checked || (if to { &o.on } else { &o.off }).is_some() {
            self.now[t][i].checked = to;
        }
    }

    fn weigh(&mut self, up: bool) {
        let (t, i) = (self.tab, self.sel[self.tab]);
        if self.tabs[t].kind != (Kind::Checklist { weight: true }) {
            return;
        }
        let Some(r) = self.now[t].get(i) else { return };
        let o = self.orig(t, &r.key);
        let Some(w) = r.n else { return };
        let to = if up {
            (w + 1).min(MAX_WEIGHT)
        } else {
            w.saturating_sub(1)
        };
        // the step the plugin must take to get there from where it started
        let need = if to > o.n.unwrap_or(0) {
            &o.inc
        } else {
            &o.dec
        };
        if Some(to) == o.n || need.is_some() {
            self.now[t][i].n = Some(to);
        }
    }

    /// shift-↑↓: swaps the row with its neighbour, both placed (rows not placed never move).
    fn move_row(&mut self, by: isize) {
        let (t, i) = (self.tab, self.sel[self.tab]);
        if self.tabs[t].kind != (Kind::Checklist { weight: false }) {
            return;
        }
        let j = i as isize + by;
        let rows = &mut self.now[t];
        if j < 0 || j as usize >= rows.len() {
            return;
        }
        let j = j as usize;
        if rows[i].n.is_none() || rows[j].n.is_none() {
            return;
        }
        rows.swap(i, j);
        self.sel[t] = j;
    }

    /// The `do` ids that turn the menu as it was into the edited one: toggles of rows not
    /// placed yet (links) first, then the other toggles, then moves, then weights.
    pub fn plan(&self, t: usize) -> Vec<String> {
        let (orig, now) = (&self.tabs[t].rows, &self.now[t]);
        let mut ids: Vec<String> = Vec::new();
        for placed in [false, true] {
            for r in now {
                let o = self.orig(t, &r.key);
                if o.n.is_some() == placed && r.checked != o.checked {
                    ids.extend(if r.checked { &o.on } else { &o.off }.clone());
                }
            }
        }
        match self.tabs[t].kind {
            Kind::Checklist { weight: false } => {
                // adjacent swaps from the old order to the new: each row moves up into place
                let mut seq: Vec<&str> = orig
                    .iter()
                    .filter(|r| r.n.is_some())
                    .map(|r| r.key.as_str())
                    .collect();
                let want = now.iter().filter(|r| r.n.is_some());
                for (i, r) in want.enumerate() {
                    let mut j = seq.iter().position(|k| *k == r.key).unwrap();
                    let up = &self.orig(t, &r.key).dec;
                    while j > i {
                        ids.extend(up.clone());
                        seq.swap(j - 1, j);
                        j -= 1;
                    }
                }
            }
            Kind::Checklist { weight: true } => {
                for r in now {
                    let o = self.orig(t, &r.key);
                    let (Some(a), Some(b)) = (o.n, r.n) else {
                        continue;
                    };
                    let (step, k) = if b > a {
                        (&o.inc, b - a)
                    } else {
                        (&o.dec, a - b)
                    };
                    for _ in 0..k {
                        ids.extend(step.clone());
                    }
                }
            }
            _ => {}
        }
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDER: &str = "tab\tlist\tActions\n\
        item\tstart\tstart proxy\n\
        tab\tradio\tMode\n\
        item\tmode-failover\tfailover (in your order)\ton\n\
        item\tmode-wrr\tweighted round-robin\n\
        tab\tchecklist\tAccounts\torder\n\
        row\tc1\ta@x\t1\t1\tclaude\ton.c1\toff.c1\tup.c1\tdown.c1\n\
        row\tc2\tb@x\t1\t2\tclaude\ton.c2\toff.c2\tup.c2\tdown.c2\n\
        row\tc3\tc@x\t0\t3\tcodex\ton.c3\toff.c3\tup.c3\tdown.c3\n\
        row\tclaude-k\tk@x\t0\t-\tclaude · not linked\tlink.claude-k\t-\t-\t-\n";

    const WEIGHT: &str = "tab\tchecklist\tAccounts\tweight\n\
        row\tc1\ta@x\t1\t1\tclaude\ton.c1\toff.c1\twdown.c1\twup.c1\n\
        row\tc2\tb@x\t1\t98\tclaude\ton.c2\toff.c2\twdown.c2\twup.c2\n";

    fn press(m: &mut Menu, code: KeyCode) -> Outcome {
        m.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn shift(m: &mut Menu, code: KeyCode) {
        m.key(KeyEvent::new(code, KeyModifiers::SHIFT));
    }

    fn ran(o: Outcome) -> Vec<String> {
        match o {
            Outcome::Run(ids) => ids,
            _ => panic!("nothing to run"),
        }
    }

    fn accounts(text: &str) -> Menu {
        let tabs = parse(text);
        let mut m = Menu::new("p", tabs);
        m.tab = m.tabs.len() - 1;
        m
    }

    #[test]
    fn parse_reads_tabs_items_and_rows_and_skips_junk() {
        let text = format!(
            "junk before\nitem\torphan\tx\n{ORDER}\
             row\tbad id\tx\t1\t1\t\ton.x\t-\t-\t-\n\
             row\tc4\tx\t2\t1\t\ton.x\t-\t-\t-\n\
             row\tc5\tx\t1\t1\t\t../x\t-\t-\t-\n\
             row\tc1\tdup\t1\t1\t\t-\t-\t-\t-\n\
             tab\tweird\tNope\nitem\tlost\tx\n\
             tab\tlist\tEmpty\n"
        );
        let t = parse(&text);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].kind, Kind::List);
        assert_eq!(t[0].items.len(), 1);
        assert_eq!(
            (t[1].kind, t[1].items[0].on, t[1].items[1].on),
            (Kind::Radio, true, false)
        );
        assert_eq!(t[2].kind, Kind::Checklist { weight: false });
        let keys: Vec<&str> = t[2].rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["c1", "c2", "c3", "claude-k"]);
        let k = &t[2].rows[3];
        assert_eq!(
            (k.n, k.off.clone(), k.on.as_deref()),
            (None, None, Some("link.claude-k"))
        );
        let w = parse("tab\tchecklist\tA\tweight\nrow\tc\tx\t1\t150\t\t-\t-\t-\t-\n");
        assert_eq!(w[0].rows[0].n, Some(MAX_WEIGHT));
        assert!(parse("").is_empty());
    }

    #[test]
    fn radio_starts_on_its_current_choice() {
        let wrr = ORDER
            .replace("(in your order)\ton\n", "(in your order)\n")
            .replace("round-robin\n", "round-robin\ton\n");
        let mut m = Menu::new("p", parse(&wrr));
        press(&mut m, KeyCode::Right);
        assert_eq!(ran(press(&mut m, KeyCode::Enter)), ["mode-wrr"]);
    }

    #[test]
    fn list_and_radio_run_the_chosen_id() {
        let mut m = Menu::new("p", parse(ORDER));
        assert_eq!(ran(press(&mut m, KeyCode::Enter)), ["start"]);
        press(&mut m, KeyCode::Right);
        press(&mut m, KeyCode::Down);
        assert_eq!(ran(press(&mut m, KeyCode::Enter)), ["mode-wrr"]);
        press(&mut m, KeyCode::Left);
        press(&mut m, KeyCode::Left);
        assert_eq!(m.tab, 2, "tabs wrap around");
    }

    #[test]
    fn toggles_run_links_first_then_on_off() {
        let mut m = accounts(ORDER);
        press(&mut m, KeyCode::Char(' ')); // c1 off
        for _ in 0..2 {
            press(&mut m, KeyCode::Down);
        }
        press(&mut m, KeyCode::Char(' ')); // c3 on
        press(&mut m, KeyCode::Down);
        press(&mut m, KeyCode::Char(' ')); // link k
        assert_eq!(
            ran(press(&mut m, KeyCode::Enter)),
            ["link.claude-k", "off.c1", "on.c3"]
        );
    }

    #[test]
    fn an_unlinked_row_can_be_ticked_and_unticked_but_not_moved() {
        let mut m = accounts(ORDER);
        m.sel[m.tab] = 3;
        press(&mut m, KeyCode::Char(' '));
        press(&mut m, KeyCode::Char(' '));
        assert!(!m.now[m.tab][3].checked);
        shift(&mut m, KeyCode::Up);
        assert_eq!(m.now[m.tab][3].key, "claude-k");
        assert!(matches!(press(&mut m, KeyCode::Enter), Outcome::Cancel));
    }

    #[test]
    fn reorder_to_any_order_becomes_up_steps() {
        let mut m = accounts(ORDER);
        // c1 c2 c3 → c3 c1 c2 → c3 c2 c1
        m.sel[m.tab] = 2;
        shift(&mut m, KeyCode::Up);
        shift(&mut m, KeyCode::Up);
        press(&mut m, KeyCode::Down);
        press(&mut m, KeyCode::Char('J'));
        let keys: Vec<&str> = m.now[m.tab].iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["c3", "c2", "c1", "claude-k"]);
        let ids = ran(press(&mut m, KeyCode::Enter));
        // replay the steps on the old order: they must reach the new one
        let mut seq = vec!["c1", "c2", "c3"];
        for id in &ids {
            let k = id.strip_prefix("up.").unwrap();
            let j = seq.iter().position(|x| *x == k).unwrap();
            seq.swap(j - 1, j);
        }
        assert_eq!(seq, ["c3", "c2", "c1"]);
        assert_eq!(ids, ["up.c3", "up.c3", "up.c2"]);
        // the last placed row can't go below an unplaced one
        m.sel[m.tab] = 2;
        shift(&mut m, KeyCode::Down);
        assert_eq!(m.now[m.tab][2].key, "c1");
    }

    #[test]
    fn weights_step_and_clamp() {
        let mut m = accounts(WEIGHT);
        for _ in 0..3 {
            press(&mut m, KeyCode::Char('-'));
        }
        assert_eq!(m.now[0][0].n, Some(0), "never below 0");
        press(&mut m, KeyCode::Down);
        for _ in 0..3 {
            press(&mut m, KeyCode::Char('+'));
        }
        assert_eq!(m.now[0][1].n, Some(MAX_WEIGHT), "never above 99");
        assert_eq!(ran(press(&mut m, KeyCode::Enter)), ["wdown.c1", "wup.c2"]);
        // order keys do nothing in a weight list
        shift(&mut m, KeyCode::Up);
        assert_eq!(m.now[0][1].key, "c2");
    }

    #[test]
    fn esc_runs_nothing_and_no_change_means_nothing_to_run() {
        let mut m = accounts(ORDER);
        press(&mut m, KeyCode::Char(' '));
        assert!(matches!(press(&mut m, KeyCode::Esc), Outcome::Cancel));
        let mut m = accounts(ORDER);
        press(&mut m, KeyCode::Char(' '));
        press(&mut m, KeyCode::Char(' '));
        assert!(matches!(press(&mut m, KeyCode::Enter), Outcome::Cancel));
    }

    #[test]
    fn a_missing_id_blocks_that_change() {
        let mut m = accounts("tab\tchecklist\tA\tweight\nrow\tc\tx\t1\t5\t\t-\t-\t-\twup.c\n");
        press(&mut m, KeyCode::Char(' '));
        press(&mut m, KeyCode::Char('-'));
        assert_eq!((m.now[0][0].checked, m.now[0][0].n), (true, Some(5)));
        press(&mut m, KeyCode::Char('+'));
        press(&mut m, KeyCode::Char('-'));
        assert_eq!(
            m.now[0][0].n,
            Some(5),
            "back to the start is always allowed"
        );
    }
}
