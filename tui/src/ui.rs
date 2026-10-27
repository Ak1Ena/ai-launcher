//! Drawing. A tab bar (home, then one tab per CLI) with the failover switch; home is a
//! full-width list of accounts (failover order first), a CLI tab is the running CLI over a
//! one-line usage status; then modal windows (forms, failover notices). `draw` returns where
//! everything clickable landed, for the mouse.

use crate::app::{App, Item, Modal, Notice, PluginsWin, Review, SetupTarget};
use crate::config::Kind;
use crate::form::{Confirm, Form, Input};
use crate::usage;
use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use std::time::{Duration, Instant};
use tui_term::widget::{Cursor, PseudoTerminal};

pub const ACCENT: Color = Color::Rgb(0xd9, 0x77, 0x57);
const PANEL: Color = Color::Rgb(0x1b, 0x1b, 0x20);
const RAISED: Color = Color::Rgb(0x2c, 0x2c, 0x34);
const SELECT: Color = Color::Rgb(0x3b, 0x2a, 0x24);
const MUTED: Color = Color::Rgb(0x8a, 0x8a, 0x96);
const FAINT: Color = Color::Rgb(0x4a, 0x4a, 0x55);
const GOOD: Color = Color::Rgb(0x7d, 0xc9, 0x7d);
const WARN: Color = Color::Rgb(0xe5, 0xc0, 0x7b);
const BAD: Color = Color::Rgb(0xe0, 0x6c, 0x75);
const SWITCH_W: u16 = 7;

#[derive(Clone, Copy, PartialEq)]
pub enum Button {
    Add,
    /// install a plugin
    AddPlugin,
    /// the Plugins screen
    Plugins,
    Edit,
    Delete,
    Failover,
    /// failover / weighted round-robin
    Mode,
    /// at a limit: switch by itself or ask
    AutoSwitch,
    /// open the next account by round-robin
    Auto,
    /// the `✻ home` tab
    Home,
}

/// Where clickable things were drawn this frame.
#[derive(Default)]
pub struct Hits {
    pub rows: Vec<(Rect, usize)>,
    pub tabs: Vec<(Rect, usize)>,
    pub closes: Vec<(Rect, usize)>,
    pub buttons: Vec<(Rect, Button)>,
    /// each failover member's on/off switch, by row
    pub switches: Vec<(Rect, usize)>,
    /// each plugin section's on/off pill, by plugin name
    pub plugins: Vec<(Rect, String)>,
    /// the Plugins screen's rows, by index into `app.plugins_items()`
    pub plugin_rows: Vec<(Rect, usize)>,
    /// the Plugins screen's clickable cells: the row they act on and the key they press
    pub plugin_btns: Vec<(Rect, usize, KeyCode)>,
    /// form fields by index; `fields.len()` is the Save button
    pub fields: Vec<(Rect, usize)>,
    pub pane: Rect,
}

fn areas(area: Rect) -> [Rect; 3] {
    Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area)
}

/// Where the running CLI is drawn (all but the tab bar and the bottom line), for sizing its
/// terminal before the frame.
pub fn pane_area(area: Rect) -> Rect {
    areas(area)[1]
}

fn s(fg: Color) -> Style {
    Style::new().fg(fg)
}

pub fn draw(f: &mut Frame, app: &App) -> Hits {
    let [title, body, bottom] = areas(f.area());
    let mut hits = Hits::default();
    title_bar(f, app, title, &mut hits);
    match app.tabs.get(app.current).filter(|_| app.in_tab()) {
        Some(tab) => {
            hits.pane = body;
            if let Some(p) = &tab.pane {
                let parser = p.screen.lock().unwrap_or_else(|e| e.into_inner());
                let mut cursor = Cursor::default();
                if tab.notice.is_some() || app.modal.is_some() {
                    cursor.hide();
                }
                f.render_widget(PseudoTerminal::new(parser.screen()).cursor(cursor), body);
                if parser.screen().scrollback() > 0 {
                    let tag = " scrolled back · type to return ";
                    let w = tag.chars().count() as u16;
                    let r = Rect {
                        x: body.right().saturating_sub(w + 1),
                        y: body.y,
                        width: w.min(body.width),
                        height: 1,
                    };
                    f.render_widget(
                        Paragraph::new(tag).style(Style::new().fg(Color::Black).bg(WARN)),
                        r,
                    );
                }
            }
            if let Some(n) = &tab.notice {
                notice(f, app, n, body);
            }
            status_line(f, app, bottom);
        }
        None => {
            home(f, app, body, &mut hits);
            footer(f, app, bottom);
        }
    }
    if let Some(w) = &app.plugins_win {
        plugins_screen(f, app, w, f.area(), &mut hits);
    }
    match &app.modal {
        Some(Modal::Form(form)) => form_window(f, form, f.area(), &mut hits),
        Some(Modal::Confirm(c)) => confirm_window(f, app, c, f.area()),
        None => {}
    }
    hits
}

// ── title bar: home, tabs, failover switch ───────────────

/// A member id without its `claude/` prefix: the Claude account's own name.
fn bare(id: &str) -> &str {
    id.strip_prefix("claude/").unwrap_or(id)
}

fn short(app: &App, id: &str) -> String {
    match id.strip_prefix("claude/") {
        Some(_) => format!("Claude · {}", bare(id)),
        None => app
            .rows
            .iter()
            .find(|r| r.entry.id == id)
            .map(|r| r.entry.label.clone())
            .unwrap_or_else(|| id.into()),
    }
}

fn title_bar(f: &mut Frame, app: &App, area: Rect, hits: &mut Hits) {
    f.render_widget(Block::new().style(Style::new().bg(PANEL)), area);
    let home = " ✻ home ";
    let on_home = !app.in_tab();
    let r = Rect {
        width: (dw(home) as u16).min(area.width),
        ..area
    };
    let style = if on_home {
        Style::new().fg(Color::Black).bg(ACCENT)
    } else {
        s(ACCENT)
    };
    f.render_widget(
        Paragraph::new(home).style(style.add_modifier(Modifier::BOLD)),
        r,
    );
    hits.buttons.push((r, Button::Home));

    // right-aligned pills, each only drawn when it fits between home and the next pill
    let brand_end = r.right();
    let place = |right: u16, text: &str| {
        let w = dw(text) as u16;
        let x = right.checked_sub(w + 1)?;
        (x > brand_end).then_some(Rect {
            x,
            width: w,
            ..area
        })
    };
    let mut limit = area.right();

    let switch = if app.failover {
        " failover ● on "
    } else {
        " failover ○ off "
    };
    if let Some(r) = place(limit, switch) {
        let style = if app.failover {
            Style::new()
                .fg(Color::Black)
                .bg(GOOD)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(MUTED).bg(RAISED)
        };
        f.render_widget(Paragraph::new(switch).style(style), r);
        hits.buttons.push((r, Button::Failover));
        limit = r.x;
    }

    let mode = if app.wrr {
        " mode: round-robin "
    } else {
        " ⇢ mode: failover "
    };
    // the mode label only shows beside the switch
    if limit < area.right()
        && let Some(r) = place(limit, mode)
    {
        f.render_widget(
            Paragraph::new(mode).style(Style::new().fg(Color::White).bg(RAISED)),
            r,
        );
        hits.buttons.push((r, Button::Mode));
        limit = r.x;
        // at a limit: switch by itself or ask (`s`)
        let auto = if app.auto_switch {
            " ⏵ switch: auto "
        } else {
            " ? switch: ask "
        };
        if let Some(r) = place(limit, auto) {
            f.render_widget(
                Paragraph::new(auto).style(Style::new().fg(Color::White).bg(RAISED)),
                r,
            );
            hits.buttons.push((r, Button::AutoSwitch));
            limit = r.x;
        }
    }

    if let Some(v) = &app.update {
        let pill = format!(" ↑ v{v} available · npm i -g ai-launcher ");
        if let Some(r) = place(limit, &pill) {
            f.render_widget(
                Paragraph::new(pill).style(Style::new().fg(Color::Black).bg(WARN)),
                r,
            );
            limit = r.x;
        }
    }

    let mut x = brand_end + 1;
    for (i, t) in app.tabs.iter().enumerate() {
        let active = !on_home && i == app.current;
        let mark = match &t.notice {
            Some(Notice::Switch { .. } | Notice::NoneLeft { .. }) => "⇄ ",
            Some(Notice::Ended { .. } | Notice::Error(_)) => "■ ",
            None => "",
        };
        let label = format!(" {} {mark}{} ", i + 1, short(app, &t.member));
        let w = label.chars().count() as u16 + 2;
        if x + w >= limit {
            break;
        }
        let style = if active {
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else if i == app.current {
            // the tab ctrl-\\ and x act on from home
            Style::new()
                .fg(ACCENT)
                .bg(RAISED)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(MUTED).bg(RAISED)
        };
        let r = Rect {
            x,
            width: w,
            ..area
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::raw(label), Span::raw("✕ ")])).style(style),
            r,
        );
        hits.tabs.push((r, i));
        hits.closes.push((
            Rect {
                x: r.right() - 2,
                width: 2,
                ..r
            },
            i,
        ));
        x += w + 1;
    }
}

/// `5h █████░░░░░ 34%` like the bash picker: green < 60 <= yellow < 90 <= red.
/// `pad` right-aligns the percent to 3 digits, for columns.
fn meter(label: &str, pct: u8, w: usize, pad: bool) -> Vec<Span<'static>> {
    let color = if pct >= 90 {
        BAD
    } else if pct >= 60 {
        WARN
    } else {
        GOOD
    };
    // any use shows at least one cell, and only 100% fills the bar
    let full = match pct as usize {
        0 => 0,
        p => ((p * w + 50) / 100)
            .max(1)
            .min(if p < 100 { w.saturating_sub(1) } else { w }),
    };
    let pct = if pad {
        format!(" {pct:>3}%")
    } else {
        format!(" {pct}%")
    };
    vec![
        Span::styled(format!("{label:<2} "), s(MUTED)),
        Span::styled("█".repeat(full), s(color)),
        Span::styled("░".repeat(w - full), s(FAINT)),
        Span::styled(pct, s(color).add_modifier(Modifier::BOLD)),
    ]
}

fn width_of(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn dw(text: &str) -> usize {
    Span::raw(text).width()
}

/// `text` padded with spaces to `w` display columns.
fn pad(text: &str, w: usize) -> String {
    format!("{text}{}", " ".repeat(w.saturating_sub(dw(text))))
}

/// Cuts `text` to `max` display columns at a word boundary, ending in `…`.
fn ellipsize(text: &str, max: usize) -> String {
    if dw(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    for word in text.split(' ') {
        let next = if out.is_empty() {
            word.to_string()
        } else {
            format!("{out} {word}")
        };
        if dw(&next) + 1 > max {
            break;
        }
        out = next;
    }
    if out.is_empty() {
        // the first word alone is too long: cut it
        for c in text.chars() {
            if dw(&out) + dw(&c.to_string()) + 1 > max {
                break;
            }
            out.push(c);
        }
    }
    out.push('…');
    out
}

/// A key cap and what it does, for hint lines.
fn hint(key: &'static str, what: impl std::fmt::Display) -> Vec<Span<'static>> {
    vec![cap(key), Span::styled(format!(" {what}  "), s(MUTED))]
}

/// A hint line that fits in `max` columns: see `fit`. Every item ends in two spaces, which
/// may run off the edge for the last one kept.
fn hints(items: Vec<(u8, Vec<Span<'static>>)>, max: usize) -> Line<'static> {
    let items = items
        .into_iter()
        .map(|(p, sp)| (p, width_of(&sp), sp))
        .collect();
    Line::from(fit(items, max + 2).concat())
}

/// Keeps the most important items (lowest priority number first) that fit in `max` columns,
/// in their original order. Items are never cut.
fn fit<T>(items: Vec<(u8, usize, T)>, max: usize) -> Vec<T> {
    let mut by_prio: Vec<usize> = (0..items.len()).collect();
    by_prio.sort_by_key(|&i| items[i].0);
    let mut keep = vec![false; items.len()];
    let mut used = 0;
    for i in by_prio {
        if used + items[i].1 <= max {
            used += items[i].1;
            keep[i] = true;
        }
    }
    items
        .into_iter()
        .zip(keep)
        .filter_map(|(item, k)| k.then_some(item.2))
        .collect()
}

fn hm(d: Duration) -> String {
    let m = d.as_secs() / 60;
    format!("{}h{:02}m", m / 60, m % 60)
}

// ── pane content when nothing runs ───────────────────────

fn cap(t: &'static str) -> Span<'static> {
    Span::styled(
        format!(" {t} "),
        Style::new()
            .fg(Color::White)
            .bg(RAISED)
            .add_modifier(Modifier::BOLD),
    )
}

// ── home: one full-width list of accounts ────────────────

/// What the home table shows; `home` tries these from richest to plainest.
#[derive(Clone, Copy)]
struct Cols {
    usage: bool,
    bar: usize,
    reset: bool,
    age: bool,
    second: bool,
    email: bool,
}

/// The home table's usage cells: the 5h and 7d windows by label (none where missing), or
/// with one column only, the most-used window. Windows with other labels fill in by order.
fn columns(wins: &[usage::Window], two: bool) -> Vec<Option<&usage::Window>> {
    if !two {
        return vec![wins.iter().max_by_key(|w| w.percent)];
    }
    let by = |l: &str| wins.iter().find(|w| w.label == l);
    match (by("5h"), by("7d")) {
        (None, None) => vec![wins.first(), wins.get(1)],
        (a, b) => vec![a, b],
    }
}

/// One row per account like the bash picker: order, name, email, 5h and 7d bars with reset
/// times, the weight in round-robin, then the member's on/off switch.
fn home(f: &mut Frame, app: &App, area: Rect, hits: &mut Hits) {
    let inner = Block::new().padding(Padding::new(1, 1, 1, 0)).inner(area);
    let [list, buttons, flash] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(inner);
    let focused = app.modal.is_none();
    let now = usage::now_secs();
    // per row, worked out once a frame: CLI running in a tab, and the email or label
    let running: Vec<bool> = app
        .rows
        .iter()
        .map(|r| {
            app.tabs
                .iter()
                .any(|t| t.member == r.entry.id && t.pane.is_some() && t.notice.is_none())
        })
        .collect();
    let who: Vec<String> = app
        .rows
        .iter()
        .map(|r| match (&r.usage.email, r.entry.kind) {
            (Some(e), _) => e.clone(),
            (None, Kind::Claude) => "not logged in".into(),
            (None, _) => match &r.usage.plan {
                Some(p) => format!("{} · {p}", r.entry.label),
                None => r.entry.label.clone(),
            },
        })
        .collect();
    let is_default = |i: usize| app.rows[i].entry.id == app.default;
    let name_w = |i: usize| {
        dw(bare(&app.rows[i].entry.id))
            + if running[i] { 2 } else { 0 }
            + if is_default(i) { 2 } else { 0 }
    };
    // plugin rows draw their own way and don't widen the table
    let table_row = |i: usize| app.rows[i].entry.kind != Kind::Plugin;
    let names_w = (0..app.rows.len())
        .filter(|&i| table_row(i))
        .map(name_w)
        .max()
        .unwrap_or(0);
    let who_w = (0..app.rows.len())
        .filter(|&i| table_row(i))
        .map(|i| dw(&who[i]))
        .max()
        .unwrap_or(0);
    // everything before the weight and switch
    let left = |i: usize, c: Cols| -> Vec<Span<'static>> {
        let r = &app.rows[i];
        if !table_row(i) {
            return Vec::new();
        }
        let off = r.priority.is_some() && !r.on;
        let mut sp = vec![match r.priority {
            Some(p) => Span::styled(
                format!("{p:>2}  "),
                s(if off { FAINT } else { ACCENT }).add_modifier(Modifier::BOLD),
            ),
            None => Span::styled(" ·  ", s(FAINT)),
        }];
        sp.push(Span::styled(
            bare(&r.entry.id).to_string(),
            if off {
                s(FAINT)
            } else {
                s(Color::White).add_modifier(Modifier::BOLD)
            },
        ));
        if is_default(i) {
            sp.push(Span::styled(" ★", s(ACCENT)));
        }
        if running[i] {
            sp.push(Span::styled(" ●", s(GOOD)));
        }
        sp.push(Span::raw(" ".repeat(names_w - name_w(i) + 2)));
        if c.email {
            let style = if r.usage.email.is_none() && r.entry.kind == Kind::Claude {
                s(BAD)
            } else if off {
                s(FAINT)
            } else {
                s(MUTED)
            };
            sp.push(Span::styled(format!("{}  ", pad(&who[i], who_w)), style));
        }
        if !c.usage {
            return sp;
        }
        if let Some(left) = r.limited {
            sp.push(Span::styled(
                format!("limited · back in {}", hm(left)),
                s(WARN),
            ));
            return sp;
        }
        if r.usage.windows.is_empty() {
            sp.push(Span::styled("no usage data", s(FAINT)));
            return sp;
        }
        let cell_w = 3 + c.bar + 5 + if c.reset { 12 } else { 0 };
        for (i, w) in columns(&r.usage.windows, c.second).into_iter().enumerate() {
            if i > 0 {
                sp.push(Span::raw("  "));
            }
            let Some(w) = w else {
                sp.push(Span::raw(" ".repeat(cell_w)));
                continue;
            };
            sp.extend(meter(&w.label, w.percent, c.bar, true));
            if c.reset {
                let at = w
                    .reset
                    .as_ref()
                    .map(|t| format!("↻ {t}"))
                    .unwrap_or_default();
                sp.push(Span::styled(format!(" {}", pad(&at, 11)), s(FAINT)));
            }
        }
        if let Some(note) = r
            .usage
            .saved_at
            .and_then(|t| usage::age_note(t, now))
            .filter(|_| c.age)
        {
            sp.push(Span::styled(format!(" · {note}"), s(FAINT)));
        }
        sp
    };
    // members end in `×weight` (round-robin) and the switch, both right after the widest usage
    let weight_w = app
        .rows
        .iter()
        .filter(|r| r.priority.is_some())
        .map(|r| dw(&format!("×{} ", r.weight)))
        .max()
        .unwrap_or(0);
    let tail = 2 + if app.wrr { weight_w } else { 0 } + SWITCH_W as usize;
    let members_w = |lefts: &[Vec<Span<'static>>]| {
        (0..app.rows.len())
            .filter(|&i| app.rows[i].priority.is_some())
            .map(|i| width_of(&lefts[i]))
            .max()
            .unwrap_or(0)
    };
    let table_w = |lefts: &[Vec<Span<'static>>]| {
        let others = (0..app.rows.len())
            .filter(|&i| app.rows[i].priority.is_none() && table_row(i))
            .map(|i| width_of(&lefts[i]))
            .max()
            .unwrap_or(0);
        // a member row is never empty, so 0 means there are none
        let members = match members_w(lefts) {
            0 => 0,
            w => w + tail,
        };
        1 + others.max(members)
    };
    let full = Cols {
        usage: true,
        bar: 10,
        reset: true,
        age: true,
        second: true,
        email: true,
    };
    let plain = Cols {
        usage: false,
        reset: false,
        age: false,
        second: false,
        email: false,
        ..full
    };
    let levels = [
        full,
        Cols { bar: 6, ..full },
        Cols {
            bar: 6,
            age: false,
            ..full
        },
        Cols {
            reset: false,
            age: false,
            ..full
        },
        Cols {
            bar: 6,
            reset: false,
            age: false,
            ..full
        },
        Cols {
            reset: false,
            age: false,
            second: false,
            ..full
        },
        Cols {
            usage: true,
            email: false,
            ..plain
        },
        plain,
    ];
    let build = |c: Cols| (0..app.rows.len()).map(|i| left(i, c)).collect::<Vec<_>>();
    let mut lefts = build(plain);
    for c in levels {
        let l = build(c);
        if table_w(&l) <= list.width as usize {
            lefts = l;
            break;
        }
    }
    let lw = members_w(&lefts);

    // section headers, and one line per account
    let mut lines: Vec<(Line, Option<usize>)> = Vec::new();
    let header = |lines: &mut Vec<(Line, Option<usize>)>, text: &'static str| {
        if !lines.is_empty() {
            lines.push((Line::raw(""), None));
        }
        lines.push((
            Line::from(Span::styled(text, s(MUTED).add_modifier(Modifier::BOLD))),
            None,
        ));
    };
    if app.rows.iter().any(|r| r.priority.is_some()) {
        header(
            &mut lines,
            if app.wrr {
                "ROUND-ROBIN  ·  ×weight  ·  n opens next"
            } else {
                "FAILOVER ORDER  ·  top is tried first"
            },
        );
    }
    let mut others = false;
    // plugin section headers' on/off pills: (line, x offset, plugin)
    let mut pills: Vec<(usize, usize, String)> = Vec::new();
    let mut section = "";
    for (i, r) in app.rows.iter().enumerate() {
        if r.entry.kind == Kind::Plugin {
            let name = crate::plugins::split_id(&r.entry.id).map_or("", |(n, _)| n);
            if name != section {
                section = name;
                let Some(p) = app.plugins.iter().find(|p| p.plugin.name == name) else {
                    continue;
                };
                lines.push((Line::raw(""), None));
                let title = format!("{}  ", p.title());
                pills.push((lines.len(), dw(&title), name.to_string()));
                lines.push((
                    Line::from(vec![
                        Span::styled(title, s(MUTED).add_modifier(Modifier::BOLD)),
                        pill(p.plugin.on),
                    ]),
                    None,
                ));
            }
            let mark = if i == app.selected && focused {
                Span::styled("▌", s(ACCENT))
            } else {
                Span::raw(" ")
            };
            // an entry (`plugin:<name>/<id>`) or the section's state line (off, failed, …)
            let entry = r.entry.id.contains('/');
            let mut sp = vec![mark, Span::styled(" ·  ", s(FAINT))];
            if entry {
                sp.push(Span::styled(
                    r.entry.label.clone(),
                    s(Color::White).add_modifier(Modifier::BOLD),
                ));
                if app.default == r.entry.id {
                    sp.push(Span::styled(" ★", s(ACCENT)));
                }
                if running[i] {
                    sp.push(Span::styled(" ●", s(GOOD)));
                }
                let room = (list.width as usize).saturating_sub(width_of(&sp) + 2);
                sp.push(Span::styled(
                    format!("  {}", ellipsize(&r.status, room)),
                    s(MUTED),
                ));
            } else {
                sp.push(Span::styled(r.entry.label.clone(), s(FAINT)));
            }
            lines.push((Line::from(sp), Some(i)));
            for d in &r.details {
                let room = (list.width as usize).saturating_sub(7);
                lines.push((
                    Line::from(Span::styled(
                        format!("       {}", ellipsize(d, room)),
                        s(FAINT),
                    )),
                    None,
                ));
            }
            continue;
        }
        if r.priority.is_none() && !others {
            others = true;
            header(&mut lines, "OTHER CLIS");
        }
        let mut sp = vec![if i == app.selected && focused {
            Span::styled("▌", s(ACCENT))
        } else {
            Span::raw(" ")
        }];
        let l = std::mem::take(&mut lefts[i]);
        let pad_to = lw.saturating_sub(width_of(&l));
        sp.extend(l);
        if r.priority.is_some() {
            sp.push(Span::raw(" ".repeat(pad_to + 2)));
            if app.wrr {
                let off = !r.on || r.weight == 0;
                sp.push(Span::styled(
                    pad(&format!("×{} ", r.weight), weight_w),
                    s(if off { FAINT } else { MUTED }),
                ));
            }
            sp.push(if r.on {
                Span::styled(
                    " ● on  ",
                    Style::new()
                        .fg(Color::Black)
                        .bg(GOOD)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(" ○ off ", Style::new().fg(MUTED).bg(RAISED))
            });
        }
        lines.push((Line::from(sp), Some(i)));
    }
    if app.rows.is_empty() {
        lines.push((
            Line::from(Span::styled(
                "No accounts yet. Press a to add one.",
                s(MUTED),
            )),
            None,
        ));
    }

    // keep the selected row in view
    let sel_line = lines
        .iter()
        .position(|(_, r)| *r == Some(app.selected))
        .unwrap_or(0);
    let h = list.height as usize;
    let skip = (sel_line + 2).saturating_sub(h);
    // the switch sits right after the usage columns, the same place on every member row
    let switch_x = list.x as usize + 1 + lw + tail - SWITCH_W as usize;
    for (n, (line, row)) in lines.into_iter().skip(skip).take(h).enumerate() {
        let r = Rect {
            y: list.y + n as u16,
            height: 1,
            ..list
        };
        if let Some((_, x, name)) = pills.iter().find(|(l, ..)| *l == skip + n)
            && x + SWITCH_W as usize <= r.width as usize
        {
            hits.plugins.push((
                Rect {
                    x: r.x + *x as u16,
                    width: SWITCH_W,
                    ..r
                },
                name.clone(),
            ));
        }
        let style = match row {
            Some(i) if i == app.selected => Style::new().bg(if focused { SELECT } else { RAISED }),
            _ => Style::new(),
        };
        f.render_widget(Paragraph::new(line).style(style), r);
        if let Some(i) = row {
            if app.rows[i].priority.is_some() && switch_x + SWITCH_W as usize <= r.right() as usize
            {
                hits.switches.push((
                    Rect {
                        x: switch_x as u16,
                        width: SWITCH_W,
                        ..r
                    },
                    i,
                ));
            }
            hits.rows.push((r, i));
        }
    }

    let mut x = buttons.x;
    for (label, b) in home_buttons(app) {
        let w = dw(label) as u16;
        // only whole buttons, inside the list's width
        if buttons.height == 0 || x + w > buttons.right() {
            break;
        }
        let r = Rect {
            x,
            y: buttons.bottom() - 1,
            width: w,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(label).style(Style::new().fg(Color::White).bg(RAISED)),
            r,
        );
        hits.buttons.push((r, b));
        x += w + 1;
    }
    if let Some((msg, until)) = &app.flash
        && Instant::now() < *until
    {
        f.render_widget(
            Paragraph::new(Span::styled(ellipsize(msg, flash.width as usize), s(WARN))),
            flash,
        );
    }
}

// ── windows over everything ──────────────────────────────

fn window(f: &mut Frame, area: Rect, title: &str, w: u16, h: u16) -> Rect {
    let r = centered(area, w, h);
    f.render_widget(Clear, r);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(s(ACCENT))
        .style(Style::new().bg(PANEL))
        .title(Span::styled(
            format!(" {title} "),
            s(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::new(2, 2, 1, 0));
    let inner = block.inner(r);
    f.render_widget(block, r);
    inner
}

/// A plugin's on/off pill, SWITCH_W wide.
fn pill(on: bool) -> Span<'static> {
    if on {
        Span::styled(
            " ● on  ",
            Style::new()
                .fg(Color::Black)
                .bg(GOOD)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(" ○ off ", Style::new().fg(MUTED).bg(RAISED))
    }
}

/// Home's buttons under the list. Symbols only from the TUI's own set, no emoji.
pub fn home_buttons(app: &App) -> Vec<(&'static str, Button)> {
    let mut labels = vec![
        (" + Add ", Button::Add),
        (" ✎ Edit ", Button::Edit),
        (" × Delete ", Button::Delete),
        (" + Plugin ", Button::AddPlugin),
        (" Plugins ", Button::Plugins),
    ];
    if app.wrr {
        let auto = if app.deciding.is_some() {
            " · asking "
        } else {
            " › Auto "
        };
        labels.push((auto, Button::Auto));
    }
    labels
}

/// The text field's value with the cursor cell lit.
fn text_input(value: &str, cursor: usize) -> Vec<Span<'static>> {
    let chars: Vec<char> = value.chars().collect();
    let before: String = chars[..cursor].iter().collect();
    let at: String = chars
        .get(cursor)
        .map(|c| c.to_string())
        .unwrap_or_else(|| " ".into());
    let after: String = chars
        .get(cursor + 1..)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    vec![
        Span::styled(before, s(Color::White)),
        Span::styled(at, Style::new().fg(Color::Black).bg(ACCENT)),
        Span::styled(after, s(Color::White)),
    ]
}

/// The Plugins screen, over everything: installed plugins, bundled ones not installed, and the
/// add button (or the add form in its place), filtered by the search text.
fn plugins_screen(f: &mut Frame, app: &App, w: &PluginsWin, area: Rect, hits: &mut Hits) {
    // nothing under the screen is clickable
    *hits = Hits::default();
    f.render_widget(Clear, area);
    f.render_widget(Block::new().style(Style::new().bg(PANEL)), area);
    if let Some(r) = &w.review {
        return review_screen(f, app, r, area, hits);
    }
    let width = area.width as usize;
    let items = app.plugins_items();
    let sel = w.sel.min(items.len() - 1);

    // title line: home, the screen's name, counts on the right
    let home = " ✻ home ";
    let counts = format!(
        "{} installed · {} available ",
        app.plugins.len(),
        app.available.len()
    );
    let left = dw(home) + 9;
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(home, s(ACCENT).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled("Plugins", s(Color::White).add_modifier(Modifier::BOLD)),
            Span::raw(" ".repeat(width.saturating_sub(left + dw(&counts)))),
            Span::styled(counts, s(MUTED)),
        ])),
        Rect { height: 1, ..area },
    );
    hits.buttons.push((
        Rect {
            width: (dw(home) as u16).min(area.width),
            height: 1,
            ..area
        },
        Button::Home,
    ));

    // the list: (line, item it selects or None, clickable cells as (x, width, key))
    type Cells = Vec<(usize, usize, KeyCode)>;
    let mut lines: Vec<(Line, Option<usize>, Cells)> = Vec::new();
    let blank = || (Line::raw(""), None, Vec::new());
    let mut search = vec![Span::styled("  search: ", s(MUTED))];
    if w.searching {
        search.extend(text_input(&w.search, w.search.chars().count()));
    } else {
        search.push(Span::styled(w.search.clone(), s(Color::White)));
    }
    let slash = "[ / ]";
    let used = width_of(&search);
    search.push(Span::raw(" ".repeat(width.saturating_sub(used + 6))));
    search.push(Span::styled(slash, s(MUTED)));
    lines.push((
        Line::from(search),
        None,
        vec![(width.saturating_sub(6), 5, KeyCode::Char('/'))],
    ));
    lines.push(blank());

    let nw = app
        .plugins
        .iter()
        .map(|p| dw(&p.plugin.name))
        .chain(app.available.iter().map(|p| dw(&p.name)))
        .max()
        .unwrap_or(0)
        .clamp(4, 16);
    // name, about, status, pill, actions, update, [-] and the gaps; about takes the rest
    const STATUS: usize = 10;
    let aw = width.saturating_sub(nw + 50);
    let head = |t: &'static str| {
        (
            Line::styled(t, s(MUTED).add_modifier(Modifier::BOLD)),
            None,
            Vec::new(),
        )
    };
    let mark = |i: usize| {
        if i == sel {
            Span::styled("▌ ", s(ACCENT))
        } else {
            Span::raw("  ")
        }
    };
    let row_style = |i: usize| {
        if i == sel {
            Style::new().bg(SELECT)
        } else {
            Style::new()
        }
    };
    let none = |t: &str| (Line::styled(format!("   {t}"), s(MUTED)), None, Vec::new());

    lines.push(head("  INSTALLED"));
    let installed: Vec<(usize, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(at, it)| match it {
            Item::Installed(i) => Some((at, *i)),
            _ => None,
        })
        .collect();
    if installed.is_empty() {
        lines.push(none(if app.plugins.is_empty() {
            "no plugins yet"
        } else {
            "no match"
        }));
    }
    for (at, i) in installed {
        let p = &app.plugins[i];
        let about = if p.about.is_empty() && p.title() != p.plugin.name {
            p.title()
        } else {
            p.about.clone()
        };
        let (status, color) = match p.status() {
            "ok" => ("● ok", GOOD),
            "failed" => ("× failed", BAD),
            "off" => ("·", MUTED),
            other => (other, MUTED),
        };
        let actions = p.plugin.on && !p.actions.is_empty();
        let git = crate::plugins::from_git(&p.plugin.dir);
        let x0 = 2 + nw + 2 + aw + 2 + STATUS + 2;
        let mut cells = vec![(x0, SWITCH_W as usize, KeyCode::Char(' '))];
        if actions {
            cells.push((x0 + 9, 7, KeyCode::Enter));
        }
        if git {
            cells.push((x0 + 18, 8, KeyCode::Char('u')));
        }
        cells.push((x0 + 28, 3, KeyCode::Char('-')));
        let line = Line::from(vec![
            mark(at),
            Span::styled(
                pad(&ellipsize(&p.plugin.name, nw), nw),
                s(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(pad(&ellipsize(&about, aw), aw), s(MUTED)),
            Span::raw("  "),
            Span::styled(pad(status, STATUS), s(color)),
            Span::raw("  "),
            pill(p.plugin.on),
            Span::raw("  "),
            Span::styled(pad(if actions { "actions" } else { "" }, 7), s(ACCENT)),
            Span::raw("  "),
            Span::styled(pad(if git { "↻ update" } else { "" }, 8), s(ACCENT)),
            Span::raw("  "),
            Span::styled("[-]", s(BAD)),
        ])
        .style(row_style(at));
        lines.push((line, Some(at), cells));
    }
    lines.push(blank());

    lines.push(head("  AVAILABLE · bundled"));
    let available: Vec<(usize, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(at, it)| match it {
            Item::Available(i) => Some((at, *i)),
            _ => None,
        })
        .collect();
    if available.is_empty() {
        lines.push(none(if app.available.is_empty() {
            "no other bundled plugins"
        } else {
            "no match"
        }));
    }
    // ends at the same column as an installed row
    let aw2 = aw + 29;
    for (at, i) in available {
        let p = &app.available[i];
        let line = Line::from(vec![
            mark(at),
            Span::styled(
                pad(&ellipsize(&p.name, nw), nw),
                s(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(pad(&ellipsize(&p.about, aw2), aw2), s(MUTED)),
            Span::raw("  "),
            Span::styled(pad(if p.has_setup { "has setup" } else { "" }, 9), s(WARN)),
            Span::raw("  "),
            Span::styled("[+]", s(GOOD)),
        ])
        .style(row_style(at));
        let x = 2 + nw + 2 + aw2 + 2 + 9 + 2;
        lines.push((line, Some(at), vec![(x, 3, KeyCode::Char('+'))]));
    }
    lines.push(blank());

    let add_at = items.len() - 1;
    match &w.form {
        None => {
            let label = "[+ Add from git / folder]";
            lines.push((
                Line::from(vec![mark(add_at), Span::styled(label, s(Color::White))])
                    .style(row_style(add_at)),
                Some(add_at),
                vec![(2, dw(label), KeyCode::Char('+'))],
            ));
        }
        Some(form) => {
            let field = &form.fields[0];
            let mut input = vec![Span::styled("  Add from git / folder: ", s(ACCENT))];
            if let Input::Text { value, cursor } = &field.input {
                input.extend(text_input(value, *cursor));
            }
            lines.push((Line::from(input), None, Vec::new()));
            lines.push((
                Line::styled(format!("  {}", field.hint), s(FAINT)),
                None,
                Vec::new(),
            ));
            if let Some(e) = &form.error {
                lines.push((Line::styled(format!("  {e}"), s(BAD)), None, Vec::new()));
            }
        }
    }

    // the list scrolls to keep the selected line in view, between the title and the bottom two
    let list = Rect {
        y: area.y + 2,
        height: area.height.saturating_sub(4),
        ..area
    };
    let at_sel = lines.iter().position(|l| l.1 == Some(sel)).unwrap_or(0);
    let top = (at_sel + 1).saturating_sub(list.height as usize);
    for (n, (line, item, cells)) in lines.into_iter().skip(top).enumerate() {
        if n >= list.height as usize {
            break;
        }
        let r = Rect {
            y: list.y + n as u16,
            height: 1,
            ..list
        };
        f.render_widget(Paragraph::new(line), r);
        if let Some(i) = item {
            hits.plugin_rows.push((r, i));
        }
        for (x, cw, code) in cells {
            // only whole cells, inside the screen
            if x + cw <= width {
                let rect = Rect {
                    x: r.x + x as u16,
                    width: cw as u16,
                    ..r
                };
                hits.plugin_btns.push((rect, item.unwrap_or(sel), code));
            }
        }
    }

    let bottom = Rect {
        y: area.bottom().saturating_sub(1),
        height: 1,
        ..area
    };
    flash_above(f, app, bottom);
    let keys = if w.form.is_some() {
        vec![(0, hint("enter", "install")), (0, hint("esc", "cancel"))]
    } else if w.searching {
        vec![(0, hint("enter", "done")), (0, hint("esc", "clear"))]
    } else {
        vec![
            (0, hint("enter", "actions")),
            (2, hint("space", "on/off")),
            (2, hint("u", "update")),
            (2, hint("s", "setup")),
            (1, hint("-", "remove")),
            (1, hint("+", "install/add")),
            (1, hint("/", "search")),
            (0, hint("esc", "back")),
        ]
    };
    f.render_widget(Paragraph::new(hints(keys, width)), bottom);
}

/// The flash message on the line above `bottom`, if one is up.
fn flash_above(f: &mut Frame, app: &App, bottom: Rect) {
    if let Some((msg, until)) = &app.flash
        && Instant::now() < *until
    {
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(
                    "  {}",
                    ellipsize(msg, (bottom.width as usize).saturating_sub(2))
                ),
                s(WARN),
            )),
            Rect {
                y: bottom.y.saturating_sub(1),
                ..bottom
            },
        );
    }
}

/// One script line made safe to draw: control characters as `^X` (C1 as `<M-^X>`), and
/// invisible, blank-looking or direction-changing characters as `<U+XXXX>` (zero-width ones,
/// fillers, and every whitespace but a plain space: bash reads those as part of a word), so the
/// screen can't be steered and nothing that runs is hidden. Tabs become spaces.
pub fn escape_line(line: &str) -> String {
    let mut out = String::new();
    for c in line.chars() {
        match c {
            '\t' => out.push_str("    "),
            '\u{7f}' => out.push_str("^?"),
            c if (c as u32) < 0x20 => {
                out.push('^');
                out.push((c as u8 + 0x40) as char);
            }
            '\u{80}'..='\u{9f}' => {
                out.push_str(&format!("<M-^{}>", (c as u8 - 0x40) as char));
            }
            '\u{061c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2069}'
            | '\u{feff}'
            | '\u{115f}'
            | '\u{1160}'
            | '\u{2800}'
            | '\u{3164}'
            | '\u{ffa0}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{e0000}'..='\u{e007f}' => out.push_str(&format!("<U+{:04X}>", c as u32)),
            c if !c.is_ascii() && (c.is_whitespace() || dw(&c.to_string()) == 0) => {
                out.push_str(&format!("<U+{:04X}>", c as u32))
            }
            c => out.push(c),
        }
    }
    out
}

/// `text` cut into pieces of at most `w` display columns (at least one piece).
fn chunks(text: &str, w: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut used = 0;
    for c in text.chars() {
        let cw = dw(&c.to_string());
        if used + cw > w.max(1) && used > 0 {
            out.push(String::new());
            used = 0;
        }
        if let Some(last) = out.last_mut() {
            last.push(c);
        }
        used += cw;
    }
    out
}

/// The setup review, over the Plugins screen: the whole script, numbered and escaped, long
/// lines wrapped (nothing past the edge), then what approving does and the two buttons.
fn review_screen(f: &mut Frame, app: &App, r: &Review, area: Rect, hits: &mut Hits) {
    let width = area.width as usize;
    let (verb, button) = match r.target {
        SetupTarget::Install(_) => ("install", "[ Approve and install ]"),
        SetupTarget::Rerun(_) => ("setup", "[ Approve and run ]"),
    };
    let row = |y: u16| Rect {
        y: area.y + y.min(area.height.saturating_sub(1)),
        height: 1,
        ..area
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ✻ home ", s(ACCENT).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(
                format!("Plugins › {verb} {}", r.target.name()),
                s(Color::White).add_modifier(Modifier::BOLD),
            ),
        ])),
        row(0),
    );
    // a script ending in a newline has no empty last line
    let mut src: Vec<&str> = r.text.split('\n').collect();
    if src.len() > 1 && src.last() == Some(&"") {
        src.pop();
    }
    let head = "  SETUP SCRIPT · review before running";
    let count = format!("setup · {} lines  ", src.len());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(head, s(MUTED).add_modifier(Modifier::BOLD)),
            Span::raw(" ".repeat(width.saturating_sub(dw(head) + dw(&count)))),
            Span::styled(count, s(MUTED)),
        ])),
        row(2),
    );

    // bottom up: keys, flash, buttons, blank, two notice lines, blank, then the box
    let bottom = area.bottom().saturating_sub(1);
    let at = |up: u16| Rect {
        y: bottom.saturating_sub(up).max(area.y),
        height: 1,
        ..area
    };
    let boxed = Rect {
        x: area.x + 2,
        y: area.y + 4,
        width: area.width.saturating_sub(4),
        height: at(7).y.saturating_sub(area.y + 4),
    };
    let nw = src.len().to_string().len();
    let inner_w = (boxed.width as usize).saturating_sub(2 + nw + 4);
    let inner_h = boxed.height.saturating_sub(2) as usize;
    // display rows from the first line on: (number or blank, text)
    // every display row of the script: scrolling moves by these, so each byte can be reached
    let rows: Vec<(String, String)> = src
        .iter()
        .enumerate()
        .flat_map(|(i, l)| {
            chunks(&escape_line(l), inner_w)
                .into_iter()
                .enumerate()
                .map(move |(k, c)| {
                    let n = if k == 0 {
                        format!("{:>nw$}", i + 1)
                    } else {
                        " ".repeat(nw)
                    };
                    (n, c)
                })
        })
        .collect();
    r.view.set((rows.len(), inner_h, inner_w));
    let top = r.scroll.min(rows.len().saturating_sub(inner_h));
    let more = rows.len() > inner_h;
    let mut block = Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(s(FAINT));
    if more {
        block = block.title_bottom(Line::styled(" ↑↓ scroll ", s(MUTED)).right_aligned());
    }
    let lines: Vec<Line> = rows
        .into_iter()
        .skip(top)
        .take(inner_h)
        .map(|(n, c)| {
            Line::from(vec![
                Span::styled(format!(" {n}  "), s(FAINT)),
                Span::styled(c, s(Color::White)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), boxed);

    f.render_widget(
        Paragraph::new(Line::styled(
            "  Runs in a terminal tab as you. The plugin is installed only if setup succeeds.",
            s(Color::White),
        )),
        at(6),
    );
    f.render_widget(
        Paragraph::new(Line::styled(
            "  The plugin's own code also runs with your permissions: install only plugins you trust.",
            s(WARN),
        )),
        at(5),
    );
    let cancel = "[ Cancel ]";
    let buttons = at(3);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled(button, s(GOOD).add_modifier(Modifier::BOLD)),
            Span::raw("   "),
            Span::styled(cancel, s(MUTED)),
        ])),
        buttons,
    );
    let sel = app.plugins_win.as_ref().map_or(0, |w| w.sel);
    let bx = buttons.x + 2;
    let cx = bx + dw(button) as u16 + 3;
    for (x, w, code) in [
        (bx, dw(button), KeyCode::Enter),
        (cx, dw(cancel), KeyCode::Esc),
    ] {
        if x + w as u16 <= area.right() {
            hits.plugin_btns.push((
                Rect {
                    x,
                    width: w as u16,
                    ..buttons
                },
                sel,
                code,
            ));
        }
    }
    flash_above(f, app, at(0));
    let keys = vec![
        (0, hint("enter", "approve")),
        (0, hint("↑↓ PgUp PgDn", "scroll")),
        (0, hint("esc", "cancel")),
    ];
    f.render_widget(Paragraph::new(hints(keys, width)), at(0));
}

fn keycap(t: &'static str) -> Span<'static> {
    Span::styled(
        format!(" {t} "),
        Style::new()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD),
    )
}

fn notice(f: &mut Frame, app: &App, n: &Notice, area: Rect) {
    let d = |t: &'static str| Span::styled(t, s(MUTED));
    let bold = s(Color::White).add_modifier(Modifier::BOLD);
    let (title, lines): (&str, Vec<Line>) = match n {
        Notice::Switch {
            from,
            to,
            codex,
            at,
        } => {
            let mut lines = vec![
                Line::from(format!("{from} hit its usage limit.")),
                Line::from(vec![
                    Span::raw(if at.is_some() {
                        "→ Moving this session to "
                    } else {
                        "→ Move this session to "
                    }),
                    Span::styled(app.label(to), bold),
                    Span::raw(if at.is_some() { "." } else { "?" }),
                ]),
            ];
            if *codex {
                lines.push(Line::from(Span::styled(
                    "This sends the conversation to OpenAI (Codex).",
                    s(WARN),
                )));
            }
            let keys = if let Some(at) = at {
                let left = at.saturating_duration_since(Instant::now()).as_secs() + 1;
                lines.extend([
                    Line::raw(""),
                    Line::from(Span::styled(format!("Switching in {left}s"), s(WARN))),
                ]);
                vec![keycap("enter"), d(" now   ")]
            } else {
                vec![keycap("y"), d(" move   ")]
            };
            lines.extend([
                Line::raw(""),
                Line::from(
                    [
                        keys,
                        vec![keycap("o"), d(" change order   "), keycap("q"), d(" stop")],
                    ]
                    .concat(),
                ),
            ]);
            ("✻ Failover", lines)
        }
        Notice::NoneLeft { from } => (
            "✻ Failover",
            vec![
                Line::from(format!("{from} hit its usage limit.")),
                Line::from(Span::styled(
                    "No other account is on and free right now.",
                    s(WARN),
                )),
                Line::raw(""),
                Line::from(vec![
                    keycap("o"),
                    d(" change order   "),
                    keycap("enter"),
                    d(" try again   "),
                    keycap("q"),
                    d(" stop"),
                ]),
            ],
        ),
        Notice::Ended { code } => (
            "Ended",
            vec![
                Line::from(format!(
                    "{} ended (exit {code}).",
                    app.label(&app.tabs[app.current].member)
                )),
                Line::raw(""),
                Line::from(vec![
                    keycap("enter"),
                    // a setup tab has nothing to start again: enter closes it
                    d(if app.tabs[app.current].member.starts_with("setup:") {
                        " close   "
                    } else {
                        " start again   "
                    }),
                    keycap("x"),
                    d(" close tab"),
                ]),
            ],
        ),
        Notice::Error(e) => (
            "Problem",
            vec![
                Line::from(Span::styled(e.clone(), s(BAD))),
                Line::raw(""),
                Line::from(vec![
                    keycap("enter"),
                    d(" try again   "),
                    keycap("x"),
                    d(" close tab"),
                ]),
            ],
        ),
    };
    let w = lines.iter().map(Line::width).max().unwrap_or(20) as u16 + 6;
    let inner = window(f, area, title, w, lines.len() as u16 + 3);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn form_window(f: &mut Frame, form: &Form, area: Rect, hits: &mut Hits) {
    let height = form.fields.len() as u16 * 3 + 6;
    let inner = window(f, area, &form.title, 66, height);
    const LABEL: u16 = 10;
    for (i, field) in form.fields.iter().enumerate() {
        let y = inner.y + i as u16 * 3;
        let focused = form.focus == i;
        let label_style = if focused {
            s(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            s(MUTED)
        };
        f.render_widget(
            Paragraph::new(Span::styled(field.label, label_style)),
            Rect {
                y,
                width: LABEL,
                height: 1,
                ..inner
            },
        );
        let box_r = Rect {
            x: inner.x + LABEL,
            y,
            width: inner.width.saturating_sub(LABEL),
            height: 1,
        };
        let bg = if field.locked {
            PANEL
        } else if focused {
            Color::Rgb(0x3a, 0x3a, 0x44)
        } else {
            RAISED
        };
        let content: Line = match &field.input {
            Input::Choice { options, at } => Line::from(vec![
                Span::styled(" ‹ ", s(if focused { ACCENT } else { FAINT })),
                Span::styled(
                    options[*at].as_str(),
                    s(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" › ", s(if focused { ACCENT } else { FAINT })),
            ]),
            Input::Text { value, cursor } if focused && !field.locked => {
                let mut spans = vec![Span::raw(" ")];
                spans.extend(text_input(value, *cursor));
                Line::from(spans)
            }
            Input::Text { value, .. } => Line::from(Span::styled(
                format!(" {value}"),
                s(if field.locked { FAINT } else { Color::White }),
            )),
        };
        f.render_widget(Paragraph::new(content).style(Style::new().bg(bg)), box_r);
        f.render_widget(
            Paragraph::new(Span::styled(field.hint.as_str(), s(FAINT))),
            Rect {
                x: box_r.x + 1,
                y: y + 1,
                width: box_r.width.saturating_sub(1),
                height: 1,
            },
        );
        hits.fields.push((
            Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 2,
            },
            i,
        ));
    }
    let y = inner.y + form.fields.len() as u16 * 3;
    if let Some(e) = &form.error {
        f.render_widget(
            Paragraph::new(Span::styled(e.as_str(), s(BAD))),
            Rect {
                y,
                height: 1,
                ..inner
            },
        );
    }
    let save_style = if form.focus == form.fields.len() {
        Style::new()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::White).bg(RAISED)
    };
    let save_r = Rect {
        x: inner.x + LABEL,
        y: y + 1,
        width: 8,
        height: 1,
    };
    f.render_widget(Paragraph::new(Span::styled("  Save  ", save_style)), save_r);
    hits.fields.push((save_r, form.fields.len()));
    f.render_widget(
        Paragraph::new(Span::styled("tab next · esc cancel", s(FAINT))),
        Rect {
            x: save_r.right() + 2,
            y: y + 1,
            width: inner.width.saturating_sub(LABEL + 10),
            height: 1,
        },
    );
}

fn confirm_window(f: &mut Frame, app: &App, c: &Confirm, area: Rect) {
    let lines = vec![
        Line::from(vec![
            Span::raw("Delete "),
            Span::styled(
                app.label(&c.id),
                s(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::raw("?"),
        ]),
        Line::raw(""),
        Line::from(Span::styled(c.what.clone(), s(MUTED))),
        Line::raw(""),
        Line::from(vec![
            keycap("y"),
            Span::styled(" delete   ", s(MUTED)),
            keycap("n"),
            Span::styled(" keep", s(MUTED)),
        ]),
    ];
    // rows after wrapping (64 wide - borders - padding = 58), so a long path never hides the y / n line;
    // shortcut: counts by width, so word wrap can still need a row more on a line of long words
    let rows: usize = lines.iter().map(|l| l.width().div_ceil(58).max(1)).sum();
    let inner = window(f, area, "Delete", 64, rows as u16 + 5);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Home's key hints (or the open window's).
fn footer(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(Block::new().style(Style::new().bg(PANEL)), area);
    // priority: 0 is kept longest when the bar is short
    let items = if app.modal.is_some() {
        vec![
            (1, hint("tab", "next")),
            (0, hint("enter", "save / confirm")),
            (0, hint("esc", "cancel")),
        ]
    } else if app
        .rows
        .get(app.selected)
        .is_some_and(|r| r.entry.kind == Kind::Plugin)
    {
        vec![
            (0, hint("enter", "open")),
            (1, hint("a", "actions")),
            (1, hint("o", "on/off")),
            (3, hint("u", "update")),
            (2, hint("d", "delete")),
            (3, hint("*", "default")),
            (2, hint("p", "add plugin")),
            (0, hint("q", "quit")),
        ]
    } else {
        let mut items = vec![
            (0, hint("enter", "open")),
            (1, hint("a", "add")),
            (3, hint("e", "edit")),
            (3, hint("d", "delete")),
            (2, hint("shift-↑↓", "order")),
            (2, hint("space", "on/off")),
            (3, hint("*", "default")),
            (1, hint("f", "failover")),
            (1, hint("w", "mode")),
            (
                2,
                hint(
                    "s",
                    if app.auto_switch {
                        "switch: auto"
                    } else {
                        "switch: ask"
                    },
                ),
            ),
        ];
        if app.wrr {
            items.extend([
                (1, hint("n", "auto")),
                (2, hint("+-", "weight")),
                (4, hint("m", "model")),
            ]);
        }
        items.push((4, hint("r", "clear limit")));
        items.push((4, hint("p", "add plugin")));
        items.push((4, hint("P", "plugins")));
        if !app.tabs.is_empty() {
            // both act on the current tab, which the tab bar marks while home shows
            let n = app.current + 1;
            items.push((2, hint("ctrl-\\", format!("tab {n}"))));
            items.push((1, hint("x", format!("close {n}"))));
        }
        items.push((0, hint("q", "quit")));
        items
    };
    f.render_widget(Paragraph::new(hints(items, area.width as usize)), area);
}

/// A CLI tab's bottom line: its account's usage (and the next failover target) on the left,
/// key hints on the right. Items that don't fit drop by priority, never cut.
fn status_line(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(Block::new().style(Style::new().bg(PANEL)), area);
    let tab = &app.tabs[app.current];
    // (priority, width, (right side, spans))
    let mut items: Vec<(u8, usize, (bool, Vec<Span<'static>>))> = Vec::new();
    let mut add =
        |p: u8, right: bool, sp: Vec<Span<'static>>| items.push((p, width_of(&sp), (right, sp)));
    add(
        0,
        false,
        vec![Span::styled(
            bare(&tab.member).to_string(),
            s(ACCENT).add_modifier(Modifier::BOLD),
        )],
    );
    let home = hint("ctrl-\\", "home");
    let row = app.rows.iter().find(|r| r.entry.id == tab.member);
    if let Some((msg, until)) = &app.flash
        && Instant::now() < *until
    {
        // a message takes the usage's place while it lasts
        let room =
            (area.width as usize).saturating_sub(dw(bare(&tab.member)) + 2 + width_of(&home));
        add(
            1,
            false,
            vec![Span::styled(format!("  {}", ellipsize(msg, room)), s(WARN))],
        );
    } else if let Some(r) = row {
        if let Some(left) = r.limited {
            add(
                1,
                false,
                vec![Span::styled(
                    format!("  limited · back in {}", hm(left)),
                    s(WARN),
                )],
            );
        }
        let wins = columns(&r.usage.windows, true);
        // the reset time after the most-used window only, the first on a tie
        let most = (0..wins.len())
            .filter(|&i| wins[i].is_some())
            .max_by_key(|&i| (wins[i].map(|w| w.percent), std::cmp::Reverse(i)));
        for (i, w) in wins.iter().enumerate() {
            let Some(w) = w else { continue };
            let mut sp = vec![Span::raw("  ")];
            sp.extend(meter(&w.label, w.percent, 4, false));
            add(1, false, sp);
            if let (Some(t), true) = (&w.reset, most == Some(i)) {
                // a 5h reset is always within a day
                let t = if w.label == "5h" {
                    t.rsplit(' ').next().unwrap_or(t)
                } else {
                    t
                };
                add(3, false, vec![Span::styled(format!(" ↻ {t}"), s(FAINT))]);
            }
        }
        // launch() arms failover only for Claude entries
        if app.failover
            && r.entry.kind == Kind::Claude
            && let Some(next) = app.next_target(app.current)
        {
            add(
                2,
                false,
                vec![Span::styled(format!("  next: {}", bare(next)), s(MUTED))],
            );
        }
    }
    add(0, true, home);
    add(2, true, hint("⌥1-9", "tabs"));
    // the right side's last two spaces fall off the edge and leave a gap after the left side
    let (left, right): (Vec<_>, Vec<_>) = fit(items, area.width as usize)
        .into_iter()
        .partition(|(r, _)| !r);
    let left: Vec<Span> = left.into_iter().flat_map(|(_, sp)| sp).collect();
    let right: Vec<Span> = right.into_iter().flat_map(|(_, sp)| sp).collect();
    f.render_widget(Paragraph::new(Line::from(left)), area);
    let rw = (width_of(&right) as u16).saturating_sub(2).min(area.width);
    f.render_widget(
        Paragraph::new(Line::from(right)),
        Rect {
            x: area.right() - rw,
            width: rw,
            ..area
        },
    );
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_drops_least_important_whole_items_and_keeps_order() {
        let items = vec![(0, 5, "a"), (2, 6, "b"), (1, 4, "c"), (0, 3, "d")];
        assert_eq!(fit(items.clone(), 100), vec!["a", "b", "c", "d"]);
        assert_eq!(fit(items.clone(), 12), vec!["a", "c", "d"]);
        assert_eq!(fit(items.clone(), 9), vec!["a", "d"]);
        // a smaller item further down still gets in when a bigger one does not fit
        assert_eq!(
            fit(vec![(0, 5, "a"), (1, 9, "b"), (2, 2, "c")], 8),
            vec!["a", "c"]
        );
        assert!(fit(items, 2).is_empty());
    }

    #[test]
    fn hints_ignore_the_last_trailing_spaces() {
        // " q " + " quit  " is 10 wide, 8 without the trailing spaces
        let line = hints(vec![(0, hint("q", "quit"))], 8);
        assert_eq!(line.width(), 10);
        assert_eq!(hints(vec![(0, hint("q", "quit"))], 7).width(), 0);
    }

    #[test]
    fn meter_fills_at_least_one_cell_and_never_all_below_100() {
        let cells = |pct, w| meter("5h", pct, w, false)[1].content.chars().count();
        assert_eq!(cells(0, 10), 0);
        assert_eq!(cells(3, 10), 1);
        assert_eq!(cells(12, 4), 1);
        assert_eq!(cells(99, 10), 9);
        assert_eq!(cells(100, 10), 10);
    }

    #[test]
    fn home_columns_by_label_and_most_used() {
        let w = |label: &str, percent| usage::Window {
            label: label.into(),
            percent,
            ..Default::default()
        };
        let labels = |c: Vec<Option<&usage::Window>>| {
            c.iter()
                .map(|w| w.map_or("-".to_string(), |w| w.label.clone()))
                .collect::<Vec<_>>()
        };
        let only7 = [w("7d", 40)];
        assert_eq!(labels(columns(&only7, true)), ["-", "7d"]);
        let both = [w("5h", 10), w("7d", 40)];
        assert_eq!(labels(columns(&both, true)), ["5h", "7d"]);
        assert_eq!(labels(columns(&both, false)), ["7d"]);
        let odd = [w("1h", 5), w("2d", 9)];
        assert_eq!(labels(columns(&odd, true)), ["1h", "2d"]);
        assert_eq!(labels(columns(&[], false)), ["-"]);
    }

    #[test]
    fn ellipsize_cuts_at_words() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("abcdefgh", 4), "abc…");
        assert_eq!(ellipsize("abc", 0), "");
        assert_eq!(ellipsize("全角文字 x", 5), "全角…");
        assert_eq!(
            ellipsize("Mode: round-robin. n opens next", 20),
            "Mode: round-robin.…"
        );
    }
}
