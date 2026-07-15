# Fetch Spinner Animation + UI Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** fetch 中に braille スピナーをアニメーションさせ、デフォルトテーマを catppuccin mocha RGB に刷新し、PR 状態アイコンカラムとタブ下線などの装飾を追加する。

**Architecture:** TUI は `crates/ghbox`(描画 ui.rs / イベントループ main.rs)、データは `crates/ghbox-core`(config.rs / github.rs / item.rs / inbox.rs)。スピナーは fetch 中のみ 100ms ごとに `Msg::Redraw` を送る tick タスク + 壁時計からフレームを導出する純関数。PR 状態は GraphQL に `state`/`isDraft` を追加取得して `Item.state: PrState` に載せ、新カラム `Column::State` で色付き Nerd Font アイコン表示する。

**Tech Stack:** Rust (edition 2024), ratatui 0.30, tokio, serde/toml, GitHub GraphQL v4

**Spec:** `docs/superpowers/specs/2026-07-15-spinner-ui-polish-design.md`

## Global Constraints

- コミット前に必ず: `cargo test --workspace` 全パス、`cargo clippy --workspace -- -D warnings` クリーン、`cargo fmt --all`
- コミットメッセージは英語・conventional commits
- config の後方互換を維持: 新規キーはすべて serde default 付き。`deny_unknown_fields` は維持
- DB スキーマ変更なし
- ratatui の罠: `Table::row_highlight_style` は選択行全域(highlight_symbol 列含む)に後から patch されるため fg を入れてはいけない。highlight style は bg+BOLD のみ、選択行の fg は rows 構築時に per-cell で設定する(ui.rs にコメントあり)

---

### Task 1: fetch スピナーアニメーション

**Files:**
- Modify: `crates/ghbox/src/ui.rs`(spinner_frame 追加、draw_status_bar 変更、テスト)
- Modify: `crates/ghbox/src/main.rs`(spawn_fetch に tick タスク追加)

**Interfaces:**
- Consumes: 既存の `Msg::Redraw`、`fetching: Arc<AtomicBool>`、`FetchingGuard`
- Produces: `fn spinner_frame(now_millis: u128) -> &'static str` と `const SPINNER_FRAMES: [&str; 10]`(ui.rs 内部。後続タスクは依存しない)

- [ ] **Step 1: 失敗するテストを書く**

`crates/ghbox/src/ui.rs` の `mod tests` に追加:

```rust
    #[test]
    fn spinner_frame_cycles_every_100ms() {
        assert_eq!(spinner_frame(0), "⠋");
        assert_eq!(spinner_frame(100), "⠙");
        assert_eq!(spinner_frame(950), "⠏");
        assert_eq!(spinner_frame(1000), "⠋"); // 1秒で一巡
    }
```

既存の `status_bar_shows_spinner_while_fetching` の末尾2行を差し替え:

```rust
        let text = buffer_text(&terminal);
        assert!(
            SPINNER_FRAMES.iter().any(|f| text.contains(f)),
            "animated spinner frame in status bar, got: {text}"
        );
        assert!(text.contains("refreshing..."), "status text");
        assert!(!text.contains("⟳"), "static icon replaced by animation");
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p ghbox spinner`
Expected: コンパイルエラー(`spinner_frame` / `SPINNER_FRAMES` 未定義)

- [ ] **Step 3: 実装**

`crates/ghbox/src/ui.rs` の `now_epoch()` の下に追加:

```rust
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
```

`draw_status_bar` の `let icon = ...` 行を差し替え:

```rust
    let icon = if fetching {
        spinner_frame(now_millis())
    } else {
        "✓"
    };
```

`crates/ghbox/src/main.rs` の `spawn_fetch` を差し替え(tick タスクの追加のみ。既存の blocking 部は不変):

```rust
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
```

- [ ] **Step 4: テスト・lint・fmt**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all`
Expected: 全パス

- [ ] **Step 5: コミット**

```bash
git add crates/ghbox/src/ui.rs crates/ghbox/src/main.rs
git commit -m "feat(tui): animate braille spinner while fetching"
```

---

### Task 2: catppuccin mocha RGB デフォルトテーマ + 新テーマキー

**Files:**
- Modify: `crates/ghbox-core/src/config.rs`(Theme 構造体・Default・テスト)
- Modify: `crates/ghbox/src/ui.rs`(faint 使用、✓ の色、テストの色アサーション更新)

**Interfaces:**
- Consumes: 既存の `ThemeColor`(Rgb variant は実装済み)
- Produces: `Theme` の新フィールド `faint`, `state_open`, `state_draft`, `state_merged`, `state_closed`(すべて `ThemeColor`)。Task 4 が `state_*` を使う

- [ ] **Step 1: 失敗するテストを書く**

`crates/ghbox-core/src/config.rs` の `mod tests` に追加:

```rust
    #[test]
    fn theme_defaults_are_catppuccin_rgb() {
        let cfg = parse("").unwrap();
        assert_eq!(cfg.theme.tab_active, ThemeColor::Rgb(0xcb, 0xa6, 0xf7));
        assert_eq!(cfg.theme.selection_bg, ThemeColor::Rgb(0x31, 0x32, 0x44));
        assert_eq!(cfg.theme.faint, ThemeColor::Rgb(0x6c, 0x70, 0x86));
        assert_eq!(cfg.theme.state_open, ThemeColor::Rgb(0xa6, 0xe3, 0xa1));
        assert_eq!(cfg.theme.state_merged, ThemeColor::Rgb(0xcb, 0xa6, 0xf7));
    }

    #[test]
    fn theme_new_keys_override() {
        let cfg = parse("[theme]\nfaint = \"gray\"\nstate_open = \"#00ff00\"\n").unwrap();
        assert_eq!(cfg.theme.faint, ThemeColor::Named(NamedColor::Gray));
        assert_eq!(cfg.theme.state_open, ThemeColor::Rgb(0x00, 0xff, 0x00));
    }
```

既存テストの期待値を更新:

- `theme_defaults_and_partial_override`: `selection_bg` の期待を `ThemeColor::Rgb(0x31, 0x32, 0x44)` に
- `theme_column_colors_default_and_override`: `pr_number` → `ThemeColor::Rgb(0x89, 0xb4, 0xfa)`、`author` → `ThemeColor::Rgb(0xf5, 0xc2, 0xe7)`、`time` → `ThemeColor::Rgb(0x7f, 0x84, 0x9c)` に

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p ghbox-core theme`
Expected: コンパイルエラー(`faint` フィールド未定義)

- [ ] **Step 3: config.rs を実装**

`Theme` 構造体に追加(既存10フィールドの後):

```rust
    /// De-emphasized text: repo/comment columns, empty-state placeholder.
    pub faint: ThemeColor,
    pub state_open: ThemeColor,
    pub state_draft: ThemeColor,
    pub state_merged: ThemeColor,
    pub state_closed: ThemeColor,
```

`impl Default for Theme` を差し替え(`use NamedColor::*;` は不要になるので削除):

```rust
impl Default for Theme {
    // catppuccin mocha
    fn default() -> Self {
        Self {
            tab_active: ThemeColor::Rgb(0xcb, 0xa6, 0xf7),   // mauve
            tab_inactive: ThemeColor::Rgb(0x6c, 0x70, 0x86), // overlay0
            border: ThemeColor::Rgb(0x45, 0x47, 0x5a),       // surface1
            selection_bg: ThemeColor::Rgb(0x31, 0x32, 0x44), // surface0
            selection_fg: ThemeColor::Rgb(0xcd, 0xd6, 0xf4), // text
            table_header: ThemeColor::Rgb(0xb4, 0xbe, 0xfe), // lavender
            status_bar: ThemeColor::Rgb(0x6c, 0x70, 0x86),   // overlay0
            pr_number: ThemeColor::Rgb(0x89, 0xb4, 0xfa),    // blue
            author: ThemeColor::Rgb(0xf5, 0xc2, 0xe7),       // pink
            time: ThemeColor::Rgb(0x7f, 0x84, 0x9c),         // overlay1
            faint: ThemeColor::Rgb(0x6c, 0x70, 0x86),        // overlay0
            state_open: ThemeColor::Rgb(0xa6, 0xe3, 0xa1),   // green
            state_draft: ThemeColor::Rgb(0x6c, 0x70, 0x86),  // overlay0
            state_merged: ThemeColor::Rgb(0xcb, 0xa6, 0xf7), // mauve
            state_closed: ThemeColor::Rgb(0xf3, 0x8b, 0xa8), // red
        }
    }
}
```

注意: `Theme` は `#[serde(default, deny_unknown_fields)]` なので新キーは自動で後方互換。

- [ ] **Step 4: ghbox 側を追随させる**

`crates/ghbox/src/ui.rs`:

`cell_style` の Repo/Comment 行を差し替え(ハードコード DarkGray → テーマ):

```rust
        Column::Repo | Column::Comment => Style::default().fg(color(theme.faint)),
```

このとき関数ドキュメントコメントも更新: `/// repo/comment are de-emphasized via theme.faint; number/author/time are\n/// themeable so users can match their terminal palette.`

`draw_status_bar` の icon を色分き Span にする(idle ✓ は state_open の緑、スピナーは accent):

```rust
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
```

`mod tests` の `selection_marker_keeps_accent_and_unselected_rows_keep_column_colors` の色アサーションを新デフォルトに更新:

```rust
        assert_eq!(
            buffer[(mx, my)].fg,
            Color::Rgb(0xcb, 0xa6, 0xf7),
            "marker keeps accent fg"
        );
        // 選択行本体は selection_fg on selection_bg
        let body = &buffer[(mx + 3, my)];
        assert_eq!(body.bg, Color::Rgb(0x31, 0x32, 0x44), "selected row bg");
        assert_eq!(body.fg, Color::Rgb(0xcd, 0xd6, 0xf4), "selected row fg");
        // 非選択行の #number セルはカラム色(pr_number=blue)を保つ
        assert!(
            cells().any(|(x, y)| buffer[(x, y)].fg == Color::Rgb(0x89, 0xb4, 0xfa)),
            "unselected row keeps column color"
        );
```

(コメント中の「Yellow」「Green」への言及も blue/mauve 等に直す)

- [ ] **Step 5: テスト・lint・fmt**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all`
Expected: 全パス

- [ ] **Step 6: コミット**

```bash
git add crates/ghbox-core/src/config.rs crates/ghbox/src/ui.rs
git commit -m "feat(theme): catppuccin mocha rgb defaults with faint and state colors"
```

---

### Task 3: PR 状態(PrState)の取得と Item への追加

**Files:**
- Modify: `crates/ghbox-core/src/item.rs`(PrState enum、Item.state、テスト)
- Modify: `crates/ghbox-core/src/github.rs`(クエリ・PrNode・PrData・parse、テスト)
- Modify: `crates/ghbox-core/src/inbox.rs`(pr_item コピー、テストヘルパ)
- Modify: `crates/ghbox-core/src/filter.rs`(テストヘルパの Item リテラルのみ)
- Modify: `crates/ghbox/src/ui.rs`, `crates/ghbox/src/app.rs`(テストの Item リテラルのみ)

**Interfaces:**
- Consumes: 既存の `PrData` / `Item` / `build_query` / `parse_sections`
- Produces: `pub enum PrState { Open, Draft, Merged, Closed }`(`ghbox_core::item::PrState`、`Copy + Serialize(lowercase)`)、`Item.state: PrState` フィールド、`PrData.state: PrState`。Task 4 が表示に使う

- [ ] **Step 1: 失敗するテストを書く**

`crates/ghbox-core/src/item.rs` の `serializes_all_fields` テストに追加:

```rust
        assert_eq!(json["state"], "open");
```

`crates/ghbox-core/src/github.rs`:

`build_query_aliases_sections_and_passes_variables` に追加:

```rust
        assert!(query.contains("state"), "PR state requested");
        assert!(query.contains("isDraft"), "draft flag requested");
```

`SECTIONS_FIXTURE` を更新: s0 の number 9 ノードに `"state": "OPEN", "isDraft": true,` を(`"number": 9,` の直後に)追加。s1 の number 12 ノードに `"state": "MERGED",` を追加。さらに s1 の nodes 配列に state キーを持たないノードを1つ追加:

```json
            {
              "number": 13,
              "title": "No state field",
              "url": "https://github.com/nogu3/hestia/pull/13",
              "updatedAt": "2026-07-02T00:00:00Z",
              "createdAt": "2026-07-01T00:00:00Z",
              "author": { "login": "alice" },
              "repository": { "nameWithOwner": "nogu3/hestia" }
            }
```

`parse_sections_returns_ordered_sections` に追加(既存アサーションは維持、`sections[1].len()` を見るものがあれば 2 に更新):

```rust
        assert_eq!(pr.state, PrState::Draft, "OPEN + isDraft => Draft");
        assert_eq!(rr.state, PrState::Merged);
        // state フィールドが無いレスポンス(古い挙動)は Open に落ちる
        assert_eq!(fetched.sections[1][1].state, PrState::Open);
```

テストモジュールのため `use crate::item::PrState;` を github.rs tests に追加。

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p ghbox-core`
Expected: コンパイルエラー(`PrState` 未定義)

- [ ] **Step 3: item.rs を実装**

`Item` 構造体定義の上に追加:

```rust
/// PR state for the state-icon column. `Draft` is derived at parse time from
/// GraphQL `state == OPEN && isDraft`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
}
```

`Item` の `pr_created_at` フィールドの直後に追加:

```rust
    pub state: PrState,
```

item.rs テストヘルパ `pr_item()` のリテラルに `state: PrState::Open,` を追加(`comment: None,` の前)。

- [ ] **Step 4: github.rs を実装**

`use crate::item::CommentInfo;` を `use crate::item::{CommentInfo, PrState};` に変更。

`PrData` に追加(`pr_created_at` の後):

```rust
    pub state: PrState,
```

`PrNode` に追加(`created_at` の後。`rename_all = "camelCase"` が `isDraft` を吸う):

```rust
    state: String,
    is_draft: bool,
```

`build_query` の PullRequest フィールド部で `url\n` の後に `        state\n        isDraft\n` を挿入。format 文字列全体は:

```rust
        query.push_str(&format!(
            "  s{i}: search(query: $q{i}, type: ISSUE, first: 50) {{\n    nodes {{\n      ... on PullRequest {{\n        number\n        title\n        url\n        state\n        isDraft\n        updatedAt\n        createdAt\n        author {{ login }}\n        repository {{ nameWithOwner }}{comments}\n      }}\n    }}\n  }}\n"
        ));
```

`parse_sections` の `prs.push(PrData {` の直前に導出を追加し、リテラルに `state,` を追加:

```rust
            // Draft only exists while OPEN; MERGED/CLOSED win regardless of
            // the flag. Missing `state` (defaulted "") reads as Open.
            let state = match node.state.as_str() {
                "MERGED" => PrState::Merged,
                "CLOSED" => PrState::Closed,
                _ if node.is_draft => PrState::Draft,
                _ => PrState::Open,
            };
```

- [ ] **Step 5: 残りの Item/PrData リテラルを追随させる**

すべて `state: PrState::Open,` を追加(機械的。import が無いモジュールには `use crate::item::PrState;` / `use ghbox_core::item::PrState;` を追加):

- `crates/ghbox-core/src/inbox.rs` `fn pr_item(pr: &PrData) -> Item`: `state: pr.state,` を追加(Open 固定ではなく伝播)
- `crates/ghbox-core/src/inbox.rs` tests `fn pr_data(...)`: `state: PrState::Open,`
- `crates/ghbox-core/src/filter.rs` tests `fn pr_item(...)`: `state: PrState::Open,`
- `crates/ghbox/src/ui.rs` tests の Item リテラル2箇所: `state: PrState::Open,`(ui.rs 先頭の import を `use ghbox_core::item::{Item, PrState};` に変更 — 現在は `use ghbox_core::item::Item;`。tests は `use super::*` で見える)
- `crates/ghbox/src/app.rs` tests `fn pr_item(...)`: `state: PrState::Open,`(tests モジュールに `use ghbox_core::item::PrState;` を追加)

- [ ] **Step 6: テスト・lint・fmt**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all`
Expected: 全パス

- [ ] **Step 7: コミット**

```bash
git add crates/ghbox-core/src crates/ghbox/src
git commit -m "feat(core): fetch pr state and is_draft into items"
```

---

### Task 4: State カラム + icons フラグ

**Files:**
- Modify: `crates/ghbox-core/src/config.rs`(Column::State、icons フラグ、デフォルトカラム、テスト)
- Modify: `crates/ghbox/src/ui.rs`(state_icon / state_color、rows 構築の選択行例外、テスト)

**Interfaces:**
- Consumes: Task 2 の `theme.state_*`、Task 3 の `Item.state: PrState`
- Produces: `Column::State` variant(config 文字列 `"state"`)、`Config.icons: bool`(デフォルト true)

- [ ] **Step 1: 失敗するテストを書く(config)**

`crates/ghbox-core/src/config.rs` の `mod tests` に追加:

```rust
    #[test]
    fn default_sections_lead_with_state_column_and_icons_on() {
        let cfg = Config::load_from(Path::new("/nonexistent/config.toml")).unwrap();
        assert!(cfg.icons);
        assert_eq!(cfg.sections[0].columns[0], Column::State);
        assert_eq!(cfg.sections[1].columns[0], Column::State);
    }

    #[test]
    fn icons_flag_parses() {
        let cfg = parse("icons = false\n").unwrap();
        assert!(!cfg.icons);
    }

    #[test]
    fn state_column_parses() {
        let cfg =
            parse("[[sections]]\ntitle = \"t\"\nquery = \"q\"\ncolumns = [\"state\", \"title\"]\n")
                .unwrap();
        assert_eq!(cfg.sections[0].columns[0], Column::State);
    }
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p ghbox-core config`
Expected: コンパイルエラー(`Column::State` / `icons` 未定義)

- [ ] **Step 3: config.rs を実装**

`Column` enum の先頭に `State,` を追加(`rename_all = "lowercase"` で `"state"`)。

`Config` 構造体に追加(`poll_interval_secs` の後):

```rust
    /// Nerd Font state icons in the `state` column; set false for plain
    /// terminals — the column falls back to a colored dot.
    pub icons: bool,
```

`impl Default for Config` に `icons: true,` を追加。

`default_sections()` の両セクションと `default_columns()` の columns 先頭に `Column::State,` を追加。

- [ ] **Step 4: 失敗するテストを書く(ui)**

`crates/ghbox/src/ui.rs` の `mod tests` に追加:

```rust
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
        assert_eq!(buffer[(x, y)].fg, Color::Rgb(0xa6, 0xe3, 0xa1), "open=green");
        let (x, y) = find("\u{f4dd}");
        assert_eq!(buffer[(x, y)].fg, Color::Rgb(0x6c, 0x70, 0x86), "draft=overlay");
        let (x, y) = find("\u{f419}");
        assert_eq!(buffer[(x, y)].fg, Color::Rgb(0xcb, 0xa6, 0xf7), "merged=mauve");
        let (x, y) = find("\u{f4dc}");
        assert_eq!(buffer[(x, y)].fg, Color::Rgb(0xf3, 0x8b, 0xa8), "closed=red");
    }

    #[test]
    fn icons_false_falls_back_to_colored_dot() {
        let mut config = Config::default();
        config.icons = false;
        let titles = config.sections.iter().map(|s| s.title.clone()).collect();
        let mut app = App::new(titles);
        app.sections[0].items.push(item_with_state(1, PrState::Open));
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal.draw(|f| draw(f, &app, &config, false)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("●"), "plain dot fallback");
        assert!(!text.contains("\u{f407}"), "no nerd font glyph");
    }
```

- [ ] **Step 5: テストが失敗することを確認**

Run: `cargo test -p ghbox state_column`
Expected: FAIL(glyph not rendered)ないしコンパイルエラー

- [ ] **Step 6: ui.rs を実装**

import に `PrState` が Task 3 で入っていること、`ThemeColor` が `ghbox_core::config` から import 済みであることを確認。

`column_label` に追加:

```rust
        Column::State => "",
```

`column_constraint` に追加:

```rust
        Column::State => Constraint::Length(2),
```

`cell_style` の下に追加:

```rust
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
```

`cell_text` のシグネチャを `fn cell_text(item: &Item, col: Column, now_epoch: i64, icons: bool) -> String` に変更し、match に追加:

```rust
        Column::State => state_icon(item.state, icons).to_string(),
```

`cell_style` のシグネチャを `fn cell_style(col: Column, theme: &Theme, state: PrState) -> Style` に変更し、match に追加:

```rust
        Column::State => Style::default().fg(color(state_color(state, theme))),
```

`draw_table` の rows 構築を差し替え(State セルは選択行でも状態色を保つ — 色が情報そのものなので selection_fg で潰さない。既存コメントの直後):

```rust
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
```

- [ ] **Step 7: テスト・lint・fmt**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all`
Expected: 全パス

- [ ] **Step 8: 実表示でグリフを確認**

Run: `printf 'open   draft   merged   closed \n'`
Expected: 4つのアイコンがターミナルに表示される(豆腐にならない)

- [ ] **Step 9: コミット**

```bash
git add crates/ghbox-core/src/config.rs crates/ghbox/src/ui.rs
git commit -m "feat(tui): pr state icon column with icons config flag"
```

---

### Task 5: タブ下線・件数 accent・空表示 + ドキュメント

**Files:**
- Modify: `crates/ghbox/Cargo.toml`(unicode-width を dev-dependencies から dependencies へ)
- Modify: `crates/ghbox/src/ui.rs`(active_tab_range / draw_rule / draw_tabs / 空表示、テスト)
- Modify: `README.md`, `CLAUDE.md`

**Interfaces:**
- Consumes: 既存の `draw_tabs` / `draw_rule`、Task 2 の `theme.faint`
- Produces: なし(最終タスク)

- [ ] **Step 1: 依存を移動**

`crates/ghbox/Cargo.toml`: `[dev-dependencies]` の `unicode-width = "0.2"` を `[dependencies]` に移動(セクションが空になったら `[dev-dependencies]` ごと削除)。

- [ ] **Step 2: 失敗するテストを書く**

`crates/ghbox/src/ui.rs` の `mod tests` に追加:

```rust
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
```

既存テストの更新:

- `empty_section_shows_no_items_placeholder`: `text.contains("no items")` を `text.contains("All clear — no items")` に
- `renders_tabs_table_and_status` の `─────` アサーションはそのまま維持できることを確認(下線はアクティブタブ幅のみで、罫線の残りは `─`)

- [ ] **Step 3: テストが失敗することを確認**

Run: `cargo test -p ghbox tab`
Expected: コンパイルエラー(`active_tab_range` 未定義)

- [ ] **Step 4: 実装**

`crates/ghbox/src/ui.rs` 先頭に追加:

```rust
use unicode_width::UnicodeWidthStr;
```

(tests モジュール内の `use unicode_width::UnicodeWidthStr;` は重複になるので削除)

`draw_tabs` の下に追加:

```rust
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
```

`draw_rule` を差し替え(シグネチャ変更):

```rust
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
        Paragraph::new("━".repeat(w as usize))
            .style(Style::default().fg(color(theme.tab_active))),
        underline,
    );
}
```

`draw` 内の呼び出しを `draw_rule(frame, app, &config.theme, chunks[1]);` に変更。

`draw_tabs` の count span を差し替え(アクティブタブの件数は accent):

```rust
        let count = format!(" {}", s.items.len());
        let count_style = if i == app.active {
            Style::default().fg(color(theme.tab_active))
        } else {
            dim
        };
        spans.push(Span::styled(count, count_style));
```

`draw_table` の空表示を差し替え:

```rust
        frame.render_widget(
            Paragraph::new("All clear — no items")
                .style(Style::default().fg(color(theme.faint)))
                .alignment(Alignment::Center),
            centered,
        );
```

- [ ] **Step 5: テスト・lint・fmt**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all`
Expected: 全パス

- [ ] **Step 6: ドキュメント更新**

`README.md` の `[theme]` ブロックを差し替え:

```toml
icons = true                        # false で Nerd Font アイコンの代わりに ● を表示

[theme]                             # 省略キーはデフォルト(catppuccin mocha)。ratatui 名前付き色(小文字) or "#rrggbb"
tab_active = "#cba6f7"              # アクティブタブ・タブ下線・選択マーカー・スピナーの accent 色
selection_bg = "#313244"
pr_number = "#89b4fa"               # PR番号カラム。ほかに author / time / faint カラム色も指定可
state_open = "#a6e3a1"              # state カラムのアイコン色: state_draft / state_merged / state_closed も指定可
```

columns の説明行(セクション2のコメント)を更新: `# filter 省略 = 検索結果そのまま。columns 省略 = ["state", "repo", "number", "title", "author", "updated"]`

`CLAUDE.md` のセクション説明のカラム列挙を更新: `- カラム: \`state\` / \`repo\` / \`number\` / \`title\` / \`author\` / \`comment\` / \`updated\` / \`created\``

- [ ] **Step 7: コミット**

```bash
git add crates/ghbox/Cargo.toml crates/ghbox/src/ui.rs Cargo.lock README.md CLAUDE.md
git commit -m "feat(tui): active tab underline, accent counts, refined empty state"
```

---

## 完了後

- `cargo run -p ghbox` で実 TUI を起動し、スピナー回転(r キー)・アイコン表示・タブ下線・配色を目視確認する(過去2回のリスタイルでスモークテスト未実施が続いているため今回は必ずやる)
- superpowers:finishing-a-development-branch でマージ/PR 判断
