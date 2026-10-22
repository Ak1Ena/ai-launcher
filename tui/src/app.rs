//! App state and behaviour: the sidebar of accounts, tabs of running CLIs, and failover
//! moving a tab's session to the next account when its claude stops on a rate limit.

use crate::config::{self, Entry, Kind, Paths};
use crate::failover::{self, Split};
use crate::form::{Confirm, Form, Outcome, Purpose};
use crate::pane::Pane;
use crate::ui::{Button, Hits};
use crate::usage::{self, Usage};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const SWITCH_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq)]
pub enum Focus {
    Sidebar,
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
}

pub struct App {
    pub paths: Paths,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub tabs: Vec<Tab>,
    pub current: usize,
    pub focus: Focus,
    pub failover: bool,
    pub quit: bool,
    pub pane_size: (u16, u16),
    /// args `ai-launcher` was started with, for the first CLI opened
    first_args: Option<Vec<String>>,
    value_flags: Option<Vec<String>>,
    base_config: Option<PathBuf>,
    mod_dir: PathBuf,
    next_handoff: usize,
    pub modal: Option<Modal>,
    /// a short message under the sidebar, and when it goes away
    pub flash: Option<(String, Instant)>,
}

impl App {
    pub fn new(args: Vec<String>) -> App {
        App::with_paths(Paths::from_env(), args)
    }

    pub fn with_paths(paths: Paths, args: Vec<String>) -> App {
        let mod_dir = failover::install_mod(&paths);
        let mut app = App {
            failover: config::failover_on(&paths),
            base_config: std::env::var_os("CLAUDE_CONFIG_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            paths,
            rows: Vec::new(),
            selected: 0,
            tabs: Vec::new(),
            current: 0,
            focus: Focus::Sidebar,
            quit: false,
            pane_size: (24, 80),
            first_args: Some(args),
            value_flags: None,
            mod_dir,
            next_handoff: 0,
            modal: None,
            flash: None,
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
        let mut rows: Vec<Row> = order
            .iter()
            .enumerate()
            .filter_map(|(i, (id, on))| {
                let e = entries.iter().find(|e| &e.id == id)?.clone();
                Some(self.row(e, Some(i + 1), *on))
            })
            .collect();
        for e in entries.into_iter().filter(|e| !e.in_failover()) {
            rows.push(self.row(e, None, false));
        }
        self.rows = rows;
        self.failover = config::failover_on(&self.paths);
        self.selected = selected
            .and_then(|id| self.rows.iter().position(|r| r.entry.id == id))
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
    }

    fn row(&self, entry: Entry, priority: Option<usize>, on: bool) -> Row {
        let usage = match entry.kind {
            Kind::Claude => usage::claude(&config::claude_dir(
                &self.paths,
                &entry,
                self.base_config.as_deref(),
            )),
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
        // ctrl-\ moves between the sidebar and the running CLI
        // terminals send it as 0x1c, which crossterm reads as ctrl-4
        if matches!(key.code, KeyCode::Char('\\' | '4'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.focus = match self.focus {
                Focus::Pane => Focus::Sidebar,
                Focus::Sidebar if !self.tabs.is_empty() => Focus::Pane,
                f => f,
            };
            return;
        }
        match self.focus {
            Focus::Sidebar => self.sidebar_key(key),
            Focus::Pane => self.pane_key(key),
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        if self.focus == Focus::Pane
            && let Some(p) = self.tabs.get(self.current).and_then(|t| t.pane.as_ref())
        {
            p.paste(text);
        }
    }

    fn sidebar_key(&mut self, key: KeyEvent) {
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
                    let mut o = self.order();
                    if let Some(m) = o.iter_mut().find(|(m, _)| *m == id) {
                        m.1 = !m.1;
                        config::save_order(&self.paths, &o);
                    }
                }
            }
            KeyCode::Char('f') => config::set_failover(&self.paths, !self.failover),
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
                self.focus = Focus::Sidebar;
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
                "Its folder moves to {}/.trash/{acct}-… (login, history and settings stay there).",
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
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => self.delete(&c.id),
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
            Kind::Other => entry.command.clone(),
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
                }
                return self.refresh();
            }
            if let Some((_, i)) = hits.rows.iter().find(|(r, _)| inside(r)) {
                // click selects, a click on the selected row opens it
                let again = self.selected == *i && self.focus == Focus::Sidebar;
                self.selected = *i;
                self.focus = Focus::Sidebar;
                if again && let Some(id) = self.rows.get(*i).map(|r| r.entry.id.clone()) {
                    self.open(&id);
                }
                return;
            }
        }
        if inside(&hits.pane)
            && let Some(tab) = self.tabs.get(self.current)
        {
            if matches!(m.kind, MouseEventKind::Down(_)) {
                self.focus = Focus::Pane;
            }
            if tab.notice.is_none()
                && let Some(p) = &tab.pane
            {
                p.mouse(m.kind, m.column - hits.pane.x, m.row - hits.pane.y);
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
            self.focus = Focus::Sidebar;
        }
    }

    // ── running CLIs ──────────────────────────────────────

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
            Kind::Other => {}
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
