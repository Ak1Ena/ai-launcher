//! Drawing. A title bar with tabs and the failover switch, a sidebar of accounts (failover
//! order first), the running CLI, and modal windows (forms, failover notices). `draw` returns
//! where everything clickable landed, for the mouse.

use crate::app::{App, Focus, Modal, Notice, Row};
use crate::config::Kind;
use crate::form::{Confirm, Form, Input};
use crate::usage;
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
pub const SIDEBAR: u16 = 44;
const SWITCH_W: u16 = 7;

#[derive(Clone, Copy, PartialEq)]
pub enum Button {
    Add,
    Edit,
    Delete,
    Failover,
    /// failover / weighted round-robin
    Mode,
    /// open the next account by round-robin
    Auto,
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
    /// form fields by index; `fields.len()` is the Save button
    pub fields: Vec<(Rect, usize)>,
    pub pane: Rect,
}

struct Areas {
    title: Rect,
    side: Rect,
    pane: Rect,
    footer: Rect,
}

fn areas(area: Rect) -> Areas {
    let [title, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let [side, pane] =
        Layout::horizontal([Constraint::Length(SIDEBAR), Constraint::Min(10)]).areas(body);
    Areas {
        title,
        side,
        pane,
        footer,
    }
}

/// Where the running CLI is drawn, for sizing its terminal before the frame.
pub fn pane_area(area: Rect) -> Rect {
    areas(area).pane
}

fn s(fg: Color) -> Style {
    Style::new().fg(fg)
}

pub fn draw(f: &mut Frame, app: &App) -> Hits {
    let a = areas(f.area());
    let mut hits = Hits {
        pane: a.pane,
        ..Hits::default()
    };
    title_bar(f, app, a.title, &mut hits);
    sidebar(f, app, a.side, &mut hits);
    match app.tabs.get(app.current) {
        Some(tab) => {
            if let Some(p) = &tab.pane {
                let parser = p.screen.lock().unwrap_or_else(|e| e.into_inner());
                let mut cursor = Cursor::default();
                if app.focus != Focus::Pane || tab.notice.is_some() || app.modal.is_some() {
                    cursor.hide();
                }
                f.render_widget(PseudoTerminal::new(parser.screen()).cursor(cursor), a.pane);
                if parser.screen().scrollback() > 0 {
                    let tag = " scrolled back · type to return ";
                    let w = tag.chars().count() as u16;
                    let r = Rect {
                        x: a.pane.right().saturating_sub(w + 1),
                        y: a.pane.y,
                        width: w,
                        height: 1,
                    };
                    f.render_widget(
                        Paragraph::new(tag).style(Style::new().fg(Color::Black).bg(WARN)),
                        r,
                    );
                }
            }
            if let Some(n) = &tab.notice {
                notice(f, app, n, a.pane);
            }
        }
        None => welcome(f, app, a.pane),
    }
    footer(f, app, a.footer);
    match &app.modal {
        Some(Modal::Form(form)) => form_window(f, form, f.area(), &mut hits),
        Some(Modal::Confirm(c)) => confirm_window(f, app, c, f.area()),
        None => {}
    }
    hits
}

// ── title bar: name, tabs, failover switch ───────────────

fn short(app: &App, id: &str) -> String {
    match id.strip_prefix("claude/") {
        Some(acct) => format!("Claude · {acct}"),
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
    let brand = " ✻ ai-launcher ";
    f.render_widget(
        Paragraph::new(Span::styled(brand, s(ACCENT).add_modifier(Modifier::BOLD))),
        area,
    );

    // right-aligned pills, each only drawn when it fits between the brand and the next pill
    let brand_end = area.x + dw(brand) as u16;
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
        " ⚖ mode: round-robin "
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

    let mut x = area.x + brand.chars().count() as u16 + 1;
    for (i, t) in app.tabs.iter().enumerate() {
        let active = i == app.current;
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

// ── sidebar ──────────────────────────────────────────────

fn sidebar(f: &mut Frame, app: &App, area: Rect, hits: &mut Hits) {
    let focused = app.focus == Focus::Sidebar && app.modal.is_none();
    let block = Block::new()
        .borders(Borders::RIGHT)
        .border_style(s(if focused { ACCENT } else { FAINT }))
        .style(Style::new().bg(PANEL))
        .padding(Padding::new(1, 1, 1, 0));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [list, buttons, flash] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(inner);

    // section headers, and two lines per account
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
    for (i, r) in app.rows.iter().enumerate() {
        if r.priority.is_none() && !others {
            others = true;
            header(&mut lines, "OTHER CLIS");
        }
        let (a, b) = row_lines(app, r, i == app.selected, focused, list.width);
        lines.push((a, Some(i)));
        lines.push((b, Some(i)));
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
    let skip = (sel_line + 3).saturating_sub(h);
    for (n, (line, row)) in lines.into_iter().skip(skip).take(h).enumerate() {
        let r = Rect {
            y: list.y + n as u16,
            height: 1,
            ..list
        };
        let selected = row == Some(app.selected);
        let bg = if selected {
            if focused { SELECT } else { RAISED }
        } else {
            PANEL
        };
        f.render_widget(Paragraph::new(line).style(Style::new().bg(bg)), r);
        if let Some(i) = row {
            // the first line of a member's row carries its switch
            if !hits.rows.iter().any(|(_, j)| *j == i) && app.rows[i].priority.is_some() {
                let x = (r.x + r.width).saturating_sub(SWITCH_W + 1);
                hits.switches.push((
                    Rect {
                        x,
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
    let y = buttons.y + 1;
    let auto = if app.deciding.is_some() {
        " ⋯ asking "
    } else {
        " ▶ Auto "
    };
    let mut buttons = vec![
        (" + Add ", Button::Add),
        (" ✎ Edit ", Button::Edit),
        (" ✕ Delete ", Button::Delete),
    ];
    if app.wrr {
        buttons.push((auto, Button::Auto));
    }
    for (label, b) in buttons {
        let w = label.chars().count() as u16;
        let r = Rect {
            x,
            y,
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

fn row_lines(
    app: &App,
    r: &Row,
    selected: bool,
    focused: bool,
    width: u16,
) -> (Line<'static>, Line<'static>) {
    let running = app
        .tabs
        .iter()
        .any(|t| t.member == r.entry.id && t.pane.is_some() && t.notice.is_none());
    let off = r.priority.is_some() && !r.on;
    let bar = if selected && focused {
        Span::styled("▌", s(ACCENT))
    } else {
        Span::raw(" ")
    };
    let num = match r.priority {
        Some(p) => Span::styled(
            format!("{p:>2}  "),
            s(if off { FAINT } else { ACCENT }).add_modifier(Modifier::BOLD),
        ),
        None => Span::styled(" ·  ", s(FAINT)),
    };
    let name = short(app, &r.entry.id);
    let name_style = if off {
        s(FAINT)
    } else {
        s(Color::White).add_modifier(Modifier::BOLD)
    };
    let mut top = vec![bar.clone(), num, Span::styled(name.clone(), name_style)];
    if running {
        top.push(Span::styled(" ●", s(GOOD)));
    }
    // the account's own failover switch (space or a click); `SWITCH_W` wide, at the right edge
    let right = match r.priority {
        Some(_) if r.on => Some(Span::styled(
            " ● on  ",
            Style::new()
                .fg(Color::Black)
                .bg(GOOD)
                .add_modifier(Modifier::BOLD),
        )),
        Some(_) => Some(Span::styled(" ○ off ", Style::new().fg(MUTED).bg(RAISED))),
        None => None,
    };
    let weight = (app.wrr && r.priority.is_some()).then(|| {
        Span::styled(
            format!("×{} ", r.weight),
            s(if r.weight == 0 || off { FAINT } else { MUTED }),
        )
    });
    let used = 5
        + name.chars().count()
        + if running { 2 } else { 0 }
        + weight.as_ref().map_or(0, |w| w.content.chars().count())
        + right.as_ref().map_or(0, |_| SWITCH_W as usize);
    top.push(Span::raw(
        " ".repeat((width as usize).saturating_sub(used + 1)),
    ));
    top.extend(weight);
    top.extend(right);

    let mut bottom = vec![bar, Span::raw("    ")];
    let kind = match r.entry.kind {
        Kind::Claude => "claude",
        Kind::Codex => "codex",
        Kind::Other => "cli",
    };
    if let Some(left) = r.limited {
        bottom.push(Span::styled(
            format!("limited · back in {}", hm(left)),
            s(WARN),
        ));
    } else if r.usage.windows.is_empty() {
        let text = if r.entry.kind == Kind::Other {
            r.entry.command.clone()
        } else {
            format!("{kind} · no usage yet")
        };
        bottom.push(Span::styled(text, s(FAINT)));
    } else {
        let wins = &r.usage.windows;
        let base = if wins.len() > 1 { 6 } else { 14 };
        // the reset time goes after the most-used window, with bars a little shorter if needed
        let most = (0..wins.len())
            .max_by_key(|&i| wins[i].percent)
            .unwrap_or(0);
        let avail = (width as usize).saturating_sub(5);
        let at = wins[most]
            .resets_at
            .map(|t| format!(" ↻ {}", usage::local_hm(t)));
        let build = |w: usize, reset: bool| {
            let mut out = Vec::new();
            for (i, win) in wins.iter().enumerate() {
                if i > 0 {
                    out.push(Span::raw("  "));
                }
                out.extend(meter(&win.label, win.percent, w, false));
                if let (true, true, Some(at)) = (reset, i == most, &at) {
                    out.push(Span::styled(at.clone(), s(FAINT)));
                }
            }
            if let Some(plan) = &r.usage.plan {
                out.push(Span::styled(format!("  {plan}"), s(MUTED)));
            }
            out
        };
        let pick = [(base, true), (base - 1, true), (base - 2, true)]
            .into_iter()
            .map(|(w, reset)| build(w, reset))
            .find(|sp| width_of(sp) <= avail)
            .unwrap_or_else(|| build(base, false));
        bottom.extend(pick);
    }
    (Line::from(top), Line::from(bottom))
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
fn hint(key: &'static str, what: &'static str) -> Vec<Span<'static>> {
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

/// What the welcome table shows; `fit` tries these from richest to plainest.
#[derive(Clone, Copy)]
struct Cols {
    bar: usize,
    reset: bool,
    age: bool,
    second: bool,
    email: bool,
}

/// One row per account like the bash picker: name, email, 5h and 7d bars with reset times.
fn welcome(f: &mut Frame, app: &App, area: Rect) {
    let now = usage::now_secs();
    let name = |r: &Row| {
        r.entry
            .id
            .strip_prefix("claude/")
            .unwrap_or(&r.entry.id)
            .to_string()
    };
    let who = |r: &Row| match (&r.usage.email, r.entry.kind) {
        (Some(e), _) => e.clone(),
        (None, Kind::Claude) => "not logged in".into(),
        (None, _) => match &r.usage.plan {
            Some(p) => format!("{} · {p}", r.entry.label),
            None => r.entry.label.clone(),
        },
    };
    let name_w = app.rows.iter().map(|r| dw(&name(r))).max().unwrap_or(0);
    let who_w = app.rows.iter().map(|r| dw(&who(r))).max().unwrap_or(0);
    // reset times per row and window, formatted once for all layouts tried below
    let resets: Vec<Vec<String>> = app
        .rows
        .iter()
        .map(|r| {
            r.usage
                .windows
                .iter()
                .map(|w| {
                    w.resets_at
                        .map(|t| format!("↻ {}", usage::local_hm(t)))
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect();
    let row_line = |(r, resets): (&Row, &Vec<String>), c: Cols| -> Line<'static> {
        let mut sp = vec![Span::styled(
            format!("{}  ", pad(&name(r), name_w)),
            s(Color::White).add_modifier(Modifier::BOLD),
        )];
        if c.email {
            let style = if r.usage.email.is_none() && r.entry.kind == Kind::Claude {
                s(BAD)
            } else {
                s(MUTED)
            };
            sp.push(Span::styled(format!("{}  ", pad(&who(r), who_w)), style));
        }
        if r.usage.windows.is_empty() {
            sp.push(Span::styled("no usage data", s(FAINT)));
            return Line::from(sp);
        }
        for (i, w) in r
            .usage
            .windows
            .iter()
            .take(if c.second { 2 } else { 1 })
            .enumerate()
        {
            if i > 0 {
                sp.push(Span::raw("  "));
            }
            sp.extend(meter(&w.label, w.percent, c.bar, true));
            if c.reset {
                sp.push(Span::styled(format!(" {}", pad(&resets[i], 11)), s(FAINT)));
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
        Line::from(sp)
    };
    let full = Cols {
        bar: 10,
        reset: true,
        age: true,
        second: true,
        email: true,
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
            reset: false,
            age: false,
            second: false,
            ..full
        },
        Cols {
            reset: false,
            age: false,
            second: false,
            email: false,
            ..full
        },
    ];
    let max = area.width.saturating_sub(2) as usize;
    let table_w = |c: Cols| {
        app.rows
            .iter()
            .zip(&resets)
            .map(|rr| row_line(rr, c).width())
            .max()
            .unwrap_or(0)
    };
    let cols = levels
        .into_iter()
        .find(|&c| table_w(c) <= max)
        .unwrap_or(levels[levels.len() - 1]);

    let hint = hints(
        vec![
            (
                0,
                vec![Span::styled("✻  ", s(ACCENT).add_modifier(Modifier::BOLD))],
            ),
            (1, hint("enter", "open in a tab")),
            (2, hint("a", "add")),
            (3, hint("w", "mode")),
            (4, hint("⌥1-9", "tabs")),
        ],
        max,
    );
    let mut lines = vec![hint, Line::raw("")];
    if app.rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "No accounts yet. Press a to add one.",
            s(MUTED),
        )));
    }
    lines.extend(app.rows.iter().zip(&resets).map(|rr| row_line(rr, cols)));
    let w = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let r = centered(area, w, lines.len() as u16);
    f.render_widget(Paragraph::new(lines), r);
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
        Notice::Switch { from, to, at } => {
            let left = at.saturating_duration_since(Instant::now()).as_secs() + 1;
            (
                "✻ Failover",
                vec![
                    Line::from(format!("{from} hit its usage limit.")),
                    Line::from(vec![
                        Span::raw("→ Moving this session to "),
                        Span::styled(app.label(to), bold),
                    ]),
                    Line::raw(""),
                    Line::from(Span::styled(format!("Switching in {left}s"), s(WARN))),
                    Line::raw(""),
                    Line::from(vec![
                        keycap("enter"),
                        d(" now   "),
                        keycap("o"),
                        d(" change order   "),
                        keycap("q"),
                        d(" stop"),
                    ]),
                ],
            )
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
                    d(" start again   "),
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
                Span::styled(options[*at], s(Color::White).add_modifier(Modifier::BOLD)),
                Span::styled(" › ", s(if focused { ACCENT } else { FAINT })),
            ]),
            Input::Text { value, cursor } if focused && !field.locked => {
                let chars: Vec<char> = value.chars().collect();
                let before: String = chars[..*cursor].iter().collect();
                let at: String = chars
                    .get(*cursor)
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| " ".into());
                let after: String = chars
                    .get(*cursor + 1..)
                    .map(|c| c.iter().collect())
                    .unwrap_or_default();
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(before, s(Color::White)),
                    Span::styled(at, Style::new().fg(Color::Black).bg(ACCENT)),
                    Span::styled(after, s(Color::White)),
                ])
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
    let inner = window(f, area, "Delete", 64, lines.len() as u16 + 4);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn footer(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(Block::new().style(Style::new().bg(PANEL)), area);
    // priority: 0 is kept longest when the bar is short
    let items = if app.modal.is_some() {
        vec![
            (1, hint("tab", "next")),
            (0, hint("enter", "save / confirm")),
            (0, hint("esc", "cancel")),
        ]
    } else if app.focus == Focus::Pane {
        vec![
            (0, hint("ctrl-\\", "back to the list")),
            (1, hint("⌥1-9", "switch tab")),
            (2, hint("⌥← →", "prev / next tab")),
            (
                3,
                vec![Span::styled(
                    "  keys go to the CLI · click a tab or the list  ",
                    s(FAINT),
                )],
            ),
        ]
    } else {
        let mut items = vec![
            (0, hint("enter", "open")),
            (1, hint("a", "add")),
            (3, hint("e", "edit")),
            (3, hint("d", "delete")),
            (2, hint("shift-↑↓", "order")),
            (2, hint("space", "on/off")),
            (1, hint("f", "failover")),
            (1, hint("w", "mode")),
        ];
        if app.wrr {
            items.extend([
                (1, hint("n", "auto")),
                (2, hint("+-", "weight")),
                (4, hint("m", "model")),
            ]);
        }
        items.push((4, hint("r", "clear limit")));
        if !app.tabs.is_empty() {
            items.push((1, hint("x", "close tab")));
        }
        items.push((0, hint("q", "quit")));
        items
    };
    f.render_widget(Paragraph::new(hints(items, area.width as usize)), area);
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
