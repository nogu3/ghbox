mod app;
mod ui;

use anyhow::{Context, Result};
use crossterm::event::{Event, KeyCode, KeyEventKind};
use ghbox_core::config::Config;
use ghbox_core::filter::CommentFilter;
use ghbox_core::github::{self, Parsed};
use ghbox_core::inbox::build_inbox;
use ghbox_core::store::Store;
use tokio::sync::mpsc;

use crate::app::App;

enum Msg {
    Key(crossterm::event::KeyEvent),
    Fetched(Box<ghbox_core::Result<Parsed>>),
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
    let interval_secs = config.poll_interval_secs.max(30);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            let result = github::fetch(&fetch_token).await;
            if fetch_tx.send(Msg::Fetched(Box::new(result))).is_err() {
                break;
            }
        }
    });

    let mut app = App::new();
    terminal.draw(|f| ui::draw(f, &app))?;

    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Key(key) => handle_key(key.code, &mut app, &store, &tx, &token),
            Msg::Fetched(result) => match *result {
                Ok(parsed) => match CommentFilter::new(&parsed.viewer_login, &[]) {
                    Ok(filter) => match build_inbox(&parsed, &filter, &store) {
                        Ok(inbox) => {
                            app.set_inbox(inbox);
                            app.status = format!("updated {}", now_hms());
                        }
                        Err(e) => app.status = format!("error: {e}"),
                    },
                    Err(e) => app.status = format!("config error: {e}"),
                },
                Err(e) => app.status = format!("fetch error: {e}"),
            },
            Msg::Redraw => {}
        }
        if app.should_quit {
            break;
        }
        terminal.draw(|f| ui::draw(f, &app))?;
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

fn handle_key(
    code: KeyCode,
    app: &mut App,
    store: &Store,
    tx: &mpsc::UnboundedSender<Msg>,
    token: &str,
) {
    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down => app.next(),
        KeyCode::Char('k') | KeyCode::Up => app.prev(),
        KeyCode::Tab => app.toggle_section(),
        KeyCode::Enter => {
            if let Some(url) = app.selected_url()
                && let Err(e) = open::that_detached(url)
            {
                app.status = format!("failed to open browser: {e}");
            }
        }
        KeyCode::Char('d') => {
            if let Some((kind, key)) = app.selected_done_entry() {
                match store.mark_done(kind, &key) {
                    Ok(()) => {
                        app.remove_selected();
                        app.status = format!("done: {key}");
                    }
                    Err(e) => app.status = format!("db error: {e}"),
                }
            }
        }
        KeyCode::Char('r') => {
            app.status = "refreshing...".into();
            let tx = tx.clone();
            let token = token.to_string();
            tokio::spawn(async move {
                let result = github::fetch(&token).await;
                let _ = tx.send(Msg::Fetched(Box::new(result)));
            });
        }
        _ => {}
    }
}
