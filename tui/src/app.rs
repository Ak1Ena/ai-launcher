//! App state and behaviour: the home list of accounts, tabs of running CLIs, and failover
//! moving a tab's session to the next account when its claude stops on a rate limit.

use crate::config::{self, Entry, Kind, Paths};
use crate::failover::{self, Split};
use crate::form::{Confirm, Form, Outcome, Purpose};
use crate::pane::Pane;
use crate::ui::{Button, Hits};
use crate::usage::{self, Usage};
use crate::wrr;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const SWITCH_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq)]
pub enum Focus {
    /// the home list of accounts
    Home,
    /// the current tab's CLI
    Pane,
}

pub enum Notice {
    /// A limit was hit: move to `to` at `at` unless the user changes it.
    Switch {
        from: String,
        to: String,
        at: Instant,
    },
    /// A limit was hit and no member is left.
    NoneLeft {
        from: String,
    },
    /// The CLI ended on its own.
    Ended {
        code: u32,
    },
    Error(String),
}

pub struct Tab {
    pub member: String,
    pub pane: Option<Pane>,
    pub notice: Option<Notice>,
    handoff: PathBuf,
    tried: Vec<String>,
    split: Split,
    session: Option<(String, PathBuf)>,
    /// the pane's exit was already handled (the pane stays for its last screen)
    done: bool,
}

pub enum Modal {
    Form(Form),
    Confirm(Confirm),
}

pub struct Row {
    pub entry: Entry,
    /// 1-based failover priority, for members.
    pub priority: Option<usize>,
    pub on: bool,
    pub limited: Option<Duration>,
    pub usage: Usage,
    /// round-robin weight, for members
    pub weight: u32,
}

/// An Auto launch waiting for the model's pick.
pub struct Deciding {
    rx: std::sync::mpsc::Receiver<Option<String>>,
    cands: Vec<(String, u32)>,
    default: String,
}

pub struct App {
    pub paths: Paths,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub tabs: Vec<Tab>,
    pub current: usize,
    pub focus: Focus,
    pub failover: bool,
    /// weighted round-robin mode: Auto opens the next account by weight
    pub wrr: bool,
    pub deciding: Option<Deciding>,
    pub quit: bool,
    pub pane_size: (u16, u16),
    /// args `ai-launcher` was started with, for the first CLI opened
    first_args: Option<Vec<String>>,
    value_flags: Option<Vec<String>>,
    base_config: Option<PathBuf>,
    mod_dir: PathBuf,
    next_handoff: usize,
    pub modal: Option<Modal>,
    /// a short message under the home list, and when it goes away
    pub flash: Option<(String, Instant)>,
    /// a press started inside the pane: drags and release follow it outside
    pane_drag: bool,
    /// home row and time of the last click, for double-click to open
    last_click: Option<(usize, Instant)>,
    /// newer version found on npm
    pub update: Option<String>,
    update_rx: Option<std::sync::mpsc::Receiver<String>>,
}

impl App {
    pub fn new(args: Vec<String>) -> App {
        let paths = Paths::from_env();
        let rx = crate::update::spawn_check(paths.fo_state.parent().unwrap_or(&paths.fo_state));
        let mut app = App::with_paths(paths, args);
        app.update_rx = Some(rx);
        app
    }

    pub fn with_paths(paths: Paths, args: Vec<String>) -> App {
        let mod_dir = failover::install_mod(&paths);
        let mut app = App {
            failover: config::failover_on(&paths),
            wrr: false,
            deciding: None,
            base_config: std::env::var_os("CLAUDE_CONFIG_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            paths,
            rows: Vec::new(),
            selected: 0,
            tabs: Vec::new(),
            current: 0,
            focus: Focus::Home,
            quit: false,
            pane_size: (24, 80),
            first_args: Some(args),
            value_flags: None,
            mod_dir,
            next_handoff: 0,
            modal: None,
            flash: None,
            pane_drag: false,
            last_click: None,
            update: None,
            update_rx: None,
        };
        app.refresh();
        app
    }

    fn entries(&self) -> Vec<Entry> {
        self.rows.iter().map(|r| r.entry.clone()).collect()
    }

    fn entry(&self, id: &str) -> Option<Entry> {
        self.rows
            .iter()
            .find(|r| r.entry.id == id)
            .map(|r| r.entry.clone())
    }

    /// Re-reads config, order, limits and usage. Members come first, in priority order.
    pub fn refresh(&mut self) {
        let selected = self.rows.get(self.selected).map(|r| r.entry.id.clone());
        let entries = config::entries(&self.paths, &config::providers(&self.paths));
        let order = config::order(&self.paths, &entries);
        let weights = config::weights(&self.paths);
        let mut rows: Vec<Row> = order
            .iter()
            .enumerate()
            .filter_map(|(i, (id, on))| {
                let e = entries.iter().find(|e| &e.id == id)?.clone();
                let mut r = self.row(e, Some(i + 1), *on);
                r.weight = config::weight_of(&weights, id);
                Some(r)
            })
            .collect();
        for e in entries.into_iter().filter(|e| !e.in_failover()) {
            rows.push(self.row(e, None, false));
        }
        self.rows = rows;
        self.failover = config::failover_on(&self.paths);
        self.wrr = config::wrr_on(&self.paths);
        self.selected = selected
            .and_then(|id| self.rows.iter().position(|r| r.entry.id == id))
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
    }

    fn row(&self, entry: Entry, priority: Option<usize>, on: bool) -> Row {
        let usage = match entry.kind {
            Kind::Claude => {
                let base = self.base_config.as_deref();
                usage::claude(
                    &config::claude_dir(&self.paths, &entry, base),
                    &config::claude_json(&self.paths, &entry, base),
                )
            }
            Kind::Codex => usage::codex(
                &config::account_dir(&self.paths, &entry).unwrap_or(self.paths.codex_home.clone()),
            ),
            Kind::Other => Usage::default(),
        };
        let limited = priority.and_then(|_| config::limited_left(&self.paths, &entry.id));
        Row {
            entry,
            priority,
            on,
            limited,
            usage,
            weight: 1,
        }
    }

    fn order(&self) -> Vec<(String, bool)> {
        config::order(&self.paths, &self.entries())
    }

    pub fn label(&self, id: &str) -> String {
        match self.entry(id) {
            Some(e) if id.starts_with("claude/") => e.label,
            Some(e) => format!("{} ({id})", e.label),
            None => id.to_string(),
        }
    }

    // ── keys ──────────────────────────────────────────────

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.modal.is_some() {
            return self.modal_key(key);
        }
        // alt-1..9 and alt-← → switch tabs from anywhere, even while typing in a CLI
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Char('0') => {
                    self.focus = Focus::Home;
                    return;
                }
                KeyCode::Char(c @ '1'..='9') => return self.show_tab(c as usize - '1' as usize),
                KeyCode::Left if !self.tabs.is_empty() => {
                    return self.show_tab((self.current + self.tabs.len() - 1) % self.tabs.len());
                }
                KeyCode::Right if !self.tabs.is_empty() => {
                    return self.show_tab((self.current + 1) % self.tabs.len());
                }
                _ => {}
            }
        }
        // ctrl-\ moves between home and the last CLI tab
        // terminals send it as 0x1c, which crossterm reads as ctrl-4
        if matches!(key.code, KeyCode::Char('\\' | '4'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.focus = match self.focus {
                Focus::Pane => Focus::Home,
                Focus::Home if !self.tabs.is_empty() => Focus::Pane,
                f => f,
            };
            return;
        }
        match self.focus {
            Focus::Home => self.home_key(key),
            Focus::Pane => self.pane_key(key),
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        if let Some(m) = &mut self.modal {
            if let Modal::Form(f) = m {
                f.paste(text);
            }
            return;
        }
        if self.focus == Focus::Pane
            && let Some(p) = self.tabs.get(self.current).and_then(|t| t.pane.as_ref())
        {
            p.paste(text);
        }
    }

    fn home_key(&mut self, key: KeyEvent) {
        let id = self.rows.get(self.selected).map(|r| r.entry.id.clone());
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') if shift || key.code == KeyCode::Char('K') => {
                self.move_member(-1)
            }
            KeyCode::Down | KeyCode::Char('j') if shift || key.code == KeyCode::Char('J') => {
                self.move_member(1)
            }
            KeyCode::Char('K') => self.move_member(-1),
            KeyCode::Char('J') => self.move_member(1),
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1))
            }
            KeyCode::Enter => {
                if let Some(id) = id {
                    self.open(&id);
                }
            }
            KeyCode::Char(' ') => {
                if let Some(id) = id {
                    self.toggle_member(&id);
                }
            }
            KeyCode::Char('f') => config::set_failover(&self.paths, !self.failover),
            KeyCode::Char('w') => self.toggle_mode(),
            KeyCode::Char('n') => self.open_auto(),
            KeyCode::Char('m') => {
                self.modal = Some(Modal::Form(Form::model(&config::model(&self.paths))))
            }
            KeyCode::Char('+' | '=') => self.add_weight(1),
            KeyCode::Char('-' | '_') => self.add_weight(-1),
            KeyCode::Char('r') => {
                if let Some(id) = id {
                    config::clear_limit(&self.paths, &id);
                }
            }
            KeyCode::Char('x') => self.close_tab(),
            KeyCode::Char('a') => self.modal = Some(Modal::Form(Form::add())),
            KeyCode::Char('e') => self.start_edit(),
            KeyCode::Char('d') => self.start_delete(),
            KeyCode::Tab => {
                if !self.tabs.is_empty() {
                    self.current = (self.current + 1) % self.tabs.len();
                }
            }
            KeyCode::Char(c @ '1'..='9') => {
                let n = c as usize - '1' as usize;
                if n < self.tabs.len() {
                    self.current = n;
                    self.focus = Focus::Pane;
                }
            }
            _ => {}
        }
        self.refresh();
    }

    fn pane_key(&mut self, key: KeyEvent) {
        let Some(tab) = self.tabs.get_mut(self.current) else {
            return;
        };
        match (&tab.notice, key.code) {
            (Some(Notice::Switch { .. }), KeyCode::Enter) => self.switch_now(self.current),
            (Some(Notice::Switch { .. } | Notice::NoneLeft { .. }), KeyCode::Char('o')) => {
                if let Some(Notice::Switch { from, .. }) = &tab.notice {
                    tab.notice = Some(Notice::NoneLeft { from: from.clone() });
                }
                self.focus = Focus::Home;
            }
            (
                Some(Notice::Switch { .. } | Notice::NoneLeft { .. }),
                KeyCode::Char('q') | KeyCode::Esc,
            ) => {
                tab.notice = Some(Notice::Ended { code: 1 });
            }
            (Some(Notice::NoneLeft { .. }), KeyCode::Enter) => self.retry(self.current),
            // a failed switch: try the switch again, the session is still there
            (Some(Notice::Error(_)), KeyCode::Enter) if tab.done && tab.session.is_some() => {
                self.retry(self.current)
            }
            (Some(Notice::Ended { .. } | Notice::Error(_)), KeyCode::Enter) => {
                let member = tab.member.clone();
                self.tabs.remove(self.current);
                self.open(&member);
            }
            (Some(Notice::Ended { .. } | Notice::Error(_)), KeyCode::Char('x' | 'q')) => {
                self.close_tab()
            }
            (Some(_), _) => {}
            (None, _) => {
                if let Some(p) = &tab.pane {
                    p.key(key);
                }
            }
        }
    }

    /// Turns one account on or off for failover. Off accounts stay in the list but are skipped.
    fn toggle_member(&mut self, id: &str) {
        let mut o = self.order();
        if let Some(m) = o.iter_mut().find(|(m, _)| m == id) {
            m.1 = !m.1;
            let state = if m.1 {
                "on"
            } else {
                "off: skipped by failover"
            };
            config::save_order(&self.paths, &o);
            self.say(format!("{id} {state}"));
        }
        self.refresh();
    }

    fn toggle_mode(&mut self) {
        config::set_wrr(&self.paths, !self.wrr);
        self.say(if self.wrr {
            "Mode: failover."
        } else {
            "Round-robin: n or ▶ Auto opens next."
        });
    }

    fn add_weight(&mut self, by: i64) {
        let Some(r) = self
            .rows
            .get(self.selected)
            .filter(|r| r.priority.is_some())
        else {
            return;
        };
        let (id, w) = (
            r.entry.id.clone(),
            (r.weight as i64 + by).clamp(0, 99) as u32,
        );
        config::save_weight(&self.paths, &id, w);
        self.say(format!("{id} weight {w}"));
    }

    /// Whether a CLI tab fills the screen (otherwise home does).
    pub fn in_tab(&self) -> bool {
        self.focus == Focus::Pane && self.current < self.tabs.len()
    }

    /// Where tab `t` would fail over to next, from the rows (no file reads: this runs every frame).
    pub fn next_target(&self, t: usize) -> Option<&str> {
        let tried = &self.tabs.get(t)?.tried;
        self.rows
            .iter()
            .find(|r| {
                r.priority.is_some() && r.on && r.limited.is_none() && !tried.contains(&r.entry.id)
            })
            .map(|r| r.entry.id.as_str())
    }

    fn show_tab(&mut self, n: usize) {
        if n < self.tabs.len() {
            self.current = n;
            self.focus = Focus::Pane;
        }
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.flash = Some((text.into(), Instant::now() + Duration::from_secs(4)));
    }

    // ── add / edit / delete ───────────────────────────────

    fn start_edit(&mut self) {
        let Some(r) = self.rows.get(self.selected) else {
            return;
        };
        let e = &r.entry;
        let form = if e.id.starts_with("claude/") {
            let label = config::providers(&self.paths)
                .into_iter()
                .find(|p| p.name == "claude")
                .map(|p| p.label);
            Form::edit(
                "claude",
                &label.unwrap_or_else(|| "Claude Code".into()),
                &e.command,
                true,
            )
        } else {
            Form::edit(&e.id, &e.label, &e.command, false)
        };
        self.modal = Some(Modal::Form(form));
    }

    fn start_delete(&mut self) {
        let Some(r) = self.rows.get(self.selected) else {
            return;
        };
        let what = match r.entry.id.strip_prefix("claude/") {
            Some(acct) => format!(
                "Its folder moves to {}/.trash/{acct}-… and it is logged out (history and settings stay there).",
                self.paths.cla_root.display()
            ),
            None => "It leaves the list. A Codex or Gemini login folder is kept, so adding it again logs back in.".into(),
        };
        self.modal = Some(Modal::Confirm(Confirm {
            id: r.entry.id.clone(),
            what,
        }));
    }

    fn modal_key(&mut self, key: KeyEvent) {
        match self.modal.take() {
            Some(Modal::Confirm(c)) => match key.code {
                KeyCode::Char('y' | 'Y') => self.delete(&c.id),
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {}
                _ => self.modal = Some(Modal::Confirm(c)),
            },
            Some(Modal::Form(mut f)) => match f.key(key) {
                Outcome::Open => self.modal = Some(Modal::Form(f)),
                Outcome::Cancel => {}
                Outcome::Save => {
                    if let Err(e) = self.save_form(&f) {
                        f.error = Some(e);
                        self.modal = Some(Modal::Form(f));
                    }
                }
            },
            None => {}
        }
        self.refresh();
    }

    fn save_form(&mut self, f: &Form) -> Result<(), String> {
        let (label, command) = (f.get("Label"), f.get("Command"));
        match &f.purpose {
            Purpose::Model => {
                let model = f.get("Model");
                if !config::valid_model(&model) {
                    return Err("A model is one word and doesn't start with -.".into());
                }
                config::set_model(&self.paths, &model);
                self.say(if model.is_empty() {
                    "Auto uses plain round-robin.".to_string()
                } else {
                    format!("Auto asks {model} which account to use.")
                });
            }
            Purpose::Edit(id) => {
                if command.is_empty() {
                    return Err("The command can't be empty.".into());
                }
                config::put_provider(&self.paths, id, Some((&command, &label)))
                    .map_err(|e| e.to_string())?;
                self.say(format!("Saved {id}."));
            }
            Purpose::Add => {
                let name = f.get("Name");
                if !config::valid_name(&name) {
                    return Err("Name: one word of letters, digits, . _ or -".into());
                }
                if f.get("Type") == "Claude account" {
                    if self.paths.cla_root.join(&name).exists() {
                        return Err(format!("A Claude account named {name} already exists."));
                    }
                    config::add_claude_account(&self.paths, &name).map_err(|e| e.to_string())?;
                    self.refresh();
                    self.open_login(&format!("claude/{name}"));
                    return Ok(());
                }
                if command.is_empty() {
                    return Err("The command can't be empty.".into());
                }
                if config::providers(&self.paths)
                    .iter()
                    .any(|p| p.name == name)
                    || name == "claude"
                {
                    return Err(format!("{name} is already in the list."));
                }
                config::put_provider(&self.paths, &name, Some((&command, &label)))
                    .map_err(|e| e.to_string())?;
                let own_login = config::add_login_dir(&self.paths, &name, &command).is_some();
                self.refresh();
                if own_login {
                    self.open_login(&name);
                } else {
                    self.say(format!("Added {name}. Press enter on it to start."));
                }
            }
        }
        Ok(())
    }

    fn delete(&mut self, id: &str) {
        if self
            .tabs
            .iter()
            .any(|t| t.member == id && t.pane.is_some() && !t.done)
        {
            return self.say(format!("Close the running {id} tab first."));
        }
        let done = match id.strip_prefix("claude/") {
            Some(acct) => config::trash_claude_account(&self.paths, acct).map(|_| ()),
            None => config::put_provider(&self.paths, id, None),
        };
        match done {
            Ok(()) => self.say(format!("Deleted {id}.")),
            Err(e) => self.say(format!("Could not delete {id}: {e}")),
        }
        self.refresh();
    }

    /// A new account's first tab: its login (`claude auth login`, `codex login`).
    fn open_login(&mut self, id: &str) {
        let Some(entry) = self.entry(id) else { return };
        let mut env: Vec<(String, Option<String>)> = Vec::new();
        let command = match entry.kind {
            Kind::Claude => {
                let dir = config::claude_dir(&self.paths, &entry, self.base_config.as_deref());
                env.push((
                    "CLAUDE_CONFIG_DIR".into(),
                    Some(dir.to_string_lossy().into_owned()),
                ));
                format!("{} auth login", entry.command)
            }
            Kind::Codex => {
                if let Some(d) = config::account_dir(&self.paths, &entry) {
                    env.push(("CODEX_HOME".into(), Some(d.to_string_lossy().into_owned())));
                }
                format!("{} login", entry.command)
            }
            Kind::Other => {
                if config::agent(&entry.command) == "gemini"
                    && let Some(d) = config::account_dir(&self.paths, &entry)
                {
                    env.push((
                        "GEMINI_CLI_HOME".into(),
                        Some(d.to_string_lossy().into_owned()),
                    ));
                }
                entry.command.clone()
            }
        };
        self.next_handoff += 1;
        self.tabs.push(Tab {
            member: id.to_string(),
            pane: None,
            notice: None,
            handoff: self.paths.fo_state.join(format!(
                "handoff.{}.{}.json",
                std::process::id(),
                self.next_handoff
            )),
            tried: vec![id.to_string()],
            split: Split::default(),
            session: None,
            done: false,
        });
        self.current = self.tabs.len() - 1;
        self.focus = Focus::Pane;
        let (rows, cols) = self.pane_size;
        match Pane::spawn(&command, &[], &env, rows, cols) {
            Ok(p) => self.tabs[self.current].pane = Some(p),
            Err(e) => {
                self.tabs[self.current].notice =
                    Some(Notice::Error(format!("could not start {command}: {e}")))
            }
        }
        self.say(format!(
            "Log in to {} in this tab. When it ends, press enter to start it.",
            self.label(id)
        ));
    }

    // ── mouse ─────────────────────────────────────────────

    pub fn on_mouse(&mut self, m: MouseEvent, hits: &Hits) {
        let at = Position::new(m.column, m.row);
        let inside = |r: &Rect| r.contains(at);
        if self.modal.is_some() {
            if let (MouseEventKind::Down(MouseButton::Left), Some(Modal::Form(f))) =
                (m.kind, &mut self.modal)
                && let Some((_, i)) = hits.fields.iter().find(|(r, _)| inside(r))
                && (*i == f.fields.len() || !f.fields[*i].locked)
            {
                f.focus = *i;
                if *i == f.fields.len() {
                    self.modal_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                }
            }
            return;
        }
        if let MouseEventKind::Down(MouseButton::Left) = m.kind {
            if let Some((_, t)) = hits.closes.iter().find(|(r, _)| inside(r)) {
                self.current = *t;
                return self.close_tab();
            }
            if let Some((_, t)) = hits.tabs.iter().find(|(r, _)| inside(r)) {
                return self.show_tab(*t);
            }
            if let Some((_, b)) = hits.buttons.iter().find(|(r, _)| inside(r)) {
                match b {
                    Button::Add => self.modal = Some(Modal::Form(Form::add())),
                    Button::Edit => self.start_edit(),
                    Button::Delete => self.start_delete(),
                    Button::Failover => config::set_failover(&self.paths, !self.failover),
                    Button::Mode => self.toggle_mode(),
                    Button::Auto => self.open_auto(),
                    Button::Home => self.focus = Focus::Home,
                }
                return self.refresh();
            }
            if let Some((_, i)) = hits.switches.iter().find(|(r, _)| inside(r)) {
                self.selected = *i;
                if let Some(id) = self.rows.get(*i).map(|r| r.entry.id.clone()) {
                    self.toggle_member(&id);
                }
                return;
            }
            if let Some((_, i)) = hits.rows.iter().find(|(r, _)| inside(r)) {
                // one click selects, a second on the same row within 500 ms opens it
                let again = self
                    .last_click
                    .is_some_and(|(r, t)| r == *i && t.elapsed() < Duration::from_millis(500));
                self.last_click = (!again).then(|| (*i, Instant::now()));
                self.selected = *i;
                self.focus = Focus::Home;
                if again && let Some(id) = self.rows.get(*i).map(|r| r.entry.id.clone()) {
                    self.open(&id);
                }
                return;
            }
        }
        let up = matches!(m.kind, MouseEventKind::Up(_));
        let follows =
            self.pane_drag && matches!(m.kind, MouseEventKind::Drag(_) | MouseEventKind::Up(_));
        if matches!(m.kind, MouseEventKind::Down(_)) && inside(&hits.pane) {
            self.pane_drag = true;
        }
        if up {
            self.pane_drag = false;
        }
        if (inside(&hits.pane) || follows)
            && let Some(tab) = self.tabs.get(self.current)
        {
            if matches!(m.kind, MouseEventKind::Down(_)) {
                self.focus = Focus::Pane;
            }
            if tab.notice.is_none()
                && let Some(p) = &tab.pane
            {
                let pane = hits.pane;
                let col = m
                    .column
                    .clamp(pane.x, pane.right().saturating_sub(1).max(pane.x));
                let row = m
                    .row
                    .clamp(pane.y, pane.bottom().saturating_sub(1).max(pane.y));
                p.mouse(m.kind, col - pane.x, row - pane.y);
            }
        }
    }

    fn move_member(&mut self, by: isize) {
        let Some(id) = self.rows.get(self.selected).map(|r| r.entry.id.clone()) else {
            return;
        };
        let mut o = self.order();
        let Some(i) = o.iter().position(|(m, _)| *m == id) else {
            return;
        };
        let j = i as isize + by;
        if j < 0 || j as usize >= o.len() {
            return;
        }
        o.swap(i, j as usize);
        config::save_order(&self.paths, &o);
        self.refresh();
    }

    fn close_tab(&mut self) {
        if self.current < self.tabs.len() {
            self.tabs.remove(self.current);
            self.current = self.current.min(self.tabs.len().saturating_sub(1));
        }
        if self.tabs.is_empty() {
            self.focus = Focus::Home;
        }
    }

    // ── running CLIs ──────────────────────────────────────

    /// Members an Auto launch may pick, in priority order: on, weight above 0, not limited.
    fn auto_candidates(&self) -> Vec<(String, u32)> {
        self.rows
            .iter()
            .filter(|r| r.priority.is_some() && r.on && r.weight > 0 && r.limited.is_none())
            .map(|r| (r.entry.id.clone(), r.weight))
            .collect()
    }

    /// Opens the next account by weighted round-robin, or the one the routing model picks.
    pub fn open_auto(&mut self) {
        if !self.wrr {
            return self.say("Auto needs round-robin mode: press w.");
        }
        if self.deciding.is_some() {
            return;
        }
        let cands = self.auto_candidates();
        let Some((default, _)) = wrr::next(&cands, &wrr::load(&self.paths), None) else {
            return self.say("No account is on, weighted above 0 and free.");
        };
        let model = config::model(&self.paths);
        // the model runs on a Claude account that is free right now, the round-robin pick first
        let dir = std::iter::once(&default)
            .chain(cands.iter().map(|(m, _)| m))
            .filter_map(|m| self.entry(m))
            .find(|e| e.kind == Kind::Claude)
            .map(|e| config::claude_dir(&self.paths, &e, self.base_config.as_deref()));
        let (false, Some(dir)) = (model.is_empty(), dir) else {
            return self.auto_open(&cands, &default);
        };
        let info: Vec<(String, u32, String)> = cands
            .iter()
            .map(|(m, w)| {
                let usage = self
                    .rows
                    .iter()
                    .find(|r| &r.entry.id == m)
                    .map(|r| {
                        r.usage
                            .windows
                            .iter()
                            .map(|w| format!("{} {}%", w.label, w.percent))
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .filter(|u| !u.is_empty())
                    .unwrap_or_else(|| "unknown".into());
                (m.clone(), *w, usage)
            })
            .collect();
        self.say(format!("Asking {model} which account to use…"));
        self.deciding = Some(Deciding {
            rx: wrr::spawn_decide(
                model,
                dir.to_string_lossy().into_owned(),
                wrr::prompt(&info, &default),
            ),
            cands,
            default,
        });
    }

    fn auto_open(&mut self, cands: &[(String, u32)], pick: &str) {
        if let Some((_, s)) = wrr::next(cands, &wrr::load(&self.paths), Some(pick)) {
            wrr::save(&self.paths, &s);
        }
        self.open(pick);
        self.say(format!("Auto → {}", self.label(pick)));
    }

    /// Opens a new tab running the entry, with the args `ai-launcher` got the first time.
    pub fn open(&mut self, id: &str) {
        // the args ai-launcher got are claude args: only a Claude entry takes them
        let is_claude = self.entry(id).is_some_and(|e| e.kind == Kind::Claude);
        let args = if is_claude {
            self.first_args.take().unwrap_or_default()
        } else {
            Vec::new()
        };
        let flags = self
            .value_flags
            .get_or_insert_with(failover::value_flags)
            .clone();
        let split = failover::split_args(&args, &flags);
        self.next_handoff += 1;
        let handoff = self.paths.fo_state.join(format!(
            "handoff.{}.{}.json",
            std::process::id(),
            self.next_handoff
        ));
        self.tabs.push(Tab {
            member: id.to_string(),
            pane: None,
            notice: None,
            handoff,
            tried: vec![id.to_string()],
            split,
            session: None,
            done: false,
        });
        self.current = self.tabs.len() - 1;
        self.focus = Focus::Pane;
        self.launch(self.current, args);
    }

    fn launch(&mut self, t: usize, args: Vec<String>) {
        let id = self.tabs[t].member.clone();
        let Some(entry) = self.entry(&id) else {
            self.tabs[t].notice = Some(Notice::Error(format!("{id} is no longer in the list")));
            return;
        };
        let failover = self.failover && entry.kind == Kind::Claude;
        let mut env: Vec<(String, Option<String>)> = Vec::new();
        let mut command = entry.command.clone();
        match entry.kind {
            Kind::Claude => {
                let cfg = if id.starts_with("claude/") {
                    Some(config::claude_dir(&self.paths, &entry, None))
                } else {
                    self.base_config.clone()
                };
                env.push((
                    "CLAUDE_CONFIG_DIR".into(),
                    cfg.map(|p| p.to_string_lossy().into_owned()),
                ));
                if failover {
                    let _ = std::fs::create_dir_all(&self.paths.fo_state);
                    let _ = std::fs::remove_file(&self.tabs[t].handoff);
                    command = format!(
                        "{command} --plugin-dir {}",
                        shell_quote(&self.mod_dir.to_string_lossy())
                    );
                    let next =
                        failover::next_member(&self.paths, &self.entries(), &self.tabs[t].tried);
                    env.push((
                        "AI_FO_HANDOFF".into(),
                        Some(self.tabs[t].handoff.to_string_lossy().into_owned()),
                    ));
                    env.push(("AI_FO_MEMBER".into(), Some(self.label(&id))));
                    env.push(("AI_FO_NEXT".into(), next.map(|n| self.label(&n))));
                }
            }
            Kind::Codex => {
                if let Some(d) = config::account_dir(&self.paths, &entry) {
                    env.push(("CODEX_HOME".into(), Some(d.to_string_lossy().into_owned())));
                }
            }
            Kind::Other => {
                if config::agent(&entry.command) == "gemini"
                    && let Some(d) = config::account_dir(&self.paths, &entry)
                {
                    env.push((
                        "GEMINI_CLI_HOME".into(),
                        Some(d.to_string_lossy().into_owned()),
                    ));
                }
            }
        }
        let (rows, cols) = self.pane_size;
        match Pane::spawn(&command, &args, &env, rows, cols) {
            Ok(p) => {
                self.tabs[t].pane = Some(p);
                self.tabs[t].notice = None;
                self.tabs[t].done = false;
            }
            Err(e) => {
                self.tabs[t].notice = Some(Notice::Error(format!("could not start {command}: {e}")))
            }
        }
    }

    /// Called every frame: notices CLIs that ended and runs due switches.
    pub fn tick(&mut self) {
        if let Some(v) = self.update_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.say(format!("Update v{v}: npm i -g ai-launcher"));
            self.update = Some(v);
            self.update_rx = None;
        }
        // the model's answer; a dead thread counts as no answer
        let reply = self.deciding.as_ref().and_then(|d| match d.rx.try_recv() {
            Ok(r) => Some(r),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(None),
        });
        if let Some(reply) = reply
            && let Some(d) = self.deciding.take()
        {
            let pick = wrr::choose(reply.as_deref(), &d.cands, &d.default);
            self.auto_open(&d.cands, &pick);
        }
        if self
            .flash
            .as_ref()
            .is_some_and(|(_, until)| Instant::now() > *until)
        {
            self.flash = None;
        }
        for t in 0..self.tabs.len() {
            if let Some(Notice::Switch { at, .. }) = &self.tabs[t].notice {
                if Instant::now() >= *at {
                    self.switch_now(t);
                }
                continue;
            }
            if self.tabs[t].done {
                continue;
            }
            let Some(code) = self.tabs[t].pane.as_mut().and_then(Pane::exited) else {
                continue;
            };
            self.tabs[t].done = true;
            let handoff = failover::read_handoff(&self.tabs[t].handoff);
            let _ = std::fs::remove_file(&self.tabs[t].handoff);
            match handoff {
                Some(h) if self.failover => {
                    config::mark_limited(&self.paths, &self.tabs[t].member);
                    self.tabs[t].session = h.session_id.zip(h.transcript);
                    self.retry(t);
                    self.refresh();
                }
                _ => {
                    if self.tabs[t].notice.is_none() {
                        self.tabs[t].notice = Some(Notice::Ended { code });
                    }
                }
            }
        }
    }

    /// Picks the next member for a tab whose account hit its limit.
    fn retry(&mut self, t: usize) {
        let from = self.label(&self.tabs[t].member);
        let next = failover::next_member(&self.paths, &self.entries(), &self.tabs[t].tried);
        self.tabs[t].notice = Some(match next {
            Some(to) => Notice::Switch {
                from,
                to,
                at: Instant::now() + SWITCH_WAIT,
            },
            None => Notice::NoneLeft { from },
        });
    }

    fn switch_now(&mut self, t: usize) {
        if !matches!(self.tabs[t].notice, Some(Notice::Switch { .. })) {
            return;
        }
        self.tabs[t].notice = None;
        if !self.failover {
            self.tabs[t].notice = Some(Notice::Ended { code: 1 });
            return;
        }
        // the order, on/off and limits may have changed while the notice was up
        let Some(to) = failover::next_member(&self.paths, &self.entries(), &self.tabs[t].tried)
        else {
            return self.retry(t);
        };
        let Some(entry) = self.entry(&to) else {
            return self.retry(t);
        };
        let tab = &mut self.tabs[t];
        let args = match (entry.kind, &tab.session) {
            (Kind::Claude, Some((sid, transcript))) => {
                let dir = config::claude_dir(&self.paths, &entry, self.base_config.as_deref());
                if let Err(e) = failover::move_transcript(transcript, &dir) {
                    tab.notice = Some(Notice::Error(format!(
                        "could not move the session to {to}: {e}"
                    )));
                    return;
                }
                let mut a = tab.split.keep.clone();
                a.extend([
                    "--resume".to_string(),
                    sid.clone(),
                    failover::CONTINUE.to_string(),
                ]);
                a
            }
            (Kind::Codex, Some((_, transcript))) => vec![format!(
                "{} Below is the end of the Claude Code transcript (full file: {}).\n\n{}",
                failover::CONTINUE,
                transcript.display(),
                failover::handoff_text(transcript)
            )],
            // no transcript yet: start over with the prompt
            _ => tab.split.prompt.clone().into_iter().collect(),
        };
        tab.member = to.clone();
        tab.tried.push(to);
        self.launch(t, args);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(1), cols.max(1));
        self.pane_size = (rows, cols);
        for t in &mut self.tabs {
            if let Some(p) = &mut t.pane {
                p.resize(rows, cols);
            }
        }
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    /// An app over a fresh temp folder: claude account `w`, other CLIs `t1` and `t2` (command `true`).
    fn app() -> App {
        let home = std::env::temp_dir().join(format!(
            "ai-launcher-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&home);
        let cla_root = home.join("cla");
        fs::create_dir_all(cla_root.join("w")).unwrap();
        let providers = home.join("cfg/providers");
        fs::create_dir_all(providers.parent().unwrap()).unwrap();
        fs::write(
            &providers,
            "claude\tclaude\tClaude Code\nt1\ttrue\tT One\nt2\ttrue\tT Two\n",
        )
        .unwrap();
        let paths = Paths {
            fo_order: home.join("cfg/failover"),
            fo_state: home.join("state"),
            ai_accounts: home.join("accts"),
            codex_home: home.join("codex"),
            cooldown: Duration::from_secs(60),
            providers,
            cla_root,
            home,
        };
        let mut a = App::with_paths(paths, vec![]);
        a.value_flags = Some(vec![]); // don't run `claude --help`
        a
    }

    #[test]
    fn draws_at_any_size_without_panicking() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut a = app();
        a.rows[0].usage = Usage {
            windows: vec![
                usage::Window {
                    label: "5h".into(),
                    percent: 8,
                    resets_at: Some(1_700_000_000),
                    reset: Some("Tue 12:59".into()),
                },
                usage::Window {
                    label: "7d".into(),
                    percent: 100,
                    ..Default::default()
                },
            ],
            plan: Some("max".into()),
            saved_at: Some(1_600_000_000),
            email: Some("全角@example.com".into()),
        };
        let draw_all = |a: &App| {
            for (w, h) in [(20, 5), (30, 10), (40, 10), (120, 40)] {
                let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
                t.draw(|f| {
                    crate::ui::draw(f, a);
                })
                .unwrap();
            }
        };
        draw_all(&a);
        a.wrr = true;
        a.update = Some("9.9.9".into());
        draw_all(&a);
        a.open("t1");
        draw_all(&a);
        a.focus = Focus::Home;
        draw_all(&a);
    }

    #[test]
    fn home_hits_match_what_is_drawn_and_nothing_overlaps() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut a = app();
        a.wrr = true;
        a.open("t1");
        a.focus = Focus::Home;
        for (w, h) in [(30, 10), (80, 24), (130, 20)] {
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            let mut hits = Hits::default();
            t.draw(|f| hits = crate::ui::draw(f, &a)).unwrap();
            let buf = t.backend().buffer();
            let text = |r: &Rect| -> String {
                (r.x..r.right())
                    .map(|x| buf[(x, r.y)].symbol().to_string())
                    .collect()
            };
            assert!(!hits.switches.is_empty(), "{w}x{h}");
            for (r, i) in &hits.switches {
                let on = a.rows[*i].on;
                assert_eq!(text(r), if on { " ● on  " } else { " ○ off " }, "{w}x{h}");
            }
            let mut drawn: Vec<Rect> = hits.buttons.iter().map(|(r, _)| *r).collect();
            drawn.extend(hits.tabs.iter().map(|(r, _)| *r));
            for (n, r) in drawn.iter().enumerate() {
                assert!(r.right() <= w, "{w}x{h}: {r:?} off screen");
                for o in &drawn[n + 1..] {
                    assert!(!r.intersects(*o), "{w}x{h}: {r:?} overlaps {o:?}");
                }
            }
            assert!(hits.pane.is_empty(), "home has no pane");
            // clicking `✻ home` from a tab comes back here
            let home = hits
                .buttons
                .iter()
                .find(|(_, b)| *b == Button::Home)
                .unwrap()
                .0;
            a.focus = Focus::Pane;
            click(&mut a, &hits, home.x, home.y);
            assert!(a.focus == Focus::Home);
        }
    }

    #[test]
    fn ctrl_backslash_and_alt_0_go_home_and_back() {
        let mut a = app();
        a.open("t1");
        a.open("t2");
        a.on_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT));
        a.on_key(KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::CONTROL));
        assert!(a.focus == Focus::Home && !a.in_tab());
        a.on_key(KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::CONTROL));
        assert!(a.in_tab() && a.current == 0);
        a.on_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::ALT));
        assert!(!a.in_tab());
    }

    fn key(a: &mut App, code: KeyCode) {
        a.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn typed(a: &mut App, s: &str) {
        s.chars().for_each(|c| key(a, KeyCode::Char(c)));
    }

    /// Opens the add form as "Other CLI" with name and command typed in (focus ends on Command).
    fn fill_add(a: &mut App, name: &str, command: &str) {
        key(a, KeyCode::Char('a'));
        (0..3).for_each(|_| key(a, KeyCode::Right));
        key(a, KeyCode::Tab);
        typed(a, name);
        key(a, KeyCode::Tab);
        key(a, KeyCode::Tab);
        typed(a, command);
    }

    fn save(a: &mut App) {
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
    }

    fn form_error(a: &App) -> Option<String> {
        match &a.modal {
            Some(Modal::Form(f)) => f.error.clone(),
            _ => None,
        }
    }

    fn providers_text(a: &App) -> String {
        fs::read_to_string(&a.paths.providers).unwrap()
    }

    fn ids(a: &App) -> Vec<String> {
        a.rows.iter().map(|r| r.entry.id.clone()).collect()
    }

    fn select(a: &mut App, id: &str) {
        a.selected = ids(a).iter().position(|i| i == id).unwrap();
    }

    fn click(a: &mut App, hits: &Hits, x: u16, y: u16) {
        a.on_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            },
            hits,
        );
    }

    fn r(y: u16) -> Rect {
        Rect::new(0, y, 5, 1)
    }

    #[test]
    fn add_other_cli_writes_the_providers_line() {
        let mut a = app();
        fill_add(&mut a, "x", "true");
        save(&mut a);
        assert!(a.modal.is_none());
        assert!(providers_text(&a).contains("x\ttrue\tx\n"));
        assert!(ids(&a).contains(&"x".to_string()));
    }

    #[test]
    fn add_duplicate_name_keeps_the_form_open_with_an_error() {
        let mut a = app();
        fill_add(&mut a, "t1", "true");
        save(&mut a);
        assert!(form_error(&a).unwrap().contains("already"));
        assert_eq!(providers_text(&a).matches("t1\t").count(), 1);
    }

    #[test]
    fn add_invalid_name_keeps_the_form_open_with_an_error() {
        let mut a = app();
        fill_add(&mut a, "a/b", "true");
        save(&mut a);
        assert!(form_error(&a).is_some());
        assert!(!providers_text(&a).contains("a/b"));
    }

    #[test]
    fn edit_changes_label_and_command_in_place() {
        let mut a = app();
        select(&mut a, "t1");
        key(&mut a, KeyCode::Char('e')); // focus starts on Label
        a.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        typed(&mut a, "New");
        key(&mut a, KeyCode::Tab);
        a.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        typed(&mut a, "false");
        save(&mut a);
        assert!(a.modal.is_none());
        let text = providers_text(&a);
        assert!(text.contains("t1\tfalse\tNew\n"), "{text}");
        let lines: Vec<_> = text.lines().collect();
        assert!(lines[1].starts_with("t1\t") && lines[2].starts_with("t2\t"));
    }

    #[test]
    fn delete_provider_leaves_the_file() {
        let mut a = app();
        select(&mut a, "t1");
        key(&mut a, KeyCode::Char('d'));
        key(&mut a, KeyCode::Char('y'));
        assert!(!providers_text(&a).contains("t1\t"));
        assert!(!ids(&a).contains(&"t1".to_string()));
    }

    #[test]
    fn delete_claude_account_moves_it_to_trash() {
        let mut a = app();
        select(&mut a, "claude/w");
        key(&mut a, KeyCode::Char('d'));
        key(&mut a, KeyCode::Char('y'));
        assert!(!a.paths.cla_root.join("w").exists());
        let trashed: Vec<_> = fs::read_dir(a.paths.cla_root.join(".trash"))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(
            trashed
                .iter()
                .any(|e| e.file_name().to_string_lossy().starts_with("w-"))
        );
        assert!(!ids(&a).contains(&"claude/w".to_string()));
    }

    #[test]
    fn delete_declined_keeps_everything() {
        let mut a = app();
        select(&mut a, "claude/w");
        key(&mut a, KeyCode::Char('d'));
        key(&mut a, KeyCode::Char('n'));
        assert!(a.modal.is_none());
        assert!(a.paths.cla_root.join("w").exists());
        assert!(ids(&a).contains(&"claude/w".to_string()));
    }

    #[test]
    fn click_row_selects_then_opens_a_tab() {
        let mut a = app();
        let i = ids(&a).iter().position(|i| i == "t1").unwrap();
        let hits = Hits {
            rows: vec![(r(1), 0), (r(2), 1), (r(3), 2)],
            ..Hits::default()
        };
        click(&mut a, &hits, 1, 1 + i as u16);
        assert_eq!(a.selected, i);
        assert!(a.tabs.is_empty());
        click(&mut a, &hits, 1, 1 + i as u16);
        assert_eq!(a.tabs.len(), 1);
        assert_eq!(a.tabs[0].member, "t1");
        assert!(a.focus == Focus::Pane);
    }

    #[test]
    fn single_click_on_keyboard_selected_row_does_not_open() {
        let mut a = app();
        let i = ids(&a).iter().position(|i| i == "t1").unwrap();
        a.selected = i;
        let hits = Hits {
            rows: vec![(r(1), 0), (r(2), 1), (r(3), 2)],
            ..Hits::default()
        };
        click(&mut a, &hits, 1, 1 + i as u16);
        assert!(a.tabs.is_empty());
        a.last_click = Some((i, Instant::now() - Duration::from_millis(600)));
        click(&mut a, &hits, 1, 1 + i as u16);
        assert!(a.tabs.is_empty());
    }

    #[test]
    fn drag_and_release_outside_pane_stay_with_the_pane() {
        let mut a = app();
        a.open("t1");
        let hits = Hits {
            pane: Rect::new(10, 2, 20, 10),
            ..Hits::default()
        };
        let ev = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        a.on_mouse(ev(MouseEventKind::Down(MouseButton::Left), 12, 3), &hits);
        assert!(a.pane_drag);
        a.on_mouse(ev(MouseEventKind::Drag(MouseButton::Left), 0, 0), &hits);
        assert!(a.pane_drag);
        a.on_mouse(ev(MouseEventKind::Up(MouseButton::Left), 0, 0), &hits);
        assert!(!a.pane_drag);
    }

    #[test]
    fn delete_confirms_on_y_only() {
        let mut a = app();
        a.start_delete();
        a.modal_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(a.modal, Some(Modal::Confirm(_))));
        a.modal_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(a.modal.is_none());
    }

    #[test]
    fn click_tab_switches_and_close_closes() {
        let mut a = app();
        a.open("t1");
        a.open("t2");
        assert_eq!(a.current, 1);
        let hits = Hits {
            tabs: vec![(r(0), 0), (Rect::new(10, 0, 5, 1), 1)],
            closes: vec![(Rect::new(6, 0, 1, 1), 0), (Rect::new(16, 0, 1, 1), 1)],
            ..Hits::default()
        };
        click(&mut a, &hits, 1, 0);
        assert_eq!(a.current, 0);
        click(&mut a, &hits, 16, 0);
        assert_eq!(a.tabs.len(), 1);
        assert_eq!(a.tabs[0].member, "t1");
    }

    #[test]
    fn click_failover_button_toggles_it() {
        let mut a = app();
        let hits = Hits {
            buttons: vec![(r(5), Button::Failover)],
            ..Hits::default()
        };
        assert!(!a.failover);
        click(&mut a, &hits, 1, 5);
        assert!(a.failover);
        click(&mut a, &hits, 1, 5);
        assert!(!a.failover);
    }

    #[test]
    fn click_account_switch_turns_it_off_and_on() {
        let mut a = app();
        let i = a.rows.iter().position(|r| r.priority.is_some()).unwrap();
        let id = a.rows[i].entry.id.clone();
        let hits = Hits {
            switches: vec![(r(3), i)],
            ..Hits::default()
        };
        assert!(a.rows[i].on);
        click(&mut a, &hits, 1, 3);
        let row = a.rows.iter().find(|r| r.entry.id == id).unwrap();
        assert!(!row.on, "one click turns it off");
        assert!(a.tabs.is_empty(), "the switch never opens the account");
        click(&mut a, &hits, 1, 3);
        assert!(a.rows.iter().find(|r| r.entry.id == id).unwrap().on);
    }

    #[test]
    fn click_form_field_focuses_and_save_saves() {
        let mut a = app();
        fill_add(&mut a, "x", "true");
        let hits = Hits {
            fields: (0..5).map(|i| (r(i as u16), i)).collect(),
            ..Hits::default()
        };
        click(&mut a, &hits, 1, 1);
        let Some(Modal::Form(f)) = &a.modal else {
            panic!("form closed")
        };
        assert_eq!(f.focus, 1);
        click(&mut a, &hits, 1, 4); // Save
        assert!(a.modal.is_none());
        assert!(providers_text(&a).contains("x\ttrue\tx\n"));
    }

    /// `app()` plus claude accounts `x` and `y` (members: w, x, y in that order).
    fn wrr_app() -> App {
        let a = app();
        for n in ["x", "y"] {
            fs::create_dir_all(a.paths.cla_root.join(n)).unwrap();
        }
        // `true` instead of claude: Auto opens tabs, and tests must not start the real CLI
        fs::write(&a.paths.providers, "claude\ttrue\tClaude Code\n").unwrap();
        let mut a = App::with_paths(a.paths.clone(), vec![]);
        a.value_flags = Some(vec![]);
        a
    }

    #[test]
    fn wrr_keys_and_candidates_skip_off_limited_and_zero_weight() {
        let mut a = wrr_app();
        key(&mut a, KeyCode::Char('w'));
        assert!(a.wrr);
        select(&mut a, "claude/w");
        key(&mut a, KeyCode::Char('+'));
        key(&mut a, KeyCode::Char('+'));
        assert_eq!(config::weights(&a.paths), vec![("claude/w".into(), 3)]);
        select(&mut a, "claude/x");
        key(&mut a, KeyCode::Char('-'));
        key(&mut a, KeyCode::Char('-'));
        assert_eq!(a.rows[a.selected].weight, 0, "never below 0");
        assert_eq!(
            a.auto_candidates(),
            vec![("claude/w".into(), 3), ("claude/y".into(), 1)]
        );
        config::mark_limited(&a.paths, "claude/w");
        a.toggle_member("claude/y");
        assert!(a.auto_candidates().is_empty());
        a.open_auto();
        assert!(a.tabs.is_empty());
        let order = fs::read_to_string(&a.paths.fo_order).unwrap();
        assert!(order.lines().all(|l| l.split('\t').count() == 2), "{order}");
    }

    #[test]
    fn auto_opens_by_round_robin_and_keeps_state() {
        let mut a = wrr_app();
        a.open_auto();
        assert!(a.tabs.is_empty(), "failover mode: Auto does nothing");
        config::set_wrr(&a.paths, true);
        config::save_weight(&a.paths, "claude/x", 0);
        a.refresh();
        for _ in 0..3 {
            a.open_auto();
        }
        let opened: Vec<_> = a.tabs.iter().map(|t| t.member.as_str()).collect();
        assert_eq!(opened, ["claude/w", "claude/y", "claude/w"]);
        // a restart continues the rotation
        let mut b = App::with_paths(a.paths.clone(), vec![]);
        b.value_flags = Some(vec![]);
        b.open_auto();
        assert_eq!(b.tabs[0].member, "claude/y");
    }

    #[test]
    fn model_reply_picks_or_falls_back_to_round_robin() {
        let mut a = wrr_app();
        config::set_wrr(&a.paths, true);
        a.refresh();
        let cands = a.auto_candidates();
        for (reply, want) in [
            (Some("claude/y\n"), "claude/y"),
            (Some("no idea"), "claude/w"),
            (None, "claude/w"),
        ] {
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(reply.map(str::to_string)).unwrap();
            a.deciding = Some(Deciding {
                rx,
                cands: cands.clone(),
                default: "claude/w".into(),
            });
            a.tick();
            assert!(a.deciding.is_none());
            assert_eq!(a.tabs.last().unwrap().member, want);
        }
        // a thread that died without answering counts as no answer
        let (tx, rx) = std::sync::mpsc::channel::<Option<String>>();
        drop(tx);
        a.deciding = Some(Deciding {
            rx,
            cands,
            default: "claude/x".into(),
        });
        a.tick();
        assert_eq!(a.tabs.last().unwrap().member, "claude/x");
    }

    #[test]
    fn model_form_saves_and_clears_the_model() {
        let mut a = app();
        key(&mut a, KeyCode::Char('m'));
        typed(&mut a, "haiku");
        save(&mut a);
        assert_eq!(config::model(&a.paths), "haiku");
        key(&mut a, KeyCode::Char('m'));
        a.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        save(&mut a);
        assert!(config::model(&a.paths).is_empty());
    }

    #[test]
    fn alt_keys_switch_tabs_from_the_pane() {
        let mut a = app();
        a.open("t1");
        a.open("t2");
        let alt = |a: &mut App, code| a.on_key(KeyEvent::new(code, KeyModifiers::ALT));
        a.focus = Focus::Pane;
        alt(&mut a, KeyCode::Char('1'));
        assert_eq!(a.current, 0);
        alt(&mut a, KeyCode::Char('2'));
        assert_eq!(a.current, 1);
        alt(&mut a, KeyCode::Left);
        assert_eq!(a.current, 0);
        alt(&mut a, KeyCode::Left); // wraps
        assert_eq!(a.current, 1);
        alt(&mut a, KeyCode::Right); // wraps
        assert_eq!(a.current, 0);
        assert!(a.focus == Focus::Pane);
    }
}
