//! Drawing: sidebar of accounts (priority, on/off, usage, limits), tabs of running CLIs, and the
//! failover notices drawn over a tab's last screen.

use crate::app::{App, Focus, Notice, Row};
use crate::config::Kind;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use std::time::{Duration, Instant};
use tui_term::widget::{Cursor, PseudoTerminal};

pub const ACCENT: Color = Color::Rgb(0xd9, 0x77, 0x57);
const DIM: Style = Style::new().fg(Color::DarkGray);
pub const SIDEBAR: u16 = 46;

/// Where the running CLI is drawn, for sizing its terminal before the frame.
pub fn pane_area(area: Rect) -> Rect {
    let [_, right] =
        Layout::horizontal([Constraint::Length(SIDEBAR), Constraint::Min(10)]).areas(body(area));
    let [_, pane] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(right);
    pane
}

fn body(area: Rect) -> Rect {
    Rect {
        height: area.height.saturating_sub(1),
        ..area
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let [left, right] =
        Layout::horizontal([Constraint::Length(SIDEBAR), Constraint::Min(10)]).areas(body(area));
    let [tabs, pane] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(right);
    sidebar(f, app, left);
    tab_bar(f, app, tabs);
    match app.tabs.get(app.current) {
        Some(tab) => {
            if let Some(p) = &tab.pane {
                let parser = p.screen.lock().unwrap_or_else(|e| e.into_inner());
                let mut cursor = Cursor::default();
                if app.focus != Focus::Pane || tab.notice.is_some() {
                    cursor.hide();
                }
                f.render_widget(PseudoTerminal::new(parser.screen()).cursor(cursor), pane);
            }
            if let Some(n) = &tab.notice {
                notice(f, app, n, pane);
            }
        }
        None => welcome(f, pane),
    }
    footer(
        f,
        app,
        Rect {
            y: area.height.saturating_sub(1),
            height: 1,
            ..area
        },
    );
}

fn sidebar(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Sidebar;
    let state = if app.failover {
        Span::styled(
            " on ",
            Style::new()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(" off ", Style::new().fg(Color::Black).bg(Color::DarkGray))
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused {
            Style::new().fg(ACCENT)
        } else {
            DIM
        })
        .title(Line::from(vec![Span::styled(
            " ✻ ai ",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )]))
        .title(Line::from(vec![Span::raw(" failover "), state, Span::raw(" ")]).right_aligned());
    let items: Vec<ListItem> = app.rows.iter().map(|r| row_item(r, app)).collect();
    let mut st = ListState::default().with_selected(Some(app.selected));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().bg(if focused {
            Color::Rgb(0x3a, 0x2a, 0x24)
        } else {
            Color::Rgb(0x26, 0x26, 0x26)
        }))
        .highlight_symbol(if focused { "▶" } else { " " });
    f.render_stateful_widget(list, area, &mut st);
}

fn row_item<'a>(r: &'a Row, app: &App) -> ListItem<'a> {
    let running = app
        .tabs
        .iter()
        .any(|t| t.member == r.entry.id && t.pane.is_some());
    let prio = match r.priority {
        Some(p) => Span::styled(format!("{p:>2} "), Style::new().fg(ACCENT)),
        None => Span::styled(" · ", DIM),
    };
    let name_style = if r.priority.is_some() && !r.on {
        DIM
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    let mut top = vec![prio, Span::styled(r.entry.label.clone(), name_style)];
    if running {
        top.push(Span::styled(" ●", Style::new().fg(Color::Green)));
    }
    if r.priority.is_some() {
        let tag = if r.on {
            Span::styled("  on", Style::new().fg(Color::Green))
        } else {
            Span::styled("  off", DIM)
        };
        top.push(tag);
    }
    let mut bottom = vec![Span::styled(
        format!("   {} ", kind_name(r.entry.kind)),
        DIM,
    )];
    if let Some(left) = r.limited {
        bottom.push(Span::styled(
            format!("limited · {} left", hm(left)),
            Style::new().fg(Color::Yellow),
        ));
    } else if r.usage.windows.is_empty() {
        bottom.push(Span::styled(
            if r.entry.kind == Kind::Other {
                ""
            } else {
                "no usage data"
            },
            DIM,
        ));
    } else {
        let bar_w = if r.usage.windows.len() > 1 { 5 } else { 12 };
        for w in &r.usage.windows {
            bottom.extend(bar(&w.label, w.percent, bar_w));
        }
    }
    if let Some(plan) = &r.usage.plan {
        bottom.push(Span::styled(format!(" {plan}"), DIM));
    }
    ListItem::new(vec![Line::from(top), Line::from(bottom)])
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Claude => "claude",
        Kind::Codex => "codex ",
        Kind::Other => "other ",
    }
}

fn bar(label: &str, pct: u8, w: usize) -> Vec<Span<'static>> {
    let color = if pct >= 90 {
        Color::Red
    } else if pct >= 60 {
        Color::Yellow
    } else {
        Color::Green
    };
    let full = (pct as usize * w + 50) / 100;
    vec![
        Span::styled(format!("{label} "), DIM),
        Span::styled("█".repeat(full), Style::new().fg(color)),
        Span::styled("░".repeat(w - full), DIM),
        Span::styled(format!("{pct:>3}% "), Style::new().fg(color)),
    ]
}

fn hm(d: Duration) -> String {
    let m = d.as_secs() / 60;
    format!("{}h{:02}m", m / 60, m % 60)
}

fn tab_bar(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();
    for (i, t) in app.tabs.iter().enumerate() {
        let on = i == app.current;
        let style = if on {
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new()
                .fg(Color::Gray)
                .bg(Color::Rgb(0x30, 0x30, 0x30))
        };
        let mark = match &t.notice {
            Some(Notice::Switch { .. } | Notice::NoneLeft { .. }) => " ⇄",
            Some(Notice::Ended { .. } | Notice::Error(_)) => " ■",
            None => "",
        };
        spans.push(Span::styled(
            format!(" {} {}{mark} ", i + 1, app.label(&t.member)),
            style,
        ));
        spans.push(Span::raw(" "));
    }
    if spans.is_empty() {
        spans.push(Span::styled(
            " no CLI open: pick one on the left and press enter",
            DIM,
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn welcome(f: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(
            "✻ ai-launcher",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Pick an account on the left and press enter. It runs here, in a tab."),
        Line::from(""),
        Line::from(Span::styled(
            "Failover",
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Line::from("When a Claude account hits its usage limit, the same session moves to the"),
        Line::from("next account in the order on the left. Codex gets a written handoff."),
        Line::from(""),
        Line::from(vec![
            Span::styled("f", Style::new().fg(ACCENT)),
            Span::raw("  turn failover on / off"),
        ]),
        Line::from(vec![
            Span::styled("shift-↑ ↓", Style::new().fg(ACCENT)),
            Span::raw("  change the order (top is tried first)"),
        ]),
        Line::from(vec![
            Span::styled("space", Style::new().fg(ACCENT)),
            Span::raw("  leave an account out / back in"),
        ]),
        Line::from(vec![
            Span::styled("r", Style::new().fg(ACCENT)),
            Span::raw("  clear an account's limit"),
        ]),
        Line::from(vec![
            Span::styled("ctrl-\\", Style::new().fg(ACCENT)),
            Span::raw("  move between this list and the running CLI"),
        ]),
    ];
    let inner = centered(area, 78, lines.len() as u16 + 2);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn notice(f: &mut Frame, app: &App, n: &Notice, area: Rect) {
    let key =
        |k: &'static str| Span::styled(k, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD));
    let (title, lines): (&str, Vec<Line>) = match n {
        Notice::Switch { from, to, at } => {
            let left = at.saturating_duration_since(Instant::now()).as_secs() + 1;
            (
                " ✻ Failover ",
                vec![
                    Line::from(format!("{from} hit its usage limit.")),
                    Line::from(vec![
                        Span::raw("→ Moving this session to "),
                        Span::styled(app.label(to), Style::new().add_modifier(Modifier::BOLD)),
                        Span::raw("."),
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled(format!("switching in {left}s · "), DIM),
                        key("enter"),
                        Span::styled(" now · ", DIM),
                        key("o"),
                        Span::styled(" change order · ", DIM),
                        key("q"),
                        Span::styled(" stop", DIM),
                    ]),
                ],
            )
        }
        Notice::NoneLeft { from } => (
            " ✻ Failover ",
            vec![
                Line::from(format!("{from} hit its usage limit.")),
                Line::from("No other account is on and free right now."),
                Line::from(""),
                Line::from(vec![
                    key("o"),
                    Span::styled(" change order · ", DIM),
                    key("enter"),
                    Span::styled(" try again · ", DIM),
                    key("q"),
                    Span::styled(" stop", DIM),
                ]),
            ],
        ),
        Notice::Ended { code } => (
            " Ended ",
            vec![
                Line::from(format!(
                    "{} ended (exit {code}).",
                    app.label(&app.tabs[app.current].member)
                )),
                Line::from(""),
                Line::from(vec![
                    key("enter"),
                    Span::styled(" start again · ", DIM),
                    key("x"),
                    Span::styled(" close tab · ", DIM),
                    key("ctrl-\\"),
                    Span::styled(" list", DIM),
                ]),
            ],
        ),
        Notice::Error(e) => (
            " Problem ",
            vec![
                Line::from(e.clone()),
                Line::from(""),
                Line::from(vec![
                    key("enter"),
                    Span::styled(" try again · ", DIM),
                    key("x"),
                    Span::styled(" close tab", DIM),
                ]),
            ],
        ),
    };
    let width = lines.iter().map(Line::width).max().unwrap_or(20) as u16 + 6;
    let r = centered(area, width, lines.len() as u16 + 2);
    f.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(
            title,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(
        Paragraph::new(lines).block(block.padding(ratatui::widgets::Padding::horizontal(2))),
        r,
    );
}

fn footer(f: &mut Frame, app: &App, area: Rect) {
    let k = |s: &'static str| Span::styled(s, Style::new().fg(ACCENT));
    let d = |s: &'static str| Span::styled(s, DIM);
    let spans = match app.focus {
        Focus::Sidebar => vec![
            k(" enter"),
            d(" open  "),
            k("shift-↑↓"),
            d(" order  "),
            k("space"),
            d(" on/off  "),
            k("f"),
            d(" failover  "),
            k("r"),
            d(" clear limit  "),
            k("1-9"),
            d(" tab  "),
            k("x"),
            d(" close  "),
            k("ctrl-\\"),
            d(" to CLI  "),
            k("q"),
            d(" quit"),
        ],
        Focus::Pane => vec![k(" ctrl-\\"), d(" back to the list (keys go to the CLI)")],
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
