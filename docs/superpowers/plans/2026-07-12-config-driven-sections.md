# Config 駆動セクション + gh-dash 風 UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ghbox を「固定2セクション」から「config.toml でセクション・カラム・テーマ・キーバインドを自由定義できる PR inbox TUI」に拡張する。

**Architecture:** ghbox-core に統一 `Item` モデルと動的 GraphQL クエリビルダを導入し、セクションごとにフィルタ(none / comment-mention / command)を適用する。既読は comment ID 単位(コメントアイテム)と PR+updatedAt 単位(PRアイテム、更新で再浮上)に一般化。TUI はタブバー + Table + ステータスバーに書き換える。新旧 API を並存させながらタスクを進め、フロントエンド切替後に旧 API を削除する。

**Tech Stack:** Rust (edition 2024), ratatui 0.30 + crossterm, tokio, reqwest + GitHub GraphQL v4, rusqlite, serde/toml

**Spec:** `docs/superpowers/specs/2026-07-12-config-driven-sections-design.md`

## Global Constraints

- `cargo test --workspace` / `cargo clippy --workspace -- -D warnings` / `cargo fmt --all` が各タスク終了時にクリーンであること
- コミットメッセージは英語、conventional commits。末尾に `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- ghbox-core は端末描画を持たない(theme/keybindings の**型と検証**は core、**描画への適用**は frontend)
- config は `deny_unknown_fields` 維持(typo 検出)。トップレベル `extra_patterns` は廃止(破壊的変更、未リリースのため許容)
- SQLite MIGRATIONS は **append-only**。既存エントリは絶対に編集しない(NAS 共有 DB)
- エラー: anyhow(バイナリ) / thiserror(lib)
- config エラーは起動時に検出して即エラー終了(該当箇所をメッセージに含める)
- 各タスク終了時にワークスペース全体がコンパイル・テストパスすること(新旧 API 並存戦略)

---

### Task 1: `item.rs` — 統一 Item モデル

**Files:**
- Create: `crates/ghbox-core/src/item.rs`
- Modify: `crates/ghbox-core/src/lib.rs`

**Interfaces:**
- Produces: `Item`(全フィールド pub、`Serialize`)、`CommentInfo`、`Item::stable_id() -> String`、`Item::pr_key() -> String`、`Item::sort_time() -> &str`、`Item::display_author() -> &str`

- [ ] **Step 1: `crates/ghbox-core/src/item.rs` を作成(テスト込み)**

```rust
use serde::Serialize;

/// A single row in a section: a PR (`comment == None`) or a specific comment
/// on a PR (`comment == Some`, produced by the comment-mention filter).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    /// Repository nameWithOwner, e.g. "nogu3/hestia".
    pub repo: String,
    pub pr_number: u64,
    pub pr_title: String,
    pub pr_url: String,
    /// PR author login.
    pub pr_author: String,
    /// ISO8601. Lexicographic order == chronological order.
    pub pr_updated_at: String,
    pub pr_created_at: String,
    /// Some only for items produced by the comment-mention filter.
    pub comment: Option<CommentInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommentInfo {
    /// GitHub comment databaseId. Read-state key for comment items.
    pub id: i64,
    /// Comment author login.
    pub author: String,
    pub body: String,
    pub created_at: String,
}

impl Item {
    /// Stable identity shared by the command-filter protocol and read-state:
    /// `comment:<databaseId>` or `pr:<repo>#<number>`.
    pub fn stable_id(&self) -> String {
        match &self.comment {
            Some(c) => format!("comment:{}", c.id),
            None => format!("pr:{}", self.pr_key()),
        }
    }

    /// Read-state key for PR items: `repo#number`.
    pub fn pr_key(&self) -> String {
        format!("{}#{}", self.repo, self.pr_number)
    }

    /// Sort timestamp: comment items by comment creation, PR items by last
    /// update.
    pub fn sort_time(&self) -> &str {
        match &self.comment {
            Some(c) => &c.created_at,
            None => &self.pr_updated_at,
        }
    }

    /// Author for the `author` column: comment author for comment items,
    /// PR author otherwise.
    pub fn display_author(&self) -> &str {
        match &self.comment {
            Some(c) => &c.author,
            None => &self.pr_author,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr_item() -> Item {
        Item {
            repo: "nogu3/hestia".into(),
            pr_number: 9,
            pr_title: "t".into(),
            pr_url: "u".into(),
            pr_author: "alice".into(),
            pr_updated_at: "2026-07-02T00:00:00Z".into(),
            pr_created_at: "2026-07-01T00:00:00Z".into(),
            comment: None,
        }
    }

    fn comment_item() -> Item {
        Item {
            comment: Some(CommentInfo {
                id: 42,
                author: "bob".into(),
                body: "@nogu3 merge please".into(),
                created_at: "2026-07-03T00:00:00Z".into(),
            }),
            ..pr_item()
        }
    }

    #[test]
    fn pr_item_stable_id_and_key() {
        assert_eq!(pr_item().stable_id(), "pr:nogu3/hestia#9");
        assert_eq!(pr_item().pr_key(), "nogu3/hestia#9");
    }

    #[test]
    fn comment_item_stable_id_uses_comment_id() {
        assert_eq!(comment_item().stable_id(), "comment:42");
    }

    #[test]
    fn sort_time_follows_item_kind() {
        assert_eq!(pr_item().sort_time(), "2026-07-02T00:00:00Z");
        assert_eq!(comment_item().sort_time(), "2026-07-03T00:00:00Z");
    }

    #[test]
    fn display_author_follows_item_kind() {
        assert_eq!(pr_item().display_author(), "alice");
        assert_eq!(comment_item().display_author(), "bob");
    }

    #[test]
    fn serializes_all_fields() {
        let json = serde_json::to_value(comment_item()).unwrap();
        assert_eq!(json["repo"], "nogu3/hestia");
        assert_eq!(json["pr_number"], 9);
        assert_eq!(json["comment"]["id"], 42);
        assert_eq!(serde_json::to_value(pr_item()).unwrap()["comment"], serde_json::Value::Null);
    }
}
```

- [ ] **Step 2: `lib.rs` にモジュール追加**

`crates/ghbox-core/src/lib.rs` の `pub mod inbox;` の下に:

```rust
pub mod item;
```

- [ ] **Step 3: テスト実行**

Run: `cargo test -p ghbox-core item::`
Expected: PASS (5 tests)

- [ ] **Step 4: fmt + clippy**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 5: Commit**

```bash
git add crates/ghbox-core/src/item.rs crates/ghbox-core/src/lib.rs
git commit -m "feat(core): add unified Item model with stable ids"
```

---

### Task 2: config — Section / SectionFilter / Column

**Files:**
- Modify: `crates/ghbox-core/src/config.rs`(大部分を書き換え)
- Modify: `crates/ghbox/src/main.rs:86`(`&config.extra_patterns` → `&[]` の一時パッチ。Task 8 で main は全面書き換えされる)

**Interfaces:**
- Consumes: なし
- Produces: `Config { poll_interval_secs: u64, db_path: PathBuf, sections: Vec<Section> }`、`Section { title: String, query: String, columns: Vec<Column>, filter: SectionFilter }`(すべて pub、`Clone`)、`Column`(enum: `Repo, Number, Title, Author, Comment, Updated, Created`、`Copy`)、`SectionFilter`(enum: `None` / `CommentMention { extra_patterns: Vec<String> }` / `Command { command: String }`)、`Config::load()` / `Config::load_from(&Path)`(パース後 `validate()` 実行)

- [ ] **Step 1: config.rs を書き換え(失敗するテストを含む)**

`crates/ghbox-core/src/config.rs` 全体を以下に置き換える(theme / keybindings は Task 3 で追加):

```rust
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Error, Result};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub poll_interval_secs: u64,
    pub db_path: PathBuf,
    pub sections: Vec<Section>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            poll_interval_secs: 300,
            db_path: default_db_path(),
            sections: default_sections(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub title: String,
    pub query: String,
    #[serde(default = "default_columns")]
    pub columns: Vec<Column>,
    #[serde(default)]
    pub filter: SectionFilter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Column {
    Repo,
    Number,
    Title,
    Author,
    Comment,
    Updated,
    Created,
}

/// Per-section filter. Deserialized via `FilterSpec` so unknown/misplaced
/// keys inside `filter = { ... }` are config errors (serde's internally
/// tagged enums do not support deny_unknown_fields).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(try_from = "FilterSpec")]
pub enum SectionFilter {
    #[default]
    None,
    CommentMention {
        extra_patterns: Vec<String>,
    },
    Command {
        command: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilterSpec {
    #[serde(rename = "type")]
    kind: String,
    extra_patterns: Option<Vec<String>>,
    command: Option<String>,
}

impl TryFrom<FilterSpec> for SectionFilter {
    type Error = String;

    fn try_from(spec: FilterSpec) -> std::result::Result<Self, String> {
        match spec.kind.as_str() {
            "none" => {
                if spec.extra_patterns.is_some() || spec.command.is_some() {
                    return Err("filter type \"none\" takes no other keys".into());
                }
                Ok(SectionFilter::None)
            }
            "comment-mention" => {
                if spec.command.is_some() {
                    return Err("filter type \"comment-mention\" does not take `command`".into());
                }
                Ok(SectionFilter::CommentMention {
                    extra_patterns: spec.extra_patterns.unwrap_or_default(),
                })
            }
            "command" => {
                if spec.extra_patterns.is_some() {
                    return Err(
                        "filter type \"command\" does not take `extra_patterns`".into()
                    );
                }
                let command = spec
                    .command
                    .ok_or_else(|| "filter type \"command\" requires `command`".to_string())?;
                Ok(SectionFilter::Command { command })
            }
            other => Err(format!(
                "unknown filter type {other:?} (expected \"none\", \"comment-mention\", or \"command\")"
            )),
        }
    }
}

/// Built-in sections used when the config file has no `sections` key.
/// Reproduces the pre-config behavior (merge requests + review requests).
fn default_sections() -> Vec<Section> {
    vec![
        Section {
            title: "マージ依頼".into(),
            query: "is:pr is:open mentions:@me".into(),
            columns: vec![
                Column::Repo,
                Column::Number,
                Column::Title,
                Column::Author,
                Column::Comment,
            ],
            filter: SectionFilter::CommentMention {
                extra_patterns: Vec::new(),
            },
        },
        Section {
            title: "レビュー依頼".into(),
            query: "is:pr is:open review-requested:@me".into(),
            columns: default_columns(),
            filter: SectionFilter::None,
        },
    ]
}

fn default_columns() -> Vec<Column> {
    vec![
        Column::Repo,
        Column::Number,
        Column::Title,
        Column::Author,
        Column::Updated,
    ]
}

fn default_db_path() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_default()
                .join(".local")
                .join("share")
        })
        .join("ghbox")
        .join("state.db")
}

fn config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("ghbox")
        .join("config.toml")
}

impl Config {
    pub fn load() -> Result<Self> {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let config = if path.exists() {
            let text = std::fs::read_to_string(path)?;
            toml::from_str(&text)
                .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?
        } else {
            Self::default()
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.sections.is_empty() {
            return Err(Error::Config(
                "sections must not be empty (omit the key entirely for the defaults)".into(),
            ));
        }
        for section in &self.sections {
            if let SectionFilter::CommentMention { extra_patterns } = &section.filter {
                for pattern in extra_patterns {
                    regex::Regex::new(pattern).map_err(|e| {
                        Error::Config(format!(
                            "section {:?}: invalid extra pattern {pattern:?}: {e}",
                            section.title
                        ))
                    })?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_text: &str) -> Result<Config> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, toml_text).unwrap();
        Config::load_from(&path)
    }

    #[test]
    fn missing_file_yields_default_two_sections() {
        let cfg = Config::load_from(Path::new("/nonexistent/config.toml")).unwrap();
        assert_eq!(cfg.poll_interval_secs, 300);
        assert!(cfg.db_path.ends_with("ghbox/state.db"));
        assert_eq!(cfg.sections.len(), 2);
        assert_eq!(cfg.sections[0].title, "マージ依頼");
        assert!(matches!(
            cfg.sections[0].filter,
            SectionFilter::CommentMention { .. }
        ));
        assert_eq!(cfg.sections[1].filter, SectionFilter::None);
    }

    #[test]
    fn sections_key_omitted_uses_defaults() {
        let cfg = parse("poll_interval_secs = 60\n").unwrap();
        assert_eq!(cfg.poll_interval_secs, 60);
        assert_eq!(cfg.sections.len(), 2);
    }

    #[test]
    fn explicit_empty_sections_is_error() {
        assert!(parse("sections = []\n").is_err());
    }

    #[test]
    fn full_section_parses() {
        let cfg = parse(
            r#"
[[sections]]
title = "マージ依頼"
query = "is:pr is:open mentions:@me"
columns = ["repo", "number", "title", "author", "comment"]
filter = { type = "comment-mention", extra_patterns = ["(?i)ship\\s*it"] }

[[sections]]
title = "自分が関わるPR"
query = "is:pr is:open involves:@me"
filter = { type = "command", command = "jq -r .id" }
"#,
        )
        .unwrap();
        assert_eq!(cfg.sections.len(), 2);
        assert_eq!(cfg.sections[0].columns[4], Column::Comment);
        assert_eq!(
            cfg.sections[0].filter,
            SectionFilter::CommentMention {
                extra_patterns: vec!["(?i)ship\\s*it".into()]
            }
        );
        assert_eq!(
            cfg.sections[1].filter,
            SectionFilter::Command {
                command: "jq -r .id".into()
            }
        );
        // columns omitted → defaults
        assert_eq!(cfg.sections[1].columns, default_columns());
    }

    #[test]
    fn filter_omitted_is_none() {
        let cfg = parse("[[sections]]\ntitle = \"t\"\nquery = \"q\"\n").unwrap();
        assert_eq!(cfg.sections[0].filter, SectionFilter::None);
    }

    #[test]
    fn unknown_column_is_error() {
        let err = parse("[[sections]]\ntitle = \"t\"\nquery = \"q\"\ncolumns = [\"bogus\"]\n");
        assert!(err.is_err());
    }

    #[test]
    fn unknown_filter_type_is_error() {
        let err = parse(
            "[[sections]]\ntitle = \"t\"\nquery = \"q\"\nfilter = { type = \"bogus\" }\n",
        );
        assert!(err.is_err());
    }

    #[test]
    fn misplaced_filter_key_is_error() {
        // command key on a comment-mention filter must be rejected
        let err = parse(
            "[[sections]]\ntitle = \"t\"\nquery = \"q\"\nfilter = { type = \"comment-mention\", command = \"x\" }\n",
        );
        assert!(err.is_err());
    }

    #[test]
    fn command_filter_requires_command() {
        let err =
            parse("[[sections]]\ntitle = \"t\"\nquery = \"q\"\nfilter = { type = \"command\" }\n");
        assert!(err.is_err());
    }

    #[test]
    fn invalid_extra_pattern_is_error() {
        let err = parse(
            "[[sections]]\ntitle = \"t\"\nquery = \"q\"\nfilter = { type = \"comment-mention\", extra_patterns = [\"(\"] }\n",
        );
        assert!(err.is_err());
    }

    #[test]
    fn removed_top_level_extra_patterns_is_error() {
        // the pre-rewrite top-level key must now be a typo error
        assert!(parse("extra_patterns = [\"ship it\"]\n").is_err());
    }

    #[test]
    fn unknown_key_is_error() {
        assert!(parse("typo_key = 1\n").is_err());
    }
}
```

- [ ] **Step 2: main.rs の一時パッチ**

`crates/ghbox/src/main.rs` の

```rust
                    match CommentFilter::new(&parsed.viewer_login, &config.extra_patterns) {
```

を

```rust
                    match CommentFilter::new(&parsed.viewer_login, &[]) {
```

に変更する(Task 8 で main.rs は全面書き換えされるまでの一時措置。デフォルト config の extra_patterns は元々空なので挙動は変わらない)。

- [ ] **Step 3: テスト実行**

Run: `cargo test --workspace`
Expected: PASS(config の新テスト 12 件を含む)

- [ ] **Step 4: fmt + clippy**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 5: Commit**

```bash
git add crates/ghbox-core/src/config.rs crates/ghbox/src/main.rs
git commit -m "feat(core): config-driven sections with per-section filters"
```

---

### Task 3: config — Theme / Keybindings / 重複検出

**Files:**
- Modify: `crates/ghbox-core/src/config.rs`

**Interfaces:**
- Consumes: Task 2 の `Config`
- Produces: `Config` に `pub theme: Theme` と `pub keybindings: Keybindings` を追加。`Theme`(7 フィールド、各 `ThemeColor`)、`ThemeColor`(enum: `Named(NamedColor)` / `Rgb(u8,u8,u8)`、`Copy`)、`NamedColor`(16 色 enum、`Copy`)、`Keybindings`(8 フィールド、各 `KeySpec`)、`KeySpec`(enum: `Char(char), Tab, BackTab, Enter, Up, Down, Esc`、`Copy`、`Display` 実装)

- [ ] **Step 1: config.rs に Theme / Keybindings を追加**

`Config` 構造体にフィールドを追加:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub poll_interval_secs: u64,
    pub db_path: PathBuf,
    pub sections: Vec<Section>,
    pub theme: Theme,
    pub keybindings: Keybindings,
}
```

`Default for Config` にも追加:

```rust
            theme: Theme::default(),
            keybindings: Keybindings::default(),
```

以下の型を config.rs に追加(`SectionFilter` 定義の後あたり):

```rust
/// ratatui named color (lowercase in config) or "#rrggbb".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeColor {
    Named(NamedColor),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
}

impl std::str::FromStr for ThemeColor {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        if let Some(hex) = s.strip_prefix('#') {
            if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                let r = u8::from_str_radix(&hex[0..2], 16).expect("checked hex");
                let g = u8::from_str_radix(&hex[2..4], 16).expect("checked hex");
                let b = u8::from_str_radix(&hex[4..6], 16).expect("checked hex");
                return Ok(ThemeColor::Rgb(r, g, b));
            }
            return Err(format!("invalid color {s:?}: expected #rrggbb"));
        }
        use NamedColor::*;
        let named = match s {
            "black" => Black,
            "red" => Red,
            "green" => Green,
            "yellow" => Yellow,
            "blue" => Blue,
            "magenta" => Magenta,
            "cyan" => Cyan,
            "gray" => Gray,
            "darkgray" => DarkGray,
            "lightred" => LightRed,
            "lightgreen" => LightGreen,
            "lightyellow" => LightYellow,
            "lightblue" => LightBlue,
            "lightmagenta" => LightMagenta,
            "lightcyan" => LightCyan,
            "white" => White,
            _ => return Err(format!("unknown color {s:?}")),
        };
        Ok(ThemeColor::Named(named))
    }
}

impl<'de> serde::Deserialize<'de> for ThemeColor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    pub tab_active: ThemeColor,
    pub tab_inactive: ThemeColor,
    pub border: ThemeColor,
    pub selection_bg: ThemeColor,
    pub selection_fg: ThemeColor,
    pub table_header: ThemeColor,
    pub status_bar: ThemeColor,
}

impl Default for Theme {
    fn default() -> Self {
        use NamedColor::*;
        Self {
            tab_active: ThemeColor::Named(Yellow),
            tab_inactive: ThemeColor::Named(DarkGray),
            border: ThemeColor::Named(DarkGray),
            selection_bg: ThemeColor::Named(Blue),
            selection_fg: ThemeColor::Named(White),
            table_header: ThemeColor::Named(Cyan),
            status_bar: ThemeColor::Named(DarkGray),
        }
    }
}

/// A key a config action can be bound to: a single character or a named
/// special key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySpec {
    Char(char),
    Tab,
    BackTab,
    Enter,
    Up,
    Down,
    Esc,
}

impl std::str::FromStr for KeySpec {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "tab" => Ok(KeySpec::Tab),
            "backtab" => Ok(KeySpec::BackTab),
            "enter" => Ok(KeySpec::Enter),
            "up" => Ok(KeySpec::Up),
            "down" => Ok(KeySpec::Down),
            "esc" => Ok(KeySpec::Esc),
            _ => {
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Ok(KeySpec::Char(c)),
                    _ => Err(format!(
                        "invalid key {s:?}: expected one character or tab/backtab/enter/up/down/esc"
                    )),
                }
            }
        }
    }
}

impl std::fmt::Display for KeySpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeySpec::Char(c) => write!(f, "{c}"),
            KeySpec::Tab => write!(f, "tab"),
            KeySpec::BackTab => write!(f, "backtab"),
            KeySpec::Enter => write!(f, "enter"),
            KeySpec::Up => write!(f, "up"),
            KeySpec::Down => write!(f, "down"),
            KeySpec::Esc => write!(f, "esc"),
        }
    }
}

impl<'de> serde::Deserialize<'de> for KeySpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Keybindings {
    pub up: KeySpec,
    pub down: KeySpec,
    pub next_section: KeySpec,
    pub prev_section: KeySpec,
    pub open: KeySpec,
    pub done: KeySpec,
    pub refresh: KeySpec,
    pub quit: KeySpec,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            up: KeySpec::Char('k'),
            down: KeySpec::Char('j'),
            next_section: KeySpec::Tab,
            prev_section: KeySpec::BackTab,
            open: KeySpec::Enter,
            done: KeySpec::Char('d'),
            refresh: KeySpec::Char('r'),
            quit: KeySpec::Char('q'),
        }
    }
}
```

`Config::validate()` の末尾(`Ok(())` の前)にキー重複検出を追加:

```rust
        let kb = &self.keybindings;
        let bindings = [
            ("up", kb.up),
            ("down", kb.down),
            ("next_section", kb.next_section),
            ("prev_section", kb.prev_section),
            ("open", kb.open),
            ("done", kb.done),
            ("refresh", kb.refresh),
            ("quit", kb.quit),
        ];
        for (i, (name_a, key_a)) in bindings.iter().enumerate() {
            for (name_b, key_b) in &bindings[i + 1..] {
                if key_a == key_b {
                    return Err(Error::Config(format!(
                        "keybindings: {name_a} and {name_b} are both bound to \"{key_a}\""
                    )));
                }
            }
        }
```

- [ ] **Step 2: テスト追加(config.rs の tests モジュール内)**

```rust
    #[test]
    fn theme_defaults_and_partial_override() {
        let cfg = parse("[theme]\ntab_active = \"red\"\n").unwrap();
        assert_eq!(cfg.theme.tab_active, ThemeColor::Named(NamedColor::Red));
        // omitted keys keep defaults
        assert_eq!(cfg.theme.selection_bg, ThemeColor::Named(NamedColor::Blue));
    }

    #[test]
    fn hex_color_parses() {
        let cfg = parse("[theme]\nborder = \"#1a2B3c\"\n").unwrap();
        assert_eq!(cfg.theme.border, ThemeColor::Rgb(0x1a, 0x2b, 0x3c));
    }

    #[test]
    fn invalid_color_is_error() {
        assert!(parse("[theme]\nborder = \"mauve\"\n").is_err());
        assert!(parse("[theme]\nborder = \"#12345\"\n").is_err());
    }

    #[test]
    fn keybindings_defaults_and_partial_override() {
        let cfg = parse("[keybindings]\nquit = \"x\"\n").unwrap();
        assert_eq!(cfg.keybindings.quit, KeySpec::Char('x'));
        assert_eq!(cfg.keybindings.next_section, KeySpec::Tab);
    }

    #[test]
    fn special_key_names_parse() {
        let cfg = parse("[keybindings]\nquit = \"esc\"\nopen = \"o\"\n").unwrap();
        assert_eq!(cfg.keybindings.quit, KeySpec::Esc);
        assert_eq!(cfg.keybindings.open, KeySpec::Char('o'));
    }

    #[test]
    fn invalid_key_name_is_error() {
        assert!(parse("[keybindings]\nquit = \"ctrl-q\"\n").is_err());
    }

    #[test]
    fn duplicate_keybinding_is_error() {
        let err = parse("[keybindings]\nquit = \"j\"\n").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("down") && msg.contains("quit"), "got: {msg}");
    }

    #[test]
    fn key_spec_display_roundtrip() {
        assert_eq!(KeySpec::Char('k').to_string(), "k");
        assert_eq!(KeySpec::BackTab.to_string(), "backtab");
    }
```

- [ ] **Step 3: テスト実行**

Run: `cargo test -p ghbox-core config::`
Expected: PASS(全 config テスト)

- [ ] **Step 4: fmt + clippy**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 5: Commit**

```bash
git add crates/ghbox-core/src/config.rs
git commit -m "feat(core): theme and keybinding config with validation"
```

---

### Task 4: store — v2 マイグレーション / pr kind / version ガード

**Files:**
- Modify: `crates/ghbox-core/src/error.rs`
- Modify: `crates/ghbox-core/src/store.rs`

**Interfaces:**
- Consumes: なし
- Produces: `Error::Schema(String)` variant、`KIND_PR: &str = "pr"`、`Store::mark_done_pr(&self, key: &str, updated_at: &str) -> Result<()>`(upsert)、`Store::is_done_pr(&self, key: &str, updated_at: &str) -> Result<bool>`(記録 updated_at >= アイテム updatedAt なら done)。既存 `mark_done` / `is_done` / `KIND_MERGE_COMMENT` は維持

- [ ] **Step 1: error.rs に variant 追加**

`crates/ghbox-core/src/error.rs` の `Config` variant の後に:

```rust
    #[error("db schema error: {0}")]
    Schema(String),
```

- [ ] **Step 2: store.rs に失敗するテストを追加**

`crates/ghbox-core/src/store.rs` の tests モジュールに追加:

```rust
    #[test]
    fn v1_db_migrates_review_requests_to_pr_kind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        {
            // Build a real v1 DB by hand.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            conn.execute(
                "INSERT INTO done_items (kind, key, done_at) VALUES ('review_request', 'o/r#1', '2026-01-02 03:04:05')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO done_items (kind, key) VALUES ('merge_comment', '42')",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        // review_request copied to pr kind; done_at converted to ISO8601 T/Z
        assert!(store.is_done_pr("o/r#1", "2026-01-02T03:04:05Z").unwrap());
        // PR updated after the mark → resurfaces
        assert!(!store.is_done_pr("o/r#1", "2026-01-02T03:04:06Z").unwrap());
        // merge_comment rows untouched
        assert!(store.is_done(KIND_MERGE_COMMENT, "42").unwrap());
        // old rows kept so old binaries sharing the NAS DB still work
        drop(store);
        let conn = Connection::open(&path).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM done_items WHERE kind = 'review_request'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn pr_mark_done_upserts_and_resurfaces_on_update() {
        let store = Store::open_in_memory().unwrap();
        assert!(!store.is_done_pr("o/r#1", "2026-01-01T00:00:00Z").unwrap());
        store.mark_done_pr("o/r#1", "2026-01-01T00:00:00Z").unwrap();
        assert!(store.is_done_pr("o/r#1", "2026-01-01T00:00:00Z").unwrap());
        // PR updated later → resurfaces
        assert!(!store.is_done_pr("o/r#1", "2026-02-01T00:00:00Z").unwrap());
        // `d` again with the new updatedAt → done again (upsert, no constraint error)
        store.mark_done_pr("o/r#1", "2026-02-01T00:00:00Z").unwrap();
        assert!(store.is_done_pr("o/r#1", "2026-02-01T00:00:00Z").unwrap());
    }

    #[test]
    fn newer_db_version_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99).unwrap();
        }
        assert!(matches!(
            Store::open(&path),
            Err(crate::Error::Schema(_))
        ));
    }
```

- [ ] **Step 3: テスト実行(失敗確認)**

Run: `cargo test -p ghbox-core store::`
Expected: FAIL — `is_done_pr` / `mark_done_pr` / `KIND_PR` 未定義のコンパイルエラー

- [ ] **Step 4: store.rs 実装**

定数追加と MIGRATIONS 追記(**v1 エントリは一切変更しない**):

```rust
pub const KIND_MERGE_COMMENT: &str = "merge_comment";
pub const KIND_REVIEW_REQUEST: &str = "review_request";
pub const KIND_PR: &str = "pr";

/// Append-only migration list. NEVER edit an existing entry — the DB lives
/// on a NAS shared across machines; only append new statements.
const MIGRATIONS: &[&str] = &[
    "CREATE TABLE done_items (
        kind TEXT NOT NULL,
        key  TEXT NOT NULL,
        done_at TEXT NOT NULL DEFAULT (datetime('now')),
        PRIMARY KEY (kind, key)
    )",
    // v2: PR items are done per (key, updatedAt) — resurface when the PR is
    // updated after the mark. Copy legacy review_request rows to the new
    // 'pr' kind (done_at reformatted to ISO8601 T/Z for lexicographic
    // comparison with GitHub's updatedAt); keep the old rows so old
    // binaries sharing the NAS DB keep working.
    "ALTER TABLE done_items ADD COLUMN updated_at TEXT;
     INSERT OR IGNORE INTO done_items (kind, key, done_at, updated_at)
       SELECT 'pr', key, done_at, strftime('%Y-%m-%dT%H:%M:%SZ', done_at)
       FROM done_items WHERE kind = 'review_request';",
];
```

`migrate()` の先頭(version 取得直後)にガード追加:

```rust
        if version as usize > MIGRATIONS.len() {
            return Err(crate::Error::Schema(format!(
                "db schema version {version} is newer than this binary supports (max {}); update ghbox",
                MIGRATIONS.len()
            )));
        }
```

メソッド追加:

```rust
    /// Marks a PR item done as of `updated_at` (the item's own updatedAt,
    /// not the wall clock). Upsert: re-marking after a resurface refreshes
    /// the recorded updatedAt.
    pub fn mark_done_pr(&self, key: &str, updated_at: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO done_items (kind, key, done_at, updated_at)
             VALUES ('pr', ?1, datetime('now'), ?2)
             ON CONFLICT(kind, key) DO UPDATE SET
               done_at = excluded.done_at, updated_at = excluded.updated_at",
            (key, updated_at),
        )?;
        Ok(())
    }

    /// A PR item is done iff a mark exists whose recorded updatedAt is >=
    /// the item's current updatedAt (ISO8601 strings compare lexicographically).
    pub fn is_done_pr(&self, key: &str, updated_at: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM done_items
             WHERE kind = 'pr' AND key = ?1 AND updated_at >= ?2",
            (key, updated_at),
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
```

- [ ] **Step 5: テスト実行**

Run: `cargo test -p ghbox-core store::`
Expected: PASS(既存 4 + 新規 3)

- [ ] **Step 6: fmt + clippy + 全テスト**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace`
Expected: クリーン

- [ ] **Step 7: Commit**

```bash
git add crates/ghbox-core/src/error.rs crates/ghbox-core/src/store.rs
git commit -m "feat(core): v2 store migration with pr kind and version guard"
```

---

### Task 5: github — 動的クエリビルダ + 汎用パース

**Files:**
- Modify: `crates/ghbox-core/src/github.rs`(既存の `QUERY` / `Parsed` / `fetch` / `parse_response` は**残したまま**追加。削除は Task 9)

**Interfaces:**
- Consumes: `config::{Section, SectionFilter}`、`item::CommentInfo`
- Produces: `Fetched { viewer_login: String, sections: Vec<Vec<PrData>> }`(sections は引数の Section 列と同順)、`PrData { repo, pr_number: u64, pr_title, pr_url, pr_author, pr_updated_at, pr_created_at, comments: Vec<CommentInfo> }`(全 pub、`Clone`)、`build_query(&[Section]) -> (String, serde_json::Value)`、`fetch_sections(token: &str, sections: &[Section]) -> Result<Fetched>`(async)、`parse_sections(json: &str, section_count: usize) -> Result<Fetched>`

- [ ] **Step 1: 失敗するテストを github.rs の tests モジュールに追加**

```rust
    use crate::config::{Section, SectionFilter};

    fn section(query: &str, filter: SectionFilter) -> Section {
        Section {
            title: "t".into(),
            query: query.into(),
            columns: vec![],
            filter,
        }
    }

    #[test]
    fn build_query_aliases_sections_and_passes_variables() {
        let sections = vec![
            section(
                "is:pr mentions:@me",
                SectionFilter::CommentMention {
                    extra_patterns: vec![],
                },
            ),
            section("is:pr review-requested:@me", SectionFilter::None),
        ];
        let (query, vars) = build_query(&sections);
        assert!(query.contains("$q0: String!, $q1: String!"), "got: {query}");
        assert!(query.contains("s0: search(query: $q0, type: ISSUE, first: 50)"));
        assert!(query.contains("s1: search(query: $q1, type: ISSUE, first: 50)"));
        assert!(query.contains("viewer { login }"));
        // comments requested only for the comment-mention section
        assert_eq!(query.matches("comments(last: 50)").count(), 1);
        let comments_pos = query.find("comments(last: 50)").unwrap();
        assert!(query.find("s0:").unwrap() < comments_pos);
        assert!(comments_pos < query.find("s1:").unwrap());
        assert_eq!(vars["q0"], "is:pr mentions:@me");
        assert_eq!(vars["q1"], "is:pr review-requested:@me");
    }

    const SECTIONS_FIXTURE: &str = r#"{
      "data": {
        "viewer": { "login": "nogu3" },
        "s0": {
          "nodes": [
            {
              "number": 9,
              "title": "Implement Device List Management",
              "url": "https://github.com/nogu3/hestia/pull/9",
              "updatedAt": "2026-04-20T00:00:00Z",
              "createdAt": "2026-04-01T00:00:00Z",
              "author": { "login": "jules" },
              "repository": { "nameWithOwner": "nogu3/hestia" },
              "comments": {
                "nodes": [
                  {
                    "databaseId": 4275373830,
                    "author": { "login": "google-labs-jules" },
                    "body": "@nogu3 please merge this",
                    "createdAt": "2026-04-19T06:51:49Z"
                  },
                  {
                    "author": null,
                    "body": "no database id",
                    "createdAt": "2026-04-19T07:00:00Z"
                  }
                ]
              }
            },
            {}
          ]
        },
        "s1": {
          "nodes": [
            {
              "number": 12,
              "title": "Fix logger",
              "url": "https://github.com/nogu3/hestia/pull/12",
              "updatedAt": "2026-07-02T00:00:00Z",
              "createdAt": "2026-07-01T00:00:00Z",
              "author": null,
              "repository": { "nameWithOwner": "nogu3/hestia" }
            }
          ]
        }
      }
    }"#;

    #[test]
    fn parse_sections_returns_ordered_sections() {
        let fetched = parse_sections(SECTIONS_FIXTURE, 2).unwrap();
        assert_eq!(fetched.viewer_login, "nogu3");
        assert_eq!(fetched.sections.len(), 2);
        // empty {} node (non-PR search result) is skipped
        assert_eq!(fetched.sections[0].len(), 1);
        let pr = &fetched.sections[0][0];
        assert_eq!(pr.repo, "nogu3/hestia");
        assert_eq!(pr.pr_number, 9);
        assert_eq!(pr.pr_author, "jules");
        assert_eq!(pr.pr_updated_at, "2026-04-20T00:00:00Z");
        // comment without databaseId is skipped
        assert_eq!(pr.comments.len(), 1);
        assert_eq!(pr.comments[0].id, 4275373830);
        assert_eq!(pr.comments[0].author, "google-labs-jules");
        // section without comments in the query parses with empty comments
        let rr = &fetched.sections[1][0];
        assert_eq!(rr.pr_number, 12);
        assert_eq!(rr.pr_author, "(unknown)"); // ghost author
        assert!(rr.comments.is_empty());
    }

    #[test]
    fn parse_sections_null_data_with_errors_is_api_error() {
        let json = r#"{ "data": null, "errors": [ { "message": "rate limited" } ] }"#;
        let err = parse_sections(json, 1).unwrap_err();
        assert!(matches!(err, Error::Api(m) if m.contains("rate limited")));
    }

    #[test]
    fn parse_sections_partial_data_with_errors_still_parses() {
        let json = r#"{
          "data": { "viewer": { "login": "nogu3" }, "s0": { "nodes": [] } },
          "errors": [ { "message": "SAML enforcement" } ]
        }"#;
        let fetched = parse_sections(json, 1).unwrap();
        assert_eq!(fetched.viewer_login, "nogu3");
        assert!(fetched.sections[0].is_empty());
    }

    #[test]
    fn parse_sections_missing_or_null_alias_yields_empty_section() {
        // a section GitHub nulled out (partial failure) must not kill the fetch
        let json = r#"{
          "data": { "viewer": { "login": "nogu3" }, "s0": null }
        }"#;
        let fetched = parse_sections(json, 1).unwrap();
        assert!(fetched.sections[0].is_empty());
    }
```

- [ ] **Step 2: テスト実行(失敗確認)**

Run: `cargo test -p ghbox-core github::`
Expected: FAIL — `build_query` / `parse_sections` 未定義のコンパイルエラー

- [ ] **Step 3: github.rs に実装追加**

use 追加:

```rust
use std::collections::HashMap;

use crate::config::{Section, SectionFilter};
use crate::item::CommentInfo;
```

型と関数追加(既存コードはそのまま):

```rust
/// Result of one multi-section fetch. `sections` is parallel to the
/// `Section` slice passed to `build_query` / `fetch_sections`.
#[derive(Debug)]
pub struct Fetched {
    pub viewer_login: String,
    pub sections: Vec<Vec<PrData>>,
}

/// One PR as returned by search, before filtering.
#[derive(Debug, Clone)]
pub struct PrData {
    pub repo: String,
    pub pr_number: u64,
    pub pr_title: String,
    pub pr_url: String,
    pub pr_author: String,
    pub pr_updated_at: String,
    pub pr_created_at: String,
    /// Populated only for sections whose filter needs comment bodies.
    pub comments: Vec<CommentInfo>,
}

/// Builds one GraphQL request covering every section: `viewer` plus one
/// aliased `search` per section (s0, s1, ...). Search strings travel as
/// variables to avoid escaping issues. Comment bodies are requested only
/// for comment-mention sections. Verified 2026-07-11 with 2 searches:
/// cost = 1 rate-limit point per call.
pub fn build_query(sections: &[Section]) -> (String, serde_json::Value) {
    let mut query = String::from("query(");
    for i in 0..sections.len() {
        if i > 0 {
            query.push_str(", ");
        }
        query.push_str(&format!("$q{i}: String!"));
    }
    query.push_str(") {\n  viewer { login }\n");
    for (i, section) in sections.iter().enumerate() {
        let comments = if matches!(section.filter, SectionFilter::CommentMention { .. }) {
            "\n        comments(last: 50) { nodes { databaseId author { login } body createdAt } }"
        } else {
            ""
        };
        query.push_str(&format!(
            "  s{i}: search(query: $q{i}, type: ISSUE, first: 50) {{\n    nodes {{\n      ... on PullRequest {{\n        number\n        title\n        url\n        updatedAt\n        createdAt\n        author {{ login }}\n        repository {{ nameWithOwner }}{comments}\n      }}\n    }}\n  }}\n"
        ));
    }
    query.push('}');
    let variables: serde_json::Map<String, serde_json::Value> = sections
        .iter()
        .enumerate()
        .map(|(i, s)| (format!("q{i}"), serde_json::Value::String(s.query.clone())))
        .collect();
    (query, serde_json::Value::Object(variables))
}

#[derive(Deserialize)]
struct SectionsResponse {
    data: Option<SectionsData>,
    errors: Option<Vec<GqlError>>,
}

#[derive(Deserialize)]
struct SectionsData {
    viewer: Actor,
    #[serde(flatten)]
    searches: HashMap<String, Option<Search<PrNode>>>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct PrNode {
    number: u64,
    title: String,
    url: String,
    updated_at: String,
    created_at: String,
    author: Option<Actor>,
    repository: Option<Repo>,
    comments: CommentConnection,
}

pub async fn fetch_sections(token: &str, sections: &[Section]) -> Result<Fetched> {
    let (query, variables) = build_query(sections);
    let client = reqwest::Client::new();
    let response = client
        .post("https://api.github.com/graphql")
        .bearer_auth(token)
        .header("User-Agent", "ghbox")
        .json(&serde_json::json!({ "query": query, "variables": variables }))
        .send()
        .await?
        .error_for_status()?;
    let text = response.text().await?;
    parse_sections(&text, sections.len())
}

pub fn parse_sections(json: &str, section_count: usize) -> Result<Fetched> {
    let resp: SectionsResponse = serde_json::from_str(json)?;
    // GitHub may return HTTP 200 with both `errors` and usable `data` (e.g.
    // one SAML-protected org node is FORBIDDEN while the rest succeeds).
    // Prefer partial data; only treat `errors` as fatal without data.
    let mut data = match resp.data {
        Some(data) => data,
        None => {
            let messages: Vec<String> = resp
                .errors
                .unwrap_or_default()
                .into_iter()
                .map(|e| e.message)
                .collect();
            let message = if messages.is_empty() {
                "response has neither data nor errors".into()
            } else {
                messages.join("; ")
            };
            return Err(Error::Api(message));
        }
    };

    let mut sections = Vec::with_capacity(section_count);
    for i in 0..section_count {
        let search = data.searches.remove(&format!("s{i}")).flatten();
        let mut prs = Vec::new();
        for node in search.map(|s| s.nodes).unwrap_or_default().into_iter().flatten() {
            let Some(repo) = node.repository else { continue };
            let comments = node
                .comments
                .nodes
                .into_iter()
                .flatten()
                .filter_map(|c| {
                    let id = c.database_id?;
                    Some(CommentInfo {
                        id,
                        author: c.author.map(|a| a.login).unwrap_or_else(|| "(unknown)".into()),
                        body: c.body,
                        created_at: c.created_at,
                    })
                })
                .collect();
            prs.push(PrData {
                repo: repo.name_with_owner,
                pr_number: node.number,
                pr_title: node.title,
                pr_url: node.url,
                pr_author: node.author.map(|a| a.login).unwrap_or_else(|| "(unknown)".into()),
                pr_updated_at: node.updated_at,
                pr_created_at: node.created_at,
                comments,
            });
        }
        sections.push(prs);
    }

    Ok(Fetched {
        viewer_login: data.viewer.login,
        sections,
    })
}
```

- [ ] **Step 4: テスト実行**

Run: `cargo test -p ghbox-core github::`
Expected: PASS(既存 8 + 新規 5)

- [ ] **Step 5: fmt + clippy**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 6: Commit**

```bash
git add crates/ghbox-core/src/github.rs
git commit -m "feat(core): dynamic multi-section GraphQL query builder and parser"
```

---

### Task 6: filter — command フィルタ実行

**Files:**
- Modify: `crates/ghbox-core/Cargo.toml`
- Modify: `crates/ghbox-core/src/error.rs`
- Modify: `crates/ghbox-core/src/filter.rs`(既存 `CommentFilter` はそのまま)

**Interfaces:**
- Consumes: `item::Item`(`stable_id()`、`Serialize`)
- Produces: `Error::Filter(String)` variant、`run_command_filter(command: &str, items: &[Item]) -> Result<HashSet<String>>`(async。`sh -c` でバッチ実行、stdin に JSONL(`id` フィールド付き)、stdout から残す id を1行1個で受け取る。タイムアウト 10 秒、非ゼロ exit はエラー)

- [ ] **Step 1: Cargo.toml に tokio 追加**

`crates/ghbox-core/Cargo.toml` の `[dependencies]` に:

```toml
tokio = { version = "1.52.3", default-features = false, features = ["process", "time", "io-util"] }
```

`[dev-dependencies]` に:

```toml
tokio = { version = "1.52.3", features = ["rt-multi-thread", "macros"] }
```

- [ ] **Step 2: error.rs に variant 追加**

`Schema` variant の後に:

```rust
    #[error("{0}")]
    Filter(String),
```

- [ ] **Step 3: filter.rs に失敗するテストを追加**

tests モジュールに追加:

```rust
    use std::time::Duration;

    use crate::item::Item;

    fn pr_item(repo: &str, number: u64) -> Item {
        Item {
            repo: repo.into(),
            pr_number: number,
            pr_title: "t".into(),
            pr_url: "u".into(),
            pr_author: "a".into(),
            pr_updated_at: "2026-07-02T00:00:00Z".into(),
            pr_created_at: "2026-07-01T00:00:00Z".into(),
            comment: None,
        }
    }

    #[tokio::test]
    async fn command_filter_keeps_ids_printed_to_stdout() {
        let items = vec![pr_item("o/r", 1), pr_item("o/r", 2)];
        // stdin carries one JSON object per item with an "id" field;
        // grep -o extracts the first item's id from it
        let keep = run_command_filter("grep -o 'pr:o/r#1'", &items).await.unwrap();
        assert!(keep.contains("pr:o/r#1"));
        assert!(!keep.contains("pr:o/r#2"));
    }

    #[tokio::test]
    async fn command_filter_empty_stdout_keeps_nothing() {
        let keep = run_command_filter("cat > /dev/null", &[pr_item("o/r", 1)])
            .await
            .unwrap();
        assert!(keep.is_empty());
    }

    #[tokio::test]
    async fn command_filter_tolerates_child_not_reading_stdin() {
        // `head -n 1` exits after one line; the resulting broken pipe on the
        // writer side must not be an error
        let items = vec![pr_item("a/a", 1), pr_item("b/b", 2)];
        let keep = run_command_filter("head -n 1 | grep -o 'pr:a/a#1'", &items)
            .await
            .unwrap();
        assert!(keep.contains("pr:a/a#1"));
    }

    #[tokio::test]
    async fn command_filter_nonzero_exit_is_error() {
        let err = run_command_filter("exit 3", &[pr_item("o/r", 1)])
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Filter(m) if m.contains("exited")));
    }

    #[tokio::test]
    async fn command_filter_timeout_is_error() {
        let err = run_command_filter_with_timeout("sleep 5", &[], Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Filter(m) if m.contains("timed out")));
    }

    #[tokio::test]
    async fn command_filter_trims_and_skips_blank_lines() {
        let keep = run_command_filter("printf '  pr:o/r#1  \\n\\n'", &[pr_item("o/r", 1)])
            .await
            .unwrap();
        assert_eq!(keep.len(), 1);
        assert!(keep.contains("pr:o/r#1"));
    }
```

- [ ] **Step 4: テスト実行(失敗確認)**

Run: `cargo test -p ghbox-core filter::`
Expected: FAIL — `run_command_filter` 未定義のコンパイルエラー

- [ ] **Step 5: filter.rs に実装追加**

ファイル先頭の use を拡張:

```rust
use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;

use regex::Regex;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::item::Item;
use crate::{Error, Result};
```

`CommentFilter` の後に追加:

```rust
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// JSON line handed to a command filter: the item plus its stable `id`.
#[derive(Serialize)]
struct ItemLine<'a> {
    id: String,
    #[serde(flatten)]
    item: &'a Item,
}

/// Runs `sh -c <command>` once per poll (batch, not per item), feeding one
/// JSON object per item on stdin and reading the stable ids to keep from
/// stdout (one per line, plain text). Unknown ids are the caller's problem
/// (they simply match nothing). Non-zero exit and timeout are errors so the
/// caller can keep the section's previous items instead of showing an
/// empty (falsely "all clear") section.
pub async fn run_command_filter(command: &str, items: &[Item]) -> Result<HashSet<String>> {
    run_command_filter_with_timeout(command, items, COMMAND_TIMEOUT).await
}

async fn run_command_filter_with_timeout(
    command: &str,
    items: &[Item],
    timeout: Duration,
) -> Result<HashSet<String>> {
    let mut input = String::new();
    for item in items {
        input.push_str(&serde_json::to_string(&ItemLine {
            id: item.stable_id(),
            item,
        })?);
        input.push('\n');
    }

    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    let mut stdin = child.stdin.take().expect("stdin is piped");
    let output = tokio::time::timeout(timeout, async {
        // The child may exit without reading all input (e.g. `head`);
        // a broken pipe here is not an error.
        let _ = stdin.write_all(input.as_bytes()).await;
        drop(stdin);
        child.wait_with_output().await
    })
    .await
    .map_err(|_| {
        // kill_on_drop reaps the child when the timed-out future is dropped
        Error::Filter(format!(
            "command filter timed out after {}s: {command}",
            timeout.as_secs_f64()
        ))
    })??;

    if !output.status.success() {
        return Err(Error::Filter(format!(
            "command filter exited with {}: {command}",
            output.status
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}
```

- [ ] **Step 6: テスト実行**

Run: `cargo test -p ghbox-core filter::`
Expected: PASS(既存 11 + 新規 6)

- [ ] **Step 7: fmt + clippy**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 8: Commit**

```bash
git add crates/ghbox-core/Cargo.toml Cargo.lock crates/ghbox-core/src/error.rs crates/ghbox-core/src/filter.rs
git commit -m "feat(core): external command section filter over JSONL"
```

---

### Task 7: inbox — build_sections(filter → 既読除外 → ソート)

**Files:**
- Modify: `crates/ghbox-core/src/inbox.rs`(既存 `Inbox` / `build_inbox` は**残したまま**追加。削除は Task 9)

**Interfaces:**
- Consumes: `config::{Section, SectionFilter}`、`filter::{CommentFilter, run_command_filter}`、`github::{Fetched, PrData}`、`item::{Item, CommentInfo}`、`store::{Store, KIND_MERGE_COMMENT}`(`is_done` / `is_done_pr`)
- Produces: `SectionData { title: String, items: Vec<Item> }`(`Clone`)、`type SectionResult = std::result::Result<SectionData, String>`(Err はステータスバー用メッセージ=当該セクションは前回表示維持)、`build_sections(sections: &[Section], fetched: &Fetched, store: &Store) -> Result<Vec<SectionResult>>`(async)

- [ ] **Step 1: inbox.rs に失敗するテストを追加**

tests モジュールに追加:

```rust
    use crate::config::{Column, Section, SectionFilter};
    use crate::github::{Fetched, PrData};
    use crate::item::CommentInfo;
    use crate::store::KIND_PR;

    fn section(filter: SectionFilter) -> Section {
        Section {
            title: "sec".into(),
            query: "q".into(),
            columns: vec![Column::Repo],
            filter,
        }
    }

    fn pr_data(repo: &str, number: u64, updated_at: &str, comments: Vec<CommentInfo>) -> PrData {
        PrData {
            repo: repo.into(),
            pr_number: number,
            pr_title: "t".into(),
            pr_url: "u".into(),
            pr_author: "author".into(),
            pr_updated_at: updated_at.into(),
            pr_created_at: "2026-01-01T00:00:00Z".into(),
            comments,
        }
    }

    fn cinfo(id: i64, author: &str, body: &str, created_at: &str) -> CommentInfo {
        CommentInfo {
            id,
            author: author.into(),
            body: body.into(),
            created_at: created_at.into(),
        }
    }

    fn fetched(sections: Vec<Vec<PrData>>) -> Fetched {
        Fetched {
            viewer_login: "nogu3".into(),
            sections,
        }
    }

    #[tokio::test]
    async fn none_filter_yields_pr_items() {
        let store = Store::open_in_memory().unwrap();
        let f = fetched(vec![vec![pr_data("o/r", 1, "2026-07-02T00:00:00Z", vec![])]]);
        let results = build_sections(&[section(SectionFilter::None)], &f, &store)
            .await
            .unwrap();
        let data = results[0].as_ref().unwrap();
        assert_eq!(data.title, "sec");
        assert_eq!(data.items.len(), 1);
        assert!(data.items[0].comment.is_none());
        assert_eq!(data.items[0].stable_id(), "pr:o/r#1");
    }

    #[tokio::test]
    async fn comment_mention_emits_item_per_matching_comment() {
        let store = Store::open_in_memory().unwrap();
        let f = fetched(vec![vec![pr_data(
            "o/r",
            1,
            "2026-07-02T00:00:00Z",
            vec![
                cinfo(1, "bot", "@nogu3 merge please", "2026-01-01T00:00:00Z"),
                cinfo(2, "bot", "@nogu3 マージして", "2026-01-02T00:00:00Z"),
                cinfo(3, "bot", "just chatting", "2026-01-03T00:00:00Z"),
                cinfo(4, "nogu3", "@nogu3 merge memo to self", "2026-01-04T00:00:00Z"),
            ],
        )]]);
        let results = build_sections(
            &[section(SectionFilter::CommentMention {
                extra_patterns: vec![],
            })],
            &f,
            &store,
        )
        .await
        .unwrap();
        let items = &results[0].as_ref().unwrap().items;
        // non-matching comment and the viewer's own comment are excluded
        let mut ids: Vec<i64> = items.iter().map(|i| i.comment.as_ref().unwrap().id).collect();
        ids.sort();
        assert_eq!(ids, vec![1, 2]);
    }

    #[tokio::test]
    async fn done_comment_ids_are_excluded() {
        let store = Store::open_in_memory().unwrap();
        store.mark_done(KIND_MERGE_COMMENT, "1").unwrap();
        let f = fetched(vec![vec![pr_data(
            "o/r",
            1,
            "2026-07-02T00:00:00Z",
            vec![
                cinfo(1, "bot", "@nogu3 merge", "2026-01-01T00:00:00Z"),
                cinfo(2, "bot", "@nogu3 merge", "2026-01-02T00:00:00Z"),
            ],
        )]]);
        let results = build_sections(
            &[section(SectionFilter::CommentMention {
                extra_patterns: vec![],
            })],
            &f,
            &store,
        )
        .await
        .unwrap();
        let items = &results[0].as_ref().unwrap().items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].comment.as_ref().unwrap().id, 2);
    }

    #[tokio::test]
    async fn done_pr_items_resurface_after_update() {
        let store = Store::open_in_memory().unwrap();
        store.mark_done_pr("o/r#1", "2026-07-02T00:00:00Z").unwrap();
        // same updatedAt → still done
        let f1 = fetched(vec![vec![pr_data("o/r", 1, "2026-07-02T00:00:00Z", vec![])]]);
        let r1 = build_sections(&[section(SectionFilter::None)], &f1, &store)
            .await
            .unwrap();
        assert!(r1[0].as_ref().unwrap().items.is_empty());
        // PR updated later → resurfaces
        let f2 = fetched(vec![vec![pr_data("o/r", 1, "2026-07-03T00:00:00Z", vec![])]]);
        let r2 = build_sections(&[section(SectionFilter::None)], &f2, &store)
            .await
            .unwrap();
        assert_eq!(r2[0].as_ref().unwrap().items.len(), 1);
        // KIND_PR is what got recorded
        assert!(store.is_done(KIND_PR, "o/r#1").unwrap());
    }

    #[tokio::test]
    async fn command_filter_retains_listed_ids_and_ignores_unknown() {
        let store = Store::open_in_memory().unwrap();
        let f = fetched(vec![vec![
            pr_data("o/r", 1, "2026-07-02T00:00:00Z", vec![]),
            pr_data("o/r", 2, "2026-07-02T00:00:00Z", vec![]),
        ]]);
        // prints one known id and one bogus id; bogus matches nothing
        let results = build_sections(
            &[section(SectionFilter::Command {
                command: "printf 'pr:o/r#2\\npr:bogus#9\\n'".into(),
            })],
            &f,
            &store,
        )
        .await
        .unwrap();
        let items = &results[0].as_ref().unwrap().items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].pr_number, 2);
    }

    #[tokio::test]
    async fn command_filter_failure_is_per_section() {
        let store = Store::open_in_memory().unwrap();
        let f = fetched(vec![
            vec![pr_data("o/r", 1, "2026-07-02T00:00:00Z", vec![])],
            vec![pr_data("o/r", 2, "2026-07-02T00:00:00Z", vec![])],
        ]);
        let sections = [
            section(SectionFilter::Command {
                command: "exit 1".into(),
            }),
            section(SectionFilter::None),
        ];
        let results = build_sections(&sections, &f, &store).await.unwrap();
        // failed section carries an error message including its title
        let err = results[0].as_ref().unwrap_err();
        assert!(err.contains("sec"), "got: {err}");
        // the other section is unaffected
        assert_eq!(results[1].as_ref().unwrap().items.len(), 1);
    }

    #[tokio::test]
    async fn items_sorted_by_repo_then_time_desc() {
        let store = Store::open_in_memory().unwrap();
        let f = fetched(vec![vec![
            pr_data("z/repo", 1, "2026-07-01T00:00:00Z", vec![]),
            pr_data("a/repo", 2, "2026-07-01T00:00:00Z", vec![]),
            pr_data("a/repo", 3, "2026-07-02T00:00:00Z", vec![]),
        ]]);
        let results = build_sections(&[section(SectionFilter::None)], &f, &store)
            .await
            .unwrap();
        let numbers: Vec<u64> = results[0]
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|i| i.pr_number)
            .collect();
        assert_eq!(numbers, vec![3, 2, 1]); // a/repo newest first, then z/repo
    }
```

- [ ] **Step 2: テスト実行(失敗確認)**

Run: `cargo test -p ghbox-core inbox::`
Expected: FAIL — `build_sections` / `SectionData` 未定義のコンパイルエラー

- [ ] **Step 3: inbox.rs に実装追加**

use を拡張(既存 use に追記):

```rust
use crate::config::{Section, SectionFilter};
use crate::filter::run_command_filter;
use crate::github::{Fetched, PrData};
use crate::item::Item;
```

既存 `build_inbox` の後に追加:

```rust
/// One section's rows, ready for display.
#[derive(Debug, Clone)]
pub struct SectionData {
    pub title: String,
    pub items: Vec<Item>,
}

/// Per-section result: Err carries a status-bar message and means the
/// frontend must keep showing the section's previous items (an empty
/// section would falsely read as "all clear").
pub type SectionResult = std::result::Result<SectionData, String>;

/// filter → read-state exclusion → sort, per section. `fetched.sections`
/// must be parallel to `sections`.
pub async fn build_sections(
    sections: &[Section],
    fetched: &Fetched,
    store: &Store,
) -> Result<Vec<SectionResult>> {
    let mut out = Vec::with_capacity(sections.len());
    for (section, prs) in sections.iter().zip(&fetched.sections) {
        let items = match &section.filter {
            SectionFilter::None => prs.iter().map(pr_item).collect(),
            SectionFilter::CommentMention { extra_patterns } => {
                let filter = CommentFilter::new(&fetched.viewer_login, extra_patterns)?;
                comment_items(prs, &filter, &fetched.viewer_login)
            }
            SectionFilter::Command { command } => {
                let candidates: Vec<Item> = prs.iter().map(pr_item).collect();
                match run_command_filter(command, &candidates).await {
                    Ok(keep) => candidates
                        .into_iter()
                        .filter(|item| keep.contains(&item.stable_id()))
                        .collect(),
                    Err(e) => {
                        out.push(Err(format!("{}: {e}", section.title)));
                        continue;
                    }
                }
            }
        };
        let mut items = exclude_done(items, store)?;
        items.sort_by(|a, b| {
            a.repo
                .cmp(&b.repo)
                .then_with(|| b.sort_time().cmp(a.sort_time()))
        });
        out.push(Ok(SectionData {
            title: section.title.clone(),
            items,
        }));
    }
    Ok(out)
}

fn pr_item(pr: &PrData) -> Item {
    Item {
        repo: pr.repo.clone(),
        pr_number: pr.pr_number,
        pr_title: pr.pr_title.clone(),
        pr_url: pr.pr_url.clone(),
        pr_author: pr.pr_author.clone(),
        pr_updated_at: pr.pr_updated_at.clone(),
        pr_created_at: pr.pr_created_at.clone(),
        comment: None,
    }
}

fn comment_items(prs: &[PrData], filter: &CommentFilter, viewer: &str) -> Vec<Item> {
    let mut items = Vec::new();
    for pr in prs {
        for comment in &pr.comments {
            if comment.author == viewer {
                continue; // own comments are not requests to me
            }
            if !filter.is_merge_request(&comment.body) {
                continue;
            }
            items.push(Item {
                comment: Some(comment.clone()),
                ..pr_item(pr)
            });
        }
    }
    items
}

fn exclude_done(items: Vec<Item>, store: &Store) -> Result<Vec<Item>> {
    let mut kept = Vec::new();
    for item in items {
        let done = match &item.comment {
            Some(c) => store.is_done(KIND_MERGE_COMMENT, &c.id.to_string())?,
            None => store.is_done_pr(&item.pr_key(), &item.pr_updated_at)?,
        };
        if !done {
            kept.push(item);
        }
    }
    Ok(kept)
}
```

- [ ] **Step 4: テスト実行**

Run: `cargo test -p ghbox-core inbox::`
Expected: PASS(既存 5 + 新規 7)

- [ ] **Step 5: fmt + clippy + 全テスト**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace`
Expected: クリーン

- [ ] **Step 6: Commit**

```bash
git add crates/ghbox-core/src/inbox.rs
git commit -m "feat(core): build_sections pipeline with per-section filter and read-state"
```

---

### Task 8: frontend — app / ui / main を config 駆動に書き換え

**Files:**
- Modify: `crates/ghbox/src/app.rs`(全面書き換え)
- Modify: `crates/ghbox/src/ui.rs`(全面書き換え: タブバー + Table + テーマ)
- Modify: `crates/ghbox/src/main.rs`(全面書き換え: keybindings 解決 + fetch_sections/build_sections)

**Interfaces:**
- Consumes: `Config`(sections/theme/keybindings)、`KeySpec`、`Column`、`Theme`、`NamedColor`、`ThemeColor`、`github::fetch_sections` / `Fetched`、`inbox::{build_sections, SectionData, SectionResult}`、`item::Item`、`store::{KIND_MERGE_COMMENT, Store}`(`mark_done` / `mark_done_pr`)
- Produces: `App`(`sections: Vec<SectionData>` + `active: usize`)、`DoneEntry`(enum: `Comment(i64)` / `Pr { key, updated_at }`)、`ui::draw(frame, &App, &Config)`

- [ ] **Step 1: app.rs を全面書き換え**

`crates/ghbox/src/app.rs` 全体を以下に置き換える:

```rust
use ghbox_core::inbox::{SectionData, SectionResult};
use ghbox_core::item::Item;

/// What pressing the done key should record for the selected item.
#[derive(Debug, Clone, PartialEq)]
pub enum DoneEntry {
    Comment(i64),
    Pr { key: String, updated_at: String },
}

pub struct App {
    /// One slot per config section, same order. Titles are filled at startup
    /// so the tab bar renders before the first fetch completes.
    /// Invariant: never empty (Config::validate rejects empty sections).
    pub sections: Vec<SectionData>,
    pub active: usize,
    pub selected: usize,
    pub status: String,
    pub should_quit: bool,
}

impl App {
    pub fn new(titles: Vec<String>) -> Self {
        Self {
            sections: titles
                .into_iter()
                .map(|title| SectionData {
                    title,
                    items: Vec::new(),
                })
                .collect(),
            active: 0,
            selected: 0,
            status: "loading...".into(),
            should_quit: false,
        }
    }

    pub fn active_section(&self) -> &SectionData {
        &self.sections[self.active]
    }

    pub fn items_len(&self) -> usize {
        self.active_section().items.len()
    }

    fn clamp_selected(&mut self) {
        let len = self.items_len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    /// Applies one fetch's per-section results. A section that failed (Err)
    /// keeps its previous items; the first error message is returned for
    /// the status bar.
    pub fn apply_results(&mut self, results: Vec<SectionResult>) -> Option<String> {
        let mut first_error = None;
        for (slot, result) in self.sections.iter_mut().zip(results) {
            match result {
                Ok(data) => *slot = data,
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }
        self.clamp_selected();
        first_error
    }

    pub fn next(&mut self) {
        if self.selected + 1 < self.items_len() {
            self.selected += 1;
        }
    }

    pub fn prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn next_section(&mut self) {
        self.active = (self.active + 1) % self.sections.len();
        self.selected = 0;
    }

    pub fn prev_section(&mut self) {
        self.active = (self.active + self.sections.len() - 1) % self.sections.len();
        self.selected = 0;
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.active_section().items.get(self.selected)
    }

    pub fn selected_url(&self) -> Option<&str> {
        self.selected_item().map(|i| i.pr_url.as_str())
    }

    pub fn selected_done_entry(&self) -> Option<DoneEntry> {
        self.selected_item().map(|i| match &i.comment {
            Some(c) => DoneEntry::Comment(c.id),
            None => DoneEntry::Pr {
                key: i.pr_key(),
                updated_at: i.pr_updated_at.clone(),
            },
        })
    }

    pub fn remove_selected(&mut self) {
        let idx = self.active;
        if self.selected < self.sections[idx].items.len() {
            self.sections[idx].items.remove(self.selected);
        }
        self.clamp_selected();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghbox_core::item::CommentInfo;

    fn pr_item(number: u64) -> Item {
        Item {
            repo: "o/r".into(),
            pr_number: number,
            pr_title: "t".into(),
            pr_url: format!("https://example.com/pr/{number}"),
            pr_author: "a".into(),
            pr_updated_at: "2026-07-02T00:00:00Z".into(),
            pr_created_at: "2026-07-01T00:00:00Z".into(),
            comment: None,
        }
    }

    fn comment_item(id: i64) -> Item {
        Item {
            pr_url: format!("https://example.com/{id}"),
            comment: Some(CommentInfo {
                id,
                author: "bob".into(),
                body: "@nogu3 merge".into(),
                created_at: "2026-07-03T00:00:00Z".into(),
            }),
            ..pr_item(1)
        }
    }

    fn app3() -> App {
        let mut app = App::new(vec!["A".into(), "B".into(), "C".into()]);
        app.sections[0].items = vec![comment_item(7), comment_item(8)];
        app.sections[1].items = vec![pr_item(3)];
        app
    }

    #[test]
    fn navigation_clamps_at_boundaries() {
        let mut app = app3();
        app.prev();
        assert_eq!(app.selected, 0);
        app.next();
        assert_eq!(app.selected, 1);
        app.next();
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn section_cycling_wraps_and_resets_selection() {
        let mut app = app3();
        app.next();
        app.next_section();
        assert_eq!(app.active, 1);
        assert_eq!(app.selected, 0);
        app.next_section();
        app.next_section();
        assert_eq!(app.active, 0); // wrapped
        app.prev_section();
        assert_eq!(app.active, 2); // wrapped backwards
    }

    #[test]
    fn done_entry_follows_item_kind() {
        let mut app = app3();
        assert_eq!(app.selected_done_entry(), Some(DoneEntry::Comment(7)));
        assert_eq!(app.selected_url(), Some("https://example.com/7"));
        app.next_section();
        assert_eq!(
            app.selected_done_entry(),
            Some(DoneEntry::Pr {
                key: "o/r#3".into(),
                updated_at: "2026-07-02T00:00:00Z".into()
            })
        );
    }

    #[test]
    fn empty_section_yields_none() {
        let mut app = app3();
        app.active = 2;
        assert_eq!(app.selected_url(), None);
        assert_eq!(app.selected_done_entry(), None);
    }

    #[test]
    fn apply_results_replaces_ok_and_keeps_err_sections() {
        let mut app = app3();
        let err = app.apply_results(vec![
            Ok(SectionData {
                title: "A".into(),
                items: vec![comment_item(9)],
            }),
            Err("B: command filter exited with 1".into()),
            Ok(SectionData {
                title: "C".into(),
                items: vec![],
            }),
        ]);
        assert_eq!(err.as_deref(), Some("B: command filter exited with 1"));
        assert_eq!(app.sections[0].items.len(), 1); // replaced
        assert_eq!(app.sections[1].items.len(), 1); // previous items kept
    }

    #[test]
    fn apply_results_clamps_selection() {
        let mut app = app3();
        app.next(); // selected = 1
        app.apply_results(vec![
            Ok(SectionData {
                title: "A".into(),
                items: vec![comment_item(9)],
            }),
            Ok(SectionData {
                title: "B".into(),
                items: vec![],
            }),
            Ok(SectionData {
                title: "C".into(),
                items: vec![],
            }),
        ]);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn remove_selected_clamps_selection() {
        let mut app = app3();
        app.next(); // select last of section 0
        app.remove_selected();
        assert_eq!(app.items_len(), 1);
        assert_eq!(app.selected, 0);
        app.remove_selected();
        assert_eq!(app.items_len(), 0);
        assert_eq!(app.selected, 0);
    }
}
```

- [ ] **Step 2: ui.rs を全面書き換え**

`crates/ghbox/src/ui.rs` 全体を以下に置き換える:

```rust
use ghbox_core::config::{Column, Config, Keybindings, NamedColor, Theme, ThemeColor};
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

fn help_line(kb: &Keybindings) -> String {
    format!(
        "{}/{}:移動  {}:切替  {}:開く  {}:対応済み  {}:更新  {}:終了",
        kb.down, kb.up, kb.next_section, kb.open, kb.done, kb.refresh, kb.quit
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

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
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
}
```

- [ ] **Step 3: main.rs を全面書き換え**

`crates/ghbox/src/main.rs` 全体を以下に置き換える:

```rust
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
```

- [ ] **Step 4: テスト実行**

Run: `cargo test --workspace`
Expected: PASS(app の新テスト 7 件、ui の 2 件を含む)

- [ ] **Step 5: fmt + clippy + ビルド**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo build -p ghbox`
Expected: クリーン

- [ ] **Step 6: Commit**

```bash
git add crates/ghbox/src/app.rs crates/ghbox/src/ui.rs crates/ghbox/src/main.rs
git commit -m "feat(tui): gh-dash style tab bar, table view, themes and keybindings"
```

---

### Task 9: 旧 API の削除(cleanup)

**Files:**
- Delete: `crates/ghbox-core/src/types.rs`
- Modify: `crates/ghbox-core/src/lib.rs`(`pub mod types;` 削除)
- Modify: `crates/ghbox-core/src/github.rs`(旧 `QUERY` / `Parsed` / `fetch` / `parse_response` / `MergePr` / `ReviewPr` / `Data` と旧テスト削除。`Actor` / `Repo` / `CommentConnection` / `Comment` / `Search` / `GqlError` / `get_token` は新パースが使うため**残す**)
- Modify: `crates/ghbox-core/src/inbox.rs`(旧 `Inbox` / `build_inbox` と旧テスト削除)
- Modify: `crates/ghbox-core/src/store.rs`(`KIND_REVIEW_REQUEST` 定数削除。マイグレーション SQL 内の `'review_request'` 文字列リテラルは**そのまま**。既存テストで `KIND_REVIEW_REQUEST` を使う箇所は `"review_request"` リテラルか `KIND_PR` に置き換え)

**Interfaces:**
- Consumes: Task 8 完了後のフロントエンド(旧 API を参照しない状態)
- Produces: 旧型が消えたクリーンな公開 API(`Item` / `Fetched` / `SectionData` 系のみ)

- [ ] **Step 1: types.rs 削除とlib.rs 更新**

```bash
git rm crates/ghbox-core/src/types.rs
```

`crates/ghbox-core/src/lib.rs` から `pub mod types;` の行を削除。

- [ ] **Step 2: github.rs から旧コード削除**

削除対象: `const QUERY`、`pub struct Parsed`、`struct Data`、`struct MergePr`、`struct ReviewPr`、`pub async fn fetch`、`pub fn parse_response`、`use crate::types::...`、tests モジュール内の `FIXTURE` と旧テスト 8 件(`parses_viewer_comments_and_reviews` / `ghost_author_becomes_unknown` / `empty_pr_node_is_skipped` / `graphql_errors_become_api_error` / `partial_data_with_errors_still_parses` / `neither_data_nor_errors_is_api_error` / `comment_without_database_id_is_skipped` / `review_node_without_repository_is_skipped`)。

**残すもの**: `Actor` / `Repo` / `CommentConnection` / `Comment` / `Search` / `GqlError` / `get_token` / Task 5 で追加した全コードと新テスト 5 件。

- [ ] **Step 3: inbox.rs から旧コード削除**

削除対象: `pub struct Inbox`、`pub fn build_inbox`、`use crate::types::...`、`use crate::store::KIND_REVIEW_REQUEST`、tests モジュール内の旧ヘルパ(`comment` / `review` / `parsed` / `setup`)と旧テスト 5 件。

- [ ] **Step 4: store.rs から KIND_REVIEW_REQUEST 削除**

`pub const KIND_REVIEW_REQUEST` の行を削除。store.rs の既存テスト `mark_done_then_is_done` / `mark_done_is_idempotent` 内の `KIND_REVIEW_REQUEST` 参照は文字列リテラル `"review_request"` に置き換える(kind 独立性のテスト意図は保たれる)。

- [ ] **Step 5: 全テスト + fmt + clippy**

Run: `cargo test --workspace && cargo fmt --all && cargo clippy --workspace -- -D warnings`
Expected: クリーン(旧 API 参照が残っているとここでコンパイルエラーになる)

- [ ] **Step 6: Commit**

```bash
git add -A crates/
git commit -m "refactor(core): drop legacy two-section types and APIs"
```

---

### Task 10: ドキュメント更新(CLAUDE.md / README)

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`

- [ ] **Step 1: CLAUDE.md 更新**

「## データフロー」節を以下に置き換え:

```markdown
## データフロー

1. config の各セクションから GraphQL search を alias(s0, s1, ...)で並べた1クエリを動的組み立てし、`viewer { login }` と合わせて1リクエストで取得(検索文字列は variables で渡す)
2. comment-mention フィルタを持つセクションのみ `comments(last: 50)` を追加取得
3. セクションごとにフィルタ適用: なし / comment-mention(同一コメント本文内に `@viewer` と `(?i)(merge|マージ)` または extra_patterns) / command(外部コマンドに JSONL を渡し残す id を受け取る)
4. 既読除外 → repo 昇順・時刻降順ソート → タブ+テーブルで表示
```

「## セクション」節を以下に置き換え:

```markdown
## セクション

config.toml の `[[sections]]` で自由定義(タイトル + GitHub 検索クエリ + フィルタ + カラム)。
config がなければ組み込みデフォルト2セクション(マージ依頼 + レビュー依頼)で動作する。

- フィルタ種別: なし / `comment-mention`(同一コメント内 mention+merge。コアロジック) / `command`(外部コマンド。stdin に JSONL、stdout に残す id)
- カラム: `repo` / `number` / `title` / `author` / `comment` / `updated` / `created`
```

「## 既読管理の原則」節を以下に置き換え:

```markdown
## 既読管理の原則

- コメントアイテム: **コメントID単位**(kind=`merge_comment`)。同一PRに新しい依頼コメントが来たら再浮上する
- PRアイテム: **PR + updatedAt 単位**(kind=`pr`、upsert)。マーク後に PR が更新されたら再浮上する
- 既読はセクション横断でグローバル(既読キーはアイテム自体から導出)
- SQLiteスキーマ変更時はマイグレーションを書く(append-only。NAS上のDBを壊さない)。DB の user_version がバイナリより新しい場合は起動拒否
```

「## キーバインド(MVP)」節を以下に置き換え:

```markdown
## キーバインド(デフォルト、config でリマップ可)

| キー | アクション | 動作 |
|---|---|---|
| j / k | down / up | 上下移動(矢印キーは常時有効) |
| Tab / BackTab | next_section / prev_section | セクション巡回 |
| Enter | open | ブラウザでPRを開く |
| d | done | 対応済みマーク |
| r | refresh | 手動リフレッシュ |
| q | quit | 終了 |
```

「## 規約」節の設定ファイル行を以下に置き換え:

```markdown
- 設定ファイルは `$XDG_CONFIG_HOME/ghbox/config.toml`(sections / theme / keybindings / ポーリング間隔 / DB パス。`deny_unknown_fields` で typo 検出、不正 config は起動時に即エラー終了)
```

アーキテクチャ節のツリーに `item.rs` 等の変更があれば現状に合わせて微修正(ツリーは crate 単位なので原則変更不要)。

- [ ] **Step 2: README.md 更新**

キーバインド表の下に1行追加:

```markdown
キーは `[keybindings]` でリマップできる(矢印キーの上下移動は常時有効)。
```

「## Config」節を以下に置き換え:

```markdown
## Config

`$XDG_CONFIG_HOME/ghbox/config.toml`(無ければ組み込みデフォルト: マージ依頼 + レビュー依頼の2セクション):

​```toml
poll_interval_secs = 300            # ポーリング間隔(秒、最小30)
db_path = "/nas/ghbox/state.db"     # 既読DB。デフォルト: $XDG_DATA_HOME/ghbox/state.db

[[sections]]
title = "マージ依頼"
query = "is:pr is:open mentions:@me"
columns = ["repo", "number", "title", "author", "comment"]
filter = { type = "comment-mention", extra_patterns = ["(?i)ship\\s*it"] }

[[sections]]
title = "レビュー依頼"
query = "is:pr is:open review-requested:@me"
# filter 省略 = 検索結果そのまま。columns 省略 = ["repo", "number", "title", "author", "updated"]

[[sections]]
title = "自分が関わるPR"
query = "is:pr is:open involves:@me"
# 外部コマンドフィルタ: stdin に1行1アイテムの JSON(id フィールド付き)、
# stdout に残すアイテムの id を1行1個返す。タイムアウト10秒
filter = { type = "command", command = "jq -r 'select(.pr_author != \"nogu3\") | .id'" }

[theme]                             # 省略キーはデフォルト。ratatui 名前付き色(小文字) or "#rrggbb"
tab_active = "yellow"
selection_bg = "blue"

[keybindings]                       # 省略キーはデフォルト。1文字 or tab/backtab/enter/up/down/esc
quit = "q"
done = "d"
​```
```

(コードフェンス内のゼロ幅文字は実際には書かない。通常の ``` を使う)

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: config-driven sections, themes, and keybindings"
```

---

## 実装後の手動確認(ユーザーと実施)

プラン外の手動スモークテスト(subagent では実施不可):

1. `cargo run -p ghbox` — デフォルト config で起動し、タブバー/テーブル/ステータスバーの表示、Tab 巡回、j/k 移動、Enter で PR オープン、d で既読、r でリフレッシュを確認
2. NAS 上の既存 v1 DB でのマイグレーション動作(バックアップを取ってから)
3. セクション数 N のクエリで rate limit cost を素振り確認(spec の未確認事項)
4. command フィルタの実運用例(`jq`)の動作確認
