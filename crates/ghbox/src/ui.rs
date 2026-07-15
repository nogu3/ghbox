use ghbox_core::config::{Column, Config, KeySpec, Keybindings, NamedColor, Theme, ThemeColor};
use ghbox_core::item::{Item, PrState};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Cell, HighlightSpacing, Paragraph, Row, Table, TableState};
use unicode_width::UnicodeWidthStr;

use crate::app::App;

pub fn draw(frame: &mut Frame, app: &App, config: &Config, fetching: bool) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_tabs(frame, app, &config.theme, chunks[0]);
    draw_rule(frame, app, &config.theme, chunks[1]);
    draw_table(frame, app, config, chunks[2]);
    draw_status_bar(frame, app, config, fetching, chunks[3]);
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
    let dim = Style::default().fg(color(theme.tab_inactive));
    let mut spans = vec![Span::raw(" ")];
    for (i, s) in app.sections.iter().enumerate() {
        let title_style = if i == app.active {
            Style::default()
                .fg(color(theme.tab_active))
                .add_modifier(Modifier::BOLD)
        } else {
            dim
        };
        spans.push(Span::styled(s.title.clone(), title_style));
        let count = format!(" {}", s.items.len());
        let count_style = if i == app.active {
            Style::default().fg(color(theme.tab_active))
        } else {
            dim
        };
        spans.push(Span::styled(count, count_style));
        if i < app.sections.len() - 1 {
            spans.push(Span::styled(" │ ", dim));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// (x offset, display width) of the active tab's title in the tab line,
/// mirroring the span layout in `draw_tabs`. Drives the accent underline in
/// the rule below the tabs.
fn active_tab_range(app: &App) -> (u16, u16) {
    let mut x = 1u16; // leading space
    for (i, s) in app.sections.iter().enumerate() {
        let title_w = s.title.width() as u16;
        if i == app.active {
            return (x, title_w);
        }
        let count_w = format!(" {}", s.items.len()).width() as u16;
        x += title_w + count_w + " │ ".width() as u16;
    }
    (0, 0)
}

fn draw_rule(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let rule = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(rule).style(Style::default().fg(color(theme.border))),
        area,
    );
    let (x, w) = active_tab_range(app);
    if w == 0 || x >= area.width {
        return;
    }
    let w = w.min(area.width - x);
    let underline = Rect {
        x: area.x + x,
        y: area.y,
        width: w,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new("━".repeat(w as usize)).style(Style::default().fg(color(theme.tab_active))),
        underline,
    );
}

fn column_label(col: Column) -> &'static str {
    match col {
        Column::State => "",
        Column::Repo => "REPO",
        Column::Number => "#",
        Column::Title => "TITLE",
        Column::Author => "AUTHOR",
        Column::Comment => "COMMENT",
        Column::Updated => "UPDATED",
        Column::Created => "CREATED",
    }
}

/// "2026-07-12T10:30:00Z" → "2d" (relative to now_epoch, unix seconds).
/// Falls back to "MM-DD" past 30 days and to the raw string when unparsable.
fn fmt_relative(ts: &str, now_epoch: i64) -> String {
    let parsed = time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339);
    let Ok(t) = parsed else {
        return ts.to_string();
    };
    let delta = now_epoch - t.unix_timestamp();
    if delta < 60 {
        "now".to_string()
    } else if delta < 3600 {
        format!("{}m", delta / 60)
    } else if delta < 86_400 {
        format!("{}h", delta / 3600)
    } else if delta < 30 * 86_400 {
        format!("{}d", delta / 86_400)
    } else {
        ts[5..10].to_string() // RFC3339パース済みなのでASCII保証
    }
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Braille spinner frames, one per 100ms tick — a full revolution per second.
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Frame for a wall-clock time. Stateless: each redraw picks its frame from
/// the clock, so nothing has to count ticks or carry animation state.
fn spinner_frame(now_millis: u128) -> &'static str {
    SPINNER_FRAMES[(now_millis / 100 % SPINNER_FRAMES.len() as u128) as usize]
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn cell_text(item: &Item, col: Column, now_epoch: i64, icons: bool) -> String {
    match col {
        Column::State => state_icon(item.state, icons).to_string(),
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
        Column::Updated => fmt_relative(&item.pr_updated_at, now_epoch),
        Column::Created => fmt_relative(
            match &item.comment {
                Some(c) => &c.created_at,
                None => &item.pr_created_at,
            },
            now_epoch,
        ),
    }
}

/// repo/comment are de-emphasized via theme.faint; number/author/time are
/// themeable so users can match their terminal palette.
fn cell_style(col: Column, theme: &Theme, state: PrState) -> Style {
    match col {
        Column::State => Style::default().fg(color(state_color(state, theme))),
        Column::Repo | Column::Comment => Style::default().fg(color(theme.faint)),
        Column::Number => Style::default().fg(color(theme.pr_number)),
        Column::Title => Style::default(),
        Column::Author => Style::default().fg(color(theme.author)),
        Column::Updated | Column::Created => Style::default().fg(color(theme.time)),
    }
}

fn state_color(state: PrState, theme: &Theme) -> ThemeColor {
    match state {
        PrState::Open => theme.state_open,
        PrState::Draft => theme.state_draft,
        PrState::Merged => theme.state_merged,
        PrState::Closed => theme.state_closed,
    }
}

/// Nerd Font octicons; with `icons = false` a plain dot is used and the
/// state color alone carries the meaning.
fn state_icon(state: PrState, icons: bool) -> &'static str {
    if !icons {
        return "●";
    }
    match state {
        PrState::Open => "\u{f407}",   // nf-oct-git_pull_request
        PrState::Draft => "\u{f4dd}",  // nf-oct-git_pull_request_draft
        PrState::Merged => "\u{f419}", // nf-oct-git_merge
        PrState::Closed => "\u{f4dc}", // nf-oct-git_pull_request_closed
    }
}

fn column_constraint(col: Column, items: &[Item]) -> Constraint {
    match col {
        Column::State => Constraint::Length(2),
        Column::Repo => {
            let max = items.iter().map(|i| i.repo.len()).max().unwrap_or(0);
            Constraint::Length(max.clamp(4, 30) as u16)
        }
        Column::Number => Constraint::Length(6),
        Column::Title => Constraint::Fill(1),
        Column::Author => Constraint::Length(12),
        Column::Comment => Constraint::Length(30),
        Column::Updated | Column::Created => Constraint::Length(7),
    }
}

fn draw_table(frame: &mut Frame, app: &App, config: &Config, area: Rect) {
    let theme = &config.theme;
    let columns = &config.sections[app.active].columns;
    let items = &app.active_section().items;

    if items.is_empty() {
        if area.height == 0 {
            return;
        }
        let centered = Rect {
            x: area.x,
            y: area.y + area.height / 2,
            width: area.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new("All clear — no items")
                .style(Style::default().fg(color(theme.faint)))
                .alignment(Alignment::Center),
            centered,
        );
        return;
    }

    let now = now_epoch();
    let header = Row::new(columns.iter().map(|&c| Cell::from(column_label(c)))).style(
        Style::default()
            .fg(color(theme.table_header))
            .add_modifier(Modifier::BOLD),
    );
    // row_highlight_style patches the whole selected row area (including the
    // highlight symbol), so a fg there would clobber the marker's accent
    // color. Instead the selected row's fg is set per cell here and the
    // highlight style only carries bg + BOLD.
    let rows = items.iter().enumerate().map(|(i, item)| {
        Row::new(columns.iter().map(|&c| {
            let style = if i == app.selected && c != Column::State {
                Style::default().fg(color(theme.selection_fg))
            } else {
                cell_style(c, theme, item.state)
            };
            Cell::from(cell_text(item, c, now, config.icons)).style(style)
        }))
    });
    let widths: Vec<Constraint> = columns
        .iter()
        .map(|&c| column_constraint(c, items))
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(
            Style::default()
                .bg(color(theme.selection_bg))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(Text::styled(
            "▌ ",
            Style::default().fg(color(theme.tab_active)),
        ))
        .highlight_spacing(HighlightSpacing::Always);

    let mut state = TableState::default();
    state.select(Some(app.selected));
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
        "{}{} move · {}{} section · {} open · {} done · {} refresh · {} quit",
        key_glyph(kb.down.primary()),
        key_glyph(kb.up.primary()),
        key_glyph(kb.prev_section.primary()),
        key_glyph(kb.next_section.primary()),
        key_glyph(kb.open.primary()),
        key_glyph(kb.done.primary()),
        key_glyph(kb.refresh.primary()),
        key_glyph(kb.quit.primary()),
    )
}

fn draw_status_bar(frame: &mut Frame, app: &App, config: &Config, fetching: bool, area: Rect) {
    let theme = &config.theme;
    let (icon, icon_color) = if fetching {
        (spinner_frame(now_millis()), color(theme.tab_active))
    } else {
        ("✓", color(theme.state_open))
    };
    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled(icon, Style::default().fg(icon_color)),
        Span::raw(format!(" {}", app.status)),
        Span::styled(
            format!(" · {}", help_line(&config.keybindings)),
            Style::default().fg(color(theme.status_bar)),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
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
                prev_wide = symbol.width() > 1;
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
            state: PrState::Open,
            comment: Some(CommentInfo {
                id: 1,
                author: "bob".into(),
                body: "@nogu3 merge please\nsecond line".into(),
                created_at: "2026-07-11T00:00:00Z".into(),
            }),
        });
        app.status = "12:34:56".into();
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal.draw(|f| draw(f, &app, &config, false)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("Merge Requests 1 │ Review Requests 0"),
            "hand-built tab line with │ divider"
        );
        assert!(text.contains("─────"), "horizontal rule under tabs");
        assert!(text.contains("nogu3/casa"), "repo column");
        assert!(text.contains("#12"), "number column");
        assert!(text.contains("Fix xxx"), "title column");
        assert!(text.contains("@nogu3 merge please"), "comment first line");
        assert!(!text.contains("second line"), "only first line of comment");
        assert!(text.contains("✓ 12:34:56"), "status with idle icon");
        assert!(text.contains("q quit"), "english help from keybindings");
        assert!(text.contains("REPO"), "uppercase header");
        assert!(text.contains("TITLE"), "uppercase header");
        assert!(text.contains("▌"), "selection marker");
        assert!(!text.contains("┌"), "no outer border");
    }

    #[test]
    fn selection_marker_keeps_accent_and_unselected_rows_keep_column_colors() {
        let config = Config::default();
        let titles = config.sections.iter().map(|s| s.title.clone()).collect();
        let mut app = App::new(titles);
        for n in [1, 2] {
            app.sections[0].items.push(Item {
                repo: "nogu3/casa".into(),
                pr_number: n,
                pr_title: format!("PR {n}"),
                pr_url: "u".into(),
                pr_author: "alice".into(),
                pr_updated_at: "2026-07-12T10:30:00Z".into(),
                pr_created_at: "2026-07-01T00:00:00Z".into(),
                state: PrState::Open,
                comment: None,
            });
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal.draw(|f| draw(f, &app, &config, false)).unwrap();
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        let cells = || (0..area.height).flat_map(|y| (0..area.width).map(move |x| (x, y)));

        // マーカーは row_highlight_style の fg に潰されず accent(tab_active=mauve)を保つ
        let (mx, my) = cells()
            .find(|&(x, y)| buffer[(x, y)].symbol() == "▌")
            .expect("selection marker cell");
        assert_eq!(
            buffer[(mx, my)].fg,
            Color::Rgb(0xcb, 0xa6, 0xf7),
            "marker keeps accent fg"
        );
        // 選択行本体は selection_fg on selection_bg
        // (highlight symbol "▌ " の2桁 + state 列の2桁の先、repo 列の中の "n")
        let body = &buffer[(mx + 5, my)];
        assert_eq!(body.bg, Color::Rgb(0x31, 0x32, 0x44), "selected row bg");
        assert_eq!(body.fg, Color::Rgb(0xcd, 0xd6, 0xf4), "selected row fg");
        // 非選択行の #number セルはカラム色(pr_number=blue)を保つ
        assert!(
            cells().any(|(x, y)| buffer[(x, y)].fg == Color::Rgb(0x89, 0xb4, 0xfa)),
            "unselected row keeps column color"
        );
    }

    #[test]
    fn empty_section_shows_no_items_placeholder() {
        let config = Config::default();
        let titles = config.sections.iter().map(|s| s.title.clone()).collect();
        let app = App::new(titles);
        let mut terminal = Terminal::new(TestBackend::new(60, 10)).unwrap();
        terminal.draw(|f| draw(f, &app, &config, false)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("All clear — no items"),
            "placeholder for empty section"
        );
        assert!(!text.contains("REPO"), "header hidden when empty");
    }

    #[test]
    fn active_tab_range_accounts_for_cjk_width() {
        let mut app = App::new(vec!["マージ依頼".into(), "b".into()]);
        // 先頭タブ: leading space の直後から、表示幅10
        assert_eq!(active_tab_range(&app), (1, 10));
        // 2番目: 1 + 10(title) + 2(" 0") + 3(" │ ") = 16
        app.active = 1;
        assert_eq!(active_tab_range(&app), (16, 1));
    }

    #[test]
    fn rule_underlines_active_tab_in_accent() {
        let config = Config::default();
        let titles = config.sections.iter().map(|s| s.title.clone()).collect();
        let app = App::new(titles);
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal.draw(|f| draw(f, &app, &config, false)).unwrap();
        let buffer = terminal.backend().buffer();
        // "Merge Requests" は幅14: x=1..=14 が ━ (accent)、その先は ─ (border)
        assert_eq!(buffer[(1, 1)].symbol(), "━");
        assert_eq!(buffer[(1, 1)].fg, Color::Rgb(0xcb, 0xa6, 0xf7));
        assert_eq!(buffer[(14, 1)].symbol(), "━");
        assert_eq!(buffer[(15, 1)].symbol(), "─");
        assert_eq!(buffer[(15, 1)].fg, Color::Rgb(0x45, 0x47, 0x5a));
    }

    fn epoch(ts: &str) -> i64 {
        time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339)
            .unwrap()
            .unix_timestamp()
    }

    #[test]
    fn fmt_relative_boundaries() {
        let now = epoch("2026-07-13T12:00:00Z");
        assert_eq!(fmt_relative("2026-07-13T11:59:30Z", now), "now");
        assert_eq!(fmt_relative("2026-07-13T11:30:00Z", now), "30m");
        assert_eq!(fmt_relative("2026-07-13T07:00:00Z", now), "5h");
        assert_eq!(fmt_relative("2026-07-11T12:00:00Z", now), "2d");
        assert_eq!(fmt_relative("2026-05-01T00:00:00Z", now), "05-01");
        // クロックスキューで未来になったタイムスタンプは "now" 扱い
        assert_eq!(fmt_relative("2026-07-14T00:00:00Z", now), "now");
        assert_eq!(fmt_relative("garbage", now), "garbage");
    }

    #[test]
    fn help_line_uses_primary_keys_with_arrow_glyphs() {
        let kb = Keybindings::default();
        assert_eq!(
            help_line(&kb),
            "↓↑ move · ←→ section · o open · d done · r refresh · q quit"
        );
    }

    #[test]
    fn spinner_frame_cycles_every_100ms() {
        assert_eq!(spinner_frame(0), "⠋");
        assert_eq!(spinner_frame(100), "⠙");
        assert_eq!(spinner_frame(950), "⠏");
        assert_eq!(spinner_frame(1000), "⠋"); // 1秒で一巡
    }

    #[test]
    fn status_bar_shows_spinner_while_fetching() {
        let config = Config::default();
        let titles = config.sections.iter().map(|s| s.title.clone()).collect();
        let mut app = App::new(titles);
        app.status = "refreshing...".into();
        let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
        terminal.draw(|f| draw(f, &app, &config, true)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            SPINNER_FRAMES.iter().any(|f| text.contains(f)),
            "animated spinner frame in status bar, got: {text}"
        );
        assert!(text.contains("refreshing..."), "status text");
        assert!(!text.contains("⟳"), "static icon replaced by animation");
    }

    fn item_with_state(n: u64, state: PrState) -> Item {
        Item {
            repo: "o/r".into(),
            pr_number: n,
            pr_title: format!("PR {n}"),
            pr_url: "u".into(),
            pr_author: "a".into(),
            pr_updated_at: "2026-07-12T10:30:00Z".into(),
            pr_created_at: "2026-07-01T00:00:00Z".into(),
            state,
            comment: None,
        }
    }

    #[test]
    fn state_column_shows_colored_icon_per_state_even_when_selected() {
        let config = Config::default();
        let titles = config.sections.iter().map(|s| s.title.clone()).collect();
        let mut app = App::new(titles);
        for (n, state) in [
            (1, PrState::Open),
            (2, PrState::Draft),
            (3, PrState::Merged),
            (4, PrState::Closed),
        ] {
            app.sections[0].items.push(item_with_state(n, state));
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal.draw(|f| draw(f, &app, &config, false)).unwrap();
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        let find = |glyph: &str| {
            (0..area.height)
                .flat_map(|y| (0..area.width).map(move |x| (x, y)))
                .find(|&(x, y)| buffer[(x, y)].symbol() == glyph)
                .unwrap_or_else(|| panic!("glyph {glyph:?} not rendered"))
        };
        // 1行目(選択行)の open アイコンも selection_fg に潰されず状態色を保つ
        let (x, y) = find("\u{f407}");
        assert_eq!(
            buffer[(x, y)].fg,
            Color::Rgb(0xa6, 0xe3, 0xa1),
            "open=green"
        );
        let (x, y) = find("\u{f4dd}");
        assert_eq!(
            buffer[(x, y)].fg,
            Color::Rgb(0x6c, 0x70, 0x86),
            "draft=overlay"
        );
        let (x, y) = find("\u{f419}");
        assert_eq!(
            buffer[(x, y)].fg,
            Color::Rgb(0xcb, 0xa6, 0xf7),
            "merged=mauve"
        );
        let (x, y) = find("\u{f4dc}");
        assert_eq!(
            buffer[(x, y)].fg,
            Color::Rgb(0xf3, 0x8b, 0xa8),
            "closed=red"
        );
    }

    #[test]
    fn icons_false_falls_back_to_colored_dot() {
        let mut config = Config::default();
        config.icons = false;
        let titles = config.sections.iter().map(|s| s.title.clone()).collect();
        let mut app = App::new(titles);
        app.sections[0]
            .items
            .push(item_with_state(1, PrState::Open));
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal.draw(|f| draw(f, &app, &config, false)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("●"), "plain dot fallback");
        assert!(!text.contains("\u{f407}"), "no nerd font glyph");
    }
}
