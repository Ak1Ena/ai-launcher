//! Drawing. A title bar with tabs and the failover switch, a sidebar of accounts (failover
//! order first), the running CLI, and modal windows (forms, failover notices). `draw` returns
//! where everything clickable landed, for the mouse.

use crate::app::{App, Focus, Modal, Notice, Row};
use crate::config::Kind;
use crate::form::{Confirm, Form, Input};
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
        None => welcome(f, a.pane),
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

    let switch = if app.failover {
        " failover ● on "
    } else {
        " failover ○ off "
    };
    let sw = switch.chars().count() as u16;
    let sw_rect = Rect {
        x: area.right().saturating_sub(sw + 1),
        width: sw,
        ..area
    };
    let sw_style = if app.failover {
        Style::new()
            .fg(Color::Black)
            .bg(GOOD)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(MUTED).bg(RAISED)
    };
    f.render_widget(Paragraph::new(switch).style(sw_style), sw_rect);
    hits.buttons.push((sw_rect, Button::Failover));

    let mut limit = sw_rect.x;
    if let Some(v) = &app.update {
        let pill = format!(" ↑ v{v} available · npm i -g ai-launcher ");
        let pw = pill.chars().count() as u16;
        if let Some(px) = sw_rect.x.checked_sub(pw + 1) {
            let r = Rect {
                x: px,
                width: pw,
                ..area
            };
            f.render_widget(
                Paragraph::new(pill).style(Style::new().fg(Color::Black).bg(WARN)),
                r,
            );
            limit = px;
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
        header(&mut lines, "FAILOVER ORDER  ·  top is tried first");
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
    for (label, b) in [
        (" + Add ", Button::Add),
        (" ✎ Edit ", Button::Edit),
        (" ✕ Delete ", Button::Delete),
    ] {
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
        f.render_widget(Paragraph::new(Span::styled(msg.as_str(), s(WARN))), flash);
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
    let used = 5
        + name.chars().count()
        + if running { 2 } else { 0 }
        + right.as_ref().map_or(0, |_| SWITCH_W as usize);
    top.push(Span::raw(
        " ".repeat((width as usize).saturating_sub(used + 1)),
    ));
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
        let w = if r.usage.windows.len() > 1 { 6 } else { 14 };
        for (i, win) in r.usage.windows.iter().enumerate() {
            if i > 0 {
                bottom.push(Span::raw("  "));
            }
            bottom.extend(meter(&win.label, win.percent, w));
        }
        if let Some(plan) = &r.usage.plan {
            bottom.push(Span::styled(format!("  {plan}"), s(MUTED)));
        }
    }
    (Line::from(top), Line::from(bottom))
}

fn meter(label: &str, pct: u8, w: usize) -> Vec<Span<'static>> {
    let color = if pct >= 90 {
        BAD
    } else if pct >= 60 {
        WARN
    } else {
        GOOD
    };
    let full = (pct as usize * w).div_ceil(100).min(w);
    vec![
        Span::styled(format!("{label} "), s(MUTED)),
        Span::styled("━".repeat(full), s(color)),
        Span::styled("━".repeat(w - full), s(FAINT)),
        Span::styled(format!(" {pct}%"), s(color)),
    ]
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

fn welcome(f: &mut Frame, area: Rect) {
    let d = |t: &'static str| Span::styled(format!("  {t}"), s(MUTED));
    let lines = vec![
        Line::from(Span::styled(
            "✻ ai-launcher",
            s(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Every AI coding CLI and account in one place.",
            s(MUTED),
        )),
        Line::raw(""),
        Line::from(vec![
            cap("enter"),
            d("open the account picked on the left, in a tab"),
        ]),
        Line::from(vec![
            cap("a"),
            d("add an account   "),
            cap("e"),
            d("edit   "),
            cap("d"),
            d("delete"),
        ]),
        Line::from(vec![
            cap("⌥1-9"),
            d("switch tabs, also while typing in a CLI (or click a tab)"),
        ]),
        Line::from(vec![cap("ctrl-\\"), d("move between the list and the CLI")]),
        Line::raw(""),
        Line::from(Span::styled(
            "Failover",
            s(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "When a Claude account hits its usage limit, the session moves to the next",
            s(MUTED),
        )),
        Line::from(Span::styled(
            "account in the failover order, in the same tab. Codex gets a handoff.",
            s(MUTED),
        )),
        Line::raw(""),
        Line::from(vec![
            cap("f"),
            d("on / off   "),
            cap("shift-↑↓"),
            d("order   "),
            cap("space"),
            d("skip   "),
            cap("r"),
            d("clear a limit"),
        ]),
    ];
    let r = centered(area, 80, lines.len() as u16);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), r);
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
    let d = |t: &'static str| Span::styled(format!(" {t}  "), s(MUTED));
    let spans = if app.modal.is_some() {
        vec![
            cap("tab"),
            d("next"),
            cap("enter"),
            d("save / confirm"),
            cap("esc"),
            d("cancel"),
        ]
    } else if app.focus == Focus::Pane {
        vec![
            cap("ctrl-\\"),
            d("back to the list"),
            cap("⌥1-9"),
            d("switch tab"),
            cap("⌥← →"),
            d("prev / next tab"),
            Span::styled("  keys go to the CLI · click a tab or the list", s(FAINT)),
        ]
    } else {
        vec![
            cap("enter"),
            d("open"),
            cap("a"),
            d("add"),
            cap("e"),
            d("edit"),
            cap("d"),
            d("delete"),
            cap("shift-↑↓"),
            d("order"),
            cap("space"),
            d("skip"),
            cap("f"),
            d("failover"),
            cap("r"),
            d("clear limit"),
            cap("x"),
            d("close tab"),
            cap("q"),
            d("quit"),
        ]
    };
    f.render_widget(Paragraph::new(Line::from(spans)), area);
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
