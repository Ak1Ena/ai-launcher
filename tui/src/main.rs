//! ai-launcher TUI: every AI coding CLI and account in one screen, each running in a tab, with
//! failover moving a session to the next account when a Claude account hits its usage limit.

mod app;
mod config;
mod failover;
mod form;
mod pane;
mod ui;
mod update;
mod usage;

use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind,
};
use crossterm::execute;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("-h" | "--help")) {
        println!(
            "ai-launcher [claude args]   open the TUI; the first CLI you open gets these args"
        );
        return Ok(());
    }
    let mut app = app::App::new(args);
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
    let result = run(&mut terminal, &mut app);
    let _ = execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        DisableMouseCapture
    );
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut app::App) -> anyhow::Result<()> {
    let mut last_refresh = Instant::now();
    let mut hits = ui::Hits::default();
    loop {
        let area = ui::pane_area(terminal.size()?.into());
        if app.pane_size != (area.height, area.width) {
            app.resize(area.height, area.width);
        }
        terminal.draw(|f| hits = ui::draw(f, app))?;
        if app.quit {
            return Ok(());
        }
        // redraw at ~30 fps while a CLI writes; idle otherwise
        if event::poll(Duration::from_millis(33))? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => app.on_key(k),
                Event::Paste(s) => app.on_paste(&s),
                Event::Mouse(m) => app.on_mouse(m, &hits),
                _ => {}
            }
        }
        app.tick();
        if last_refresh.elapsed() > Duration::from_secs(10) {
            app.refresh();
            last_refresh = Instant::now();
        }
    }
}
