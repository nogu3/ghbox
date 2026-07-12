mod app;
mod ui;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use crossterm::event::{Event, KeyCode, KeyEventKind};
use ghbox_core::config::{Config, KeyBinding, KeySpec, Section};
use ghbox_core::github;
use ghbox_core::inbox::{SectionResult, build_sections};
use ghbox_core::store::{KIND_MERGE_COMMENT, Store};
use tokio::sync::mpsc;

use crate::app::{App, DoneEntry};

enum Msg {
    Key(crossterm::event::KeyEvent),
    Sections(Box<ghbox_core::Result<Vec<SectionResult>>>),
    Redraw,
}

/// Fetch + filter off the event loop: command filters can block up to 10s
/// per section and must not freeze input handling. Opens its own Store —
/// SQLite handles a second connection to the same DB fine, and reads see
/// the main loop's committed done-marks.
async fn fetch_and_build(
    token: &str,
    sections: &[Section],
    db_path: &Path,
) -> ghbox_core::Result<Vec<SectionResult>> {
    let fetched = github::fetch_sections(token, sections).await?;
    let store = Store::open(db_path)?;
    build_sections(sections, &fetched, &store).await
}

/// `build_sections` holds `&Store` across the command-filter `.await`, and
/// `Store` (a `rusqlite::Connection`) is `!Sync`, so the future is `!Send`
/// and cannot be handed to `tokio::spawn` directly. Drive it to completion
/// on a `spawn_blocking` thread instead, via `Handle::block_on` — the
/// future never needs to move across threads once it starts, only the
/// plain-data inputs and the result do.
///
/// `fetching` guards against overlapping fetches: a black-holed HTTP
/// connection would otherwise pin one blocking-pool thread per poll tick.
/// Returns `false` (no task spawned) if a fetch is already in flight.
fn spawn_fetch(
    tx: &mpsc::UnboundedSender<Msg>,
    fetching: &Arc<AtomicBool>,
    token: String,
    sections: Vec<Section>,
    db_path: std::path::PathBuf,
) -> bool {
    if fetching.swap(true, Ordering::SeqCst) {
        return false;
    }
    let tx = tx.clone();
    let fetching = Arc::clone(fetching);
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let result = handle.block_on(fetch_and_build(&token, &sections, &db_path));
        let _ = tx.send(Msg::Sections(Box::new(result)));
        fetching.store(false, Ordering::SeqCst);
    });
    true
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
    let fetching = Arc::new(AtomicBool::new(false));
    let fetch_tx = tx.clone();
    let fetch_token = token.clone();
    let fetch_sections_cfg = config.sections.clone();
    let fetch_db_path = config.db_path.clone();
    let fetch_fetching = Arc::clone(&fetching);
    let interval_secs = config.poll_interval_secs.max(30);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            if fetch_tx.is_closed() {
                break;
            }
            // Skip this tick if a fetch is still in flight; no message needed.
            spawn_fetch(
                &fetch_tx,
                &fetch_fetching,
                fetch_token.clone(),
                fetch_sections_cfg.clone(),
                fetch_db_path.clone(),
            );
        }
    });

    let titles = config.sections.iter().map(|s| s.title.clone()).collect();
    let mut app = App::new(titles);
    terminal.draw(|f| ui::draw(f, &app, &config))?;

    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Key(key) => {
                handle_key(key.code, &mut app, &config, &store, &tx, &fetching, &token)
            }
            Msg::Sections(result) => match *result {
                Ok(results) => match app.apply_results(results) {
                    Some(e) => app.status = format!("filter error: {e}"),
                    None => app.status = format!("updated {}", now_hms()),
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
        KeySpec::Left => code == KeyCode::Left,
        KeySpec::Right => code == KeyCode::Right,
        KeySpec::Esc => code == KeyCode::Esc,
    }
}

fn binding_matches(binding: &KeyBinding, code: KeyCode) -> bool {
    binding.0.iter().any(|spec| key_matches(*spec, code))
}

fn handle_key(
    code: KeyCode,
    app: &mut App,
    config: &Config,
    store: &Store,
    tx: &mpsc::UnboundedSender<Msg>,
    fetching: &Arc<AtomicBool>,
    token: &str,
) {
    let kb = &config.keybindings;
    if binding_matches(&kb.quit, code) {
        app.should_quit = true;
    } else if binding_matches(&kb.down, code) {
        app.next();
    } else if binding_matches(&kb.up, code) {
        app.prev();
    } else if binding_matches(&kb.next_section, code) {
        app.next_section();
    } else if binding_matches(&kb.prev_section, code) {
        app.prev_section();
    } else if binding_matches(&kb.open, code) {
        if let Some(url) = app.selected_url()
            && let Err(e) = open::that_detached(url)
        {
            app.status = format!("failed to open browser: {e}");
        }
    } else if binding_matches(&kb.done, code) {
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
    } else if binding_matches(&kb.refresh, code) {
        let spawned = spawn_fetch(
            tx,
            fetching,
            token.to_string(),
            config.sections.clone(),
            config.db_path.clone(),
        );
        app.status = if spawned {
            "refreshing...".into()
        } else {
            "fetch already in progress".into()
        };
    }
}
