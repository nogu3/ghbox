use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::app::{App, Section};

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_merge_section(frame, app, chunks[0]);
    draw_review_section(frame, app, chunks[1]);
    draw_status_bar(frame, app, chunks[2]);
}

/// Builds list rows with a repo header line whenever the repo changes.
/// Returns the rows and the row index corresponding to `selected` (item
/// index), if any.
fn build_rows<'a>(
    entries: impl Iterator<Item = (&'a str, Line<'a>)>,
    selected: usize,
) -> (Vec<ListItem<'a>>, Option<usize>) {
    let mut rows = Vec::new();
    let mut selected_row = None;
    let mut last_repo: Option<&str> = None;
    for (item_idx, (repo, line)) in entries.enumerate() {
        if last_repo != Some(repo) {
            rows.push(ListItem::new(Line::styled(
                format!("▍{repo}"),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            last_repo = Some(repo);
        }
        if item_idx == selected {
            selected_row = Some(rows.len());
        }
        rows.push(ListItem::new(line));
    }
    (rows, selected_row)
}

fn section_block(title: &str, active: bool) -> Block<'_> {
    let style = if active {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(title)
}

fn draw_merge_section(frame: &mut Frame, app: &App, area: Rect) {
    let active = app.section == Section::Merge;
    let entries = app.inbox.merge_requests.iter().map(|m| {
        let first_line = m.body.lines().next().unwrap_or_default();
        (
            m.repo.as_str(),
            Line::raw(format!(
                "  #{} {} — @{}: {}",
                m.pr_number, m.pr_title, m.author, first_line
            )),
        )
    });
    let selected = if active { app.selected } else { usize::MAX };
    let (rows, selected_row) = build_rows(entries, selected);
    let title = format!("マージ依頼 ({})", app.inbox.merge_requests.len());
    draw_list(frame, area, rows, selected_row, &title, active);
}

fn draw_review_section(frame: &mut Frame, app: &App, area: Rect) {
    let active = app.section == Section::Review;
    let entries = app.inbox.review_requests.iter().map(|r| {
        (
            r.repo.as_str(),
            Line::raw(format!(
                "  #{} {} — by @{}",
                r.pr_number, r.pr_title, r.author
            )),
        )
    });
    let selected = if active { app.selected } else { usize::MAX };
    let (rows, selected_row) = build_rows(entries, selected);
    let title = format!("レビュー依頼 ({})", app.inbox.review_requests.len());
    draw_list(frame, area, rows, selected_row, &title, active);
}

fn draw_list(
    frame: &mut Frame,
    area: Rect,
    rows: Vec<ListItem>,
    selected_row: Option<usize>,
    title: &str,
    active: bool,
) {
    let list = List::new(rows)
        .block(section_block(title, active))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    state.select(selected_row);
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let help = "j/k:移動  Tab:切替  Enter:開く  d:対応済み  r:更新  q:終了";
    let text = format!(" {} | {help}", app.status);
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}
