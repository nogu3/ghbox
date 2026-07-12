mod app;
mod ui;

use anyhow::{Context, Result};
use crossterm::event::{Event, KeyCode, KeyEventKind};
use ghbox_core::config::{Config, KeySpec};
use ghbox_core::github::{self, Fetched};
use ghbox_core::inbox::build_sections;
use ghbox_core::store::{KIND_MERGE_COMMENT, Store};
use tokio::sync::mpsc;

use crate::app::{App, DoneEntry};

enum Msg {
    Key(crossterm::event::KeyEvent),
    Fetched(Box<ghbox_core::Result<Fetched>>),
    Redraw,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::load().context("failed to load config")?;
    let token = github::get_token().context("failed to get token via `gh auth token`")?;
    let store = Store::open(&config.db_path)
        .with_context(|| format!("failed to open db at {}", config.db_path.display()))?;

    let terminal = ratatui::init();
    let result = run(terminal, config, token, store).await;
    ratatui::restore();
    result
}

async fn run(
    mut terminal: ratatui::DefaultTerminal,
    config: Config,
    token: String,
    store: Store,
) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();

    // Input reader: crossterm blocking reads on a dedicated thread.
    let input_tx = tx.clone();
    std::thread::spawn(move || {
        loop {
            match crossterm::event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    if input_tx.send(Msg::Key(key)).is_err() {
                        break;
                    }
                }
                Ok(Event::Resize(..)) => {
                    if input_tx.send(Msg::Redraw).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    // Periodic fetch.
    let fetch_tx = tx.clone();
    let fetch_token = token.clone();
    let fetch_sections_cfg = config.sections.clone();
    let interval_secs = config.poll_interval_secs.max(30);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            let result = github::fetch_sections(&fetch_token, &fetch_sections_cfg).await;
            if fetch_tx.send(Msg::Fetched(Box::new(result))).is_err() {
                break;
            }
        }
    });

    let titles = config.sections.iter().map(|s| s.title.clone()).collect();
    let mut app = App::new(titles);
    terminal.draw(|f| ui::draw(f, &app, &config))?;

    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Key(key) => handle_key(key.code, &mut app, &config, &store, &tx, &token),
            Msg::Fetched(result) => match *result {
                Ok(fetched) => match build_sections(&config.sections, &fetched, &store).await {
                    Ok(results) => match app.apply_results(results) {
                        Some(e) => app.status = format!("filter error: {e}"),
                        None => app.status = format!("updated {}", now_hms()),
                    },
                    Err(e) => app.status = format!("error: {e}"),
                },
                Err(e) => app.status = format!("fetch error: {e}"),
            },
            Msg::Redraw => {}
        }
        if app.should_quit {
            break;
        }
        terminal.draw(|f| ui::draw(f, &app, &config))?;
    }
    Ok(())
}

/// HH:MM:SS local-ish time without pulling in chrono (UTC is fine for MVP).
fn now_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02} UTC")
}

fn key_matches(spec: KeySpec, code: KeyCode) -> bool {
    match spec {
        KeySpec::Char(c) => code == KeyCode::Char(c),
        KeySpec::Tab => code == KeyCode::Tab,
        KeySpec::BackTab => code == KeyCode::BackTab,
        KeySpec::Enter => code == KeyCode::Enter,
        KeySpec::Up => code == KeyCode::Up,
        KeySpec::Down => code == KeyCode::Down,
        KeySpec::Esc => code == KeyCode::Esc,
    }
}

fn handle_key(
    code: KeyCode,
    app: &mut App,
    config: &Config,
    store: &Store,
    tx: &mpsc::UnboundedSender<Msg>,
    token: &str,
) {
    let kb = &config.keybindings;
    // Configured bindings take precedence; the arrow-key arms at the end are
    // an always-on fallback for row movement (independent of keybindings).
    if key_matches(kb.quit, code) {
        app.should_quit = true;
    } else if key_matches(kb.down, code) {
        app.next();
    } else if key_matches(kb.up, code) {
        app.prev();
    } else if key_matches(kb.next_section, code) {
        app.next_section();
    } else if key_matches(kb.prev_section, code) {
        app.prev_section();
    } else if key_matches(kb.open, code) {
        if let Some(url) = app.selected_url()
            && let Err(e) = open::that_detached(url)
        {
            app.status = format!("failed to open browser: {e}");
        }
    } else if key_matches(kb.done, code) {
        let Some(entry) = app.selected_done_entry() else {
            return;
        };
        let (result, label) = match &entry {
            DoneEntry::Comment(id) => (
                store.mark_done(KIND_MERGE_COMMENT, &id.to_string()),
                id.to_string(),
            ),
            DoneEntry::Pr { key, updated_at } => (store.mark_done_pr(key, updated_at), key.clone()),
        };
        match result {
            Ok(()) => {
                app.remove_selected();
                app.status = format!("done: {label}");
            }
            Err(e) => app.status = format!("db error: {e}"),
        }
    } else if key_matches(kb.refresh, code) {
        app.status = "refreshing...".into();
        let tx = tx.clone();
        let token = token.to_string();
        let sections = config.sections.clone();
        tokio::spawn(async move {
            let result = github::fetch_sections(&token, &sections).await;
            let _ = tx.send(Msg::Fetched(Box::new(result)));
        });
    } else if code == KeyCode::Down {
        app.next();
    } else if code == KeyCode::Up {
        app.prev();
    }
}
