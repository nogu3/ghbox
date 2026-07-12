use ghbox_core::config::{Column, Config, KeySpec, Keybindings, NamedColor, Theme, ThemeColor};
use ghbox_core::item::Item;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Tabs};

use crate::app::App;

pub fn draw(frame: &mut Frame, app: &App, config: &Config) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_tabs(frame, app, &config.theme, chunks[0]);
    draw_table(frame, app, config, chunks[1]);
    draw_status_bar(frame, app, config, chunks[2]);
}

fn color(c: ThemeColor) -> Color {
    match c {
        ThemeColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
        ThemeColor::Named(n) => match n {
            NamedColor::Black => Color::Black,
            NamedColor::Red => Color::Red,
            NamedColor::Green => Color::Green,
            NamedColor::Yellow => Color::Yellow,
            NamedColor::Blue => Color::Blue,
            NamedColor::Magenta => Color::Magenta,
            NamedColor::Cyan => Color::Cyan,
            NamedColor::Gray => Color::Gray,
            NamedColor::DarkGray => Color::DarkGray,
            NamedColor::LightRed => Color::LightRed,
            NamedColor::LightGreen => Color::LightGreen,
            NamedColor::LightYellow => Color::LightYellow,
            NamedColor::LightBlue => Color::LightBlue,
            NamedColor::LightMagenta => Color::LightMagenta,
            NamedColor::LightCyan => Color::LightCyan,
            NamedColor::White => Color::White,
        },
    }
}

fn draw_tabs(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let titles: Vec<Line> = app
        .sections
        .iter()
        .map(|s| Line::raw(format!("{} {}", s.title, s.items.len())))
        .collect();
    let tabs = Tabs::new(titles)
        .select(app.active)
        .style(Style::default().fg(color(theme.tab_inactive)))
        .highlight_style(
            Style::default()
                .fg(color(theme.tab_active))
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, area);
}

fn column_label(col: Column) -> &'static str {
    match col {
        Column::Repo => "repo",
        Column::Number => "#",
        Column::Title => "title",
        Column::Author => "author",
        Column::Comment => "comment",
        Column::Updated => "updated",
        Column::Created => "created",
    }
}

/// "2026-07-12T10:30:00Z" → "07-12 10:30". GitHub timestamps are ASCII;
/// anything unexpected is shown as-is.
fn fmt_ts(ts: &str) -> String {
    if ts.len() >= 16 && ts.is_ascii() {
        format!("{} {}", &ts[5..10], &ts[11..16])
    } else {
        ts.to_string()
    }
}

fn cell_text(item: &Item, col: Column) -> String {
    match col {
        Column::Repo => item.repo.clone(),
        Column::Number => format!("#{}", item.pr_number),
        Column::Title => item.pr_title.clone(),
        Column::Author => item.display_author().to_string(),
        Column::Comment => item
            .comment
            .as_ref()
            .and_then(|c| c.body.lines().next())
            .unwrap_or_default()
            .to_string(),
        Column::Updated => fmt_ts(&item.pr_updated_at),
        Column::Created => fmt_ts(match &item.comment {
            Some(c) => &c.created_at,
            None => &item.pr_created_at,
        }),
    }
}

fn column_constraint(col: Column, items: &[Item]) -> Constraint {
    match col {
        Column::Repo => {
            let max = items.iter().map(|i| i.repo.len()).max().unwrap_or(0);
            Constraint::Length(max.clamp(4, 30) as u16)
        }
        Column::Number => Constraint::Length(6),
        Column::Title => Constraint::Fill(1),
        Column::Author => Constraint::Length(12),
        Column::Comment => Constraint::Length(30),
        Column::Updated | Column::Created => Constraint::Length(11),
    }
}

fn draw_table(frame: &mut Frame, app: &App, config: &Config, area: Rect) {
    let theme = &config.theme;
    let columns = &config.sections[app.active].columns;
    let items = &app.active_section().items;

    let header = Row::new(columns.iter().map(|&c| Cell::from(column_label(c)))).style(
        Style::default()
            .fg(color(theme.table_header))
            .add_modifier(Modifier::BOLD),
    );
    let rows = items
        .iter()
        .map(|item| Row::new(columns.iter().map(|&c| Cell::from(cell_text(item, c)))));
    let widths: Vec<Constraint> = columns
        .iter()
        .map(|&c| column_constraint(c, items))
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(color(theme.border))),
        )
        .row_highlight_style(
            Style::default()
                .bg(color(theme.selection_bg))
                .fg(color(theme.selection_fg))
                .add_modifier(Modifier::BOLD),
        );

    let mut state = TableState::default();
    state.select(if items.is_empty() {
        None
    } else {
        Some(app.selected)
    });
    frame.render_stateful_widget(table, area, &mut state);
}

fn key_glyph(spec: KeySpec) -> String {
    match spec {
        KeySpec::Up => "↑".to_string(),
        KeySpec::Down => "↓".to_string(),
        KeySpec::Left => "←".to_string(),
        KeySpec::Right => "→".to_string(),
        other => other.to_string(),
    }
}

fn help_line(kb: &Keybindings) -> String {
    format!(
        "{}/{}:移動  {}:切替  {}:開く  {}:対応済み  {}:更新  {}:終了",
        key_glyph(kb.down.primary()),
        key_glyph(kb.up.primary()),
        key_glyph(kb.next_section.primary()),
        key_glyph(kb.open.primary()),
        key_glyph(kb.done.primary()),
        key_glyph(kb.refresh.primary()),
        key_glyph(kb.quit.primary()),
    )
}

fn draw_status_bar(frame: &mut Frame, app: &App, config: &Config, area: Rect) {
    let text = format!(" {} | {}", app.status, help_line(&config.keybindings));
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(color(config.theme.status_bar))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghbox_core::item::CommentInfo;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Concatenates every cell's symbol into one string for substring
    /// assertions. Skips the blank filler cell ratatui inserts after every
    /// double-width (e.g. CJK) character, so multi-byte text like
    /// "マージ依頼" reads back contiguous instead of space-separated.
    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let area = buffer.area();
        let mut out = String::new();
        for y in 0..area.height {
            let mut prev_wide = false;
            for x in 0..area.width {
                let symbol = buffer[(x, y)].symbol();
                if prev_wide && symbol == " " {
                    prev_wide = false;
                    continue;
                }
                prev_wide = symbol.len() > 1;
                out.push_str(symbol);
            }
        }
        out
    }

    #[test]
    fn renders_tabs_table_and_status() {
        let config = Config::default();
        let titles = config.sections.iter().map(|s| s.title.clone()).collect();
        let mut app = App::new(titles);
        app.sections[0].items.push(Item {
            repo: "nogu3/casa".into(),
            pr_number: 12,
            pr_title: "Fix xxx".into(),
            pr_url: "u".into(),
            pr_author: "alice".into(),
            pr_updated_at: "2026-07-12T10:30:00Z".into(),
            pr_created_at: "2026-07-01T00:00:00Z".into(),
            comment: Some(CommentInfo {
                id: 1,
                author: "bob".into(),
                body: "@nogu3 merge please\nsecond line".into(),
                created_at: "2026-07-11T00:00:00Z".into(),
            }),
        });
        app.status = "updated 12:34:56 UTC".into();
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal.draw(|f| draw(f, &app, &config)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("マージ依頼 1"), "tab bar with count");
        assert!(text.contains("レビュー依頼 0"), "inactive tab");
        assert!(text.contains("nogu3/casa"), "repo column");
        assert!(text.contains("#12"), "number column");
        assert!(text.contains("Fix xxx"), "title column");
        assert!(text.contains("@nogu3 merge please"), "comment first line");
        assert!(!text.contains("second line"), "only first line of comment");
        assert!(text.contains("updated 12:34:56 UTC"), "status bar");
        assert!(text.contains("q:終了"), "help from keybindings");
    }

    #[test]
    fn fmt_ts_formats_iso8601() {
        assert_eq!(fmt_ts("2026-07-12T10:30:00Z"), "07-12 10:30");
        assert_eq!(fmt_ts("garbage"), "garbage");
    }

    #[test]
    fn help_line_uses_primary_keys_with_arrow_glyphs() {
        let kb = Keybindings::default();
        assert_eq!(
            help_line(&kb),
            "↓/↑:移動  →:切替  o:開く  d:対応済み  r:更新  q:終了"
        );
    }
}
