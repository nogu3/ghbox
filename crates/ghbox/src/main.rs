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
    /// Built sections plus any GraphQL `errors` that came back with partial
    /// data (SAML-blocked org, etc.) — surfaced in the status bar.
    Sections(Box<ghbox_core::Result<(Vec<SectionResult>, Vec<String>)>>),
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
) -> ghbox_core::Result<(Vec<SectionResult>, Vec<String>)> {
    let fetched = github::fetch_sections(token, sections).await?;
    let store = Store::open(db_path)?;
    let results = build_sections(sections, &fetched, &store).await?;
    Ok((results, fetched.errors))
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
/// Clears the in-flight flag when the fetch task ends — including a panic,
/// which would otherwise leave the flag stuck and silently disable refresh
/// for the rest of the session.
struct FetchingGuard(Arc<AtomicBool>);

impl Drop for FetchingGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

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
    // Spinner ticks: the UI redraws only on messages, so while the fetch is
    // in flight something must pulse the event loop. The task ends itself
    // when the flag clears — FetchingGuard drops it even on a fetch panic.
    let tick_tx = tx.clone();
    let tick_fetching = Arc::clone(fetching);
    tokio::spawn(async move {
        while tick_fetching.load(Ordering::SeqCst) {
            if tick_tx.send(Msg::Redraw).is_err() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
    let tx = tx.clone();
    let guard = FetchingGuard(Arc::clone(fetching));
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let result = handle.block_on(fetch_and_build(&token, &sections, &db_path));
        // Clear the in-flight flag before notifying the UI so the spinner is
        // already off when the result is drawn. Unwind still drops the guard.
        drop(guard);
        let _ = tx.send(Msg::Sections(Box::new(result)));
    });
    true
}

fn main() -> Result<()> {
    // `time` refuses to read the local offset once the process is
    // multi-threaded (env-var soundness), so capture it before the tokio
    // runtime spawns its workers. Falls back to UTC if indeterminate.
    let local_offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?
        .block_on(async_main(local_offset))
}

async fn async_main(local_offset: time::UtcOffset) -> Result<()> {
    let config = Config::load().context("failed to load config")?;
    let token = github::get_token().context("failed to get token via `gh auth token`")?;
    let store = Store::open(&config.db_path)
        .with_context(|| format!("failed to open db at {}", config.db_path.display()))?;

    let terminal = ratatui::init();
    let result = run(terminal, config, token, store, local_offset).await;
    ratatui::restore();
    result
}

async fn run(
    mut terminal: ratatui::DefaultTerminal,
    config: Config,
    token: String,
    store: Store,
    local_offset: time::UtcOffset,
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
        // Default (Burst) would fire every tick missed during laptop sleep
        // back-to-back on resume; Delay fetches once and reschedules.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if fetch_tx.is_closed() {
                break;
            }
            // Skip this tick if a fetch is still in flight; no message needed.
            if spawn_fetch(
                &fetch_tx,
                &fetch_fetching,
                fetch_token.clone(),
                fetch_sections_cfg.clone(),
                fetch_db_path.clone(),
            ) {
                let _ = fetch_tx.send(Msg::Redraw);
            }
        }
    });

    let titles = config.sections.iter().map(|s| s.title.clone()).collect();
    let mut app = App::new(titles);
    terminal.draw(|f| ui::draw(f, &app, &config, fetching.load(Ordering::SeqCst)))?;

    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Key(key) => {
                handle_key(key.code, &mut app, &config, &store, &tx, &fetching, &token)
            }
            Msg::Sections(result) => match *result {
                Ok((results, api_errors)) => {
                    let filter_error = app.apply_results(results);
                    app.status = compose_status(now_hms(local_offset), filter_error, &api_errors);
                }
                Err(e) => app.status = format!("fetch error: {e}"),
            },
            Msg::Redraw => {}
        }
        if app.should_quit {
            break;
        }
        terminal.draw(|f| ui::draw(f, &app, &config, fetching.load(Ordering::SeqCst)))?;
    }
    Ok(())
}

/// Builds the post-fetch status line. A per-section filter error is the most
/// actionable signal so it leads; GraphQL `errors` (partial-failure warnings)
/// are appended so a section emptied by a SAML block can't read as "all clear".
fn compose_status(hms: String, filter_error: Option<String>, api_errors: &[String]) -> String {
    let base = match filter_error {
        Some(e) => format!("filter error: {e}"),
        None => hms,
    };
    if api_errors.is_empty() {
        base
    } else {
        format!("{base} ⚠ API: {}", api_errors.join("; "))
    }
}

/// HH:MM:SS in the local timezone captured at startup (shown next to the
/// ✓ icon).
fn now_hms(offset: time::UtcOffset) -> String {
    hms(time::OffsetDateTime::now_utc().to_offset(offset))
}

fn hms(t: time::OffsetDateTime) -> String {
    format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second())
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use time::{OffsetDateTime, UtcOffset};

    use super::{FetchingGuard, compose_status, hms};

    #[test]
    fn hms_applies_local_offset() {
        let epoch = OffsetDateTime::from_unix_timestamp(0).unwrap();
        let jst = UtcOffset::from_hms(9, 0, 0).unwrap();
        assert_eq!(hms(epoch.to_offset(jst)), "09:00:00");
        assert_eq!(hms(epoch), "00:00:00");
    }

    #[test]
    fn fetching_guard_resets_flag_on_drop() {
        let flag = Arc::new(AtomicBool::new(true));
        let guard = FetchingGuard(Arc::clone(&flag));
        assert!(flag.load(Ordering::SeqCst));
        drop(guard);
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn fetching_guard_resets_flag_on_panic() {
        // a panicking fetch task must not leave the in-flight flag stuck,
        // which would silently disable refresh for the rest of the session
        let flag = Arc::new(AtomicBool::new(true));
        let flag2 = Arc::clone(&flag);
        let _ = std::panic::catch_unwind(move || {
            let _guard = FetchingGuard(flag2);
            panic!("boom");
        });
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn status_is_plain_update_when_no_errors() {
        let s = compose_status("00:00:00".into(), None, &[]);
        assert_eq!(s, "00:00:00");
    }

    #[test]
    fn status_leads_with_filter_error() {
        let s = compose_status("00:00:00".into(), Some("sec: exited with 1".into()), &[]);
        assert_eq!(s, "filter error: sec: exited with 1");
    }

    #[test]
    fn status_appends_api_warnings_after_update() {
        let s = compose_status(
            "00:00:00".into(),
            None,
            &["SAML enforcement".to_string(), "rate limited".to_string()],
        );
        assert_eq!(s, "00:00:00 ⚠ API: SAML enforcement; rate limited");
    }

    #[test]
    fn status_shows_both_filter_error_and_api_warnings() {
        let s = compose_status(
            "00:00:00".into(),
            Some("sec: exited with 1".into()),
            &["SAML enforcement".to_string()],
        );
        assert_eq!(
            s,
            "filter error: sec: exited with 1 ⚠ API: SAML enforcement"
        );
    }
}
