//! App state and behaviour: the sidebar of accounts, tabs of running CLIs, and failover
//! moving a tab's session to the next account when its claude stops on a rate limit.

use crate::config::{self, Entry, Kind, Paths};
use crate::failover::{self, Split};
use crate::pane::Pane;
use crate::usage::{self, Usage};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
}

impl App {
    pub fn new(args: Vec<String>) -> App {
        let paths = Paths::from_env();
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
        let args = self.first_args.take().unwrap_or_default();
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
        let Some(Notice::Switch { to, .. }) = self.tabs[t].notice.take() else {
            return;
        };
        // the order may have changed while the notice was up
        let to = if config::limited_left(&self.paths, &to).is_none() {
            to
        } else {
            match failover::next_member(&self.paths, &self.entries(), &self.tabs[t].tried) {
                Some(n) => n,
                None => return self.retry(t),
            }
        };
        let Some(entry) = self.entry(&to) else {
            return self.retry(t);
        };
        let tab = &mut self.tabs[t];
        tab.member = to.clone();
        tab.tried.push(to.clone());
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
        self.launch(t, args);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
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
