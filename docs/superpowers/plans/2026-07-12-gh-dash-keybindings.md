# gh-dash 準拠キーバインド Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** キーバインドを gh-dash に寄せ、1アクションに複数キーを割り当て可能にし、セクション切替を矢印（`←`/`→`）と vim（`h`/`l`）にする。

**Architecture:** `ghbox-core` の `Keybindings` を単一 `KeySpec` から `KeyBinding(Vec<KeySpec>)` に拡張。config は文字列単体・配列の両方を受理（後方互換）。`ghbox` の `handle_key` は各アクションの `KeyBinding` に対し「いずれかのキー一致」で判定し、上下矢印のハードコードフォールバックを撤去する。

**Tech Stack:** Rust (edition 2024), serde, crossterm, ratatui。

## Global Constraints

- lib のエラーは `thiserror`（`ghbox_core::Error::Config`）、バイナリは `anyhow`。
- config は `#[serde(default, deny_unknown_fields)]`。不正 config は起動時に即エラー。
- コミットメッセージは英語・conventional commits。
- 検証: `cargo test --workspace` / `cargo clippy --workspace -- -D warnings` / `cargo fmt --all`。
- コミット対象は本作業で編集したファイルのみを明示 `git add`。

---

### Task 1: `KeySpec` に `Left` / `Right` を追加

矢印セクション切替のために方向キー2種を追加する。`KeySpec` へのバリアント追加は
`main.rs` の `key_matches` の網羅 match を壊すため、同タスクで両ファイルを更新して
ワークスペースをビルド可能に保つ。

**Files:**
- Modify: `crates/ghbox-core/src/config.rs`（`KeySpec` enum / `FromStr` / `Display` / テスト）
- Modify: `crates/ghbox/src/main.rs`（`key_matches` に arm 追加）

**Interfaces:**
- Produces: `KeySpec::Left`, `KeySpec::Right`（文字列 `"left"` / `"right"` にパース、`Display` は同名）。

- [ ] **Step 1: 失敗するテストを書く**

`crates/ghbox-core/src/config.rs` の `mod tests` 内に追加:

```rust
    #[test]
    fn left_right_keys_parse() {
        let cfg = parse("[keybindings]\nnext_section = \"right\"\nprev_section = \"left\"\n").unwrap();
        assert_eq!(cfg.keybindings.next_section, KeySpec::Right);
        assert_eq!(cfg.keybindings.prev_section, KeySpec::Left);
        assert_eq!(KeySpec::Left.to_string(), "left");
        assert_eq!(KeySpec::Right.to_string(), "right");
    }
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p ghbox-core left_right_keys_parse`
Expected: コンパイルエラー（`KeySpec::Left` / `KeySpec::Right` が存在しない）で FAIL。

- [ ] **Step 3: `KeySpec` に variant を追加**

`crates/ghbox-core/src/config.rs` の `KeySpec` enum（`Up` / `Down` の近く）を更新:

```rust
pub enum KeySpec {
    Char(char),
    Tab,
    BackTab,
    Enter,
    Up,
    Down,
    Left,
    Right,
    Esc,
}
```

`FromStr` の match に追加（`"down" => ...` の後）:

```rust
            "up" => Ok(KeySpec::Up),
            "down" => Ok(KeySpec::Down),
            "left" => Ok(KeySpec::Left),
            "right" => Ok(KeySpec::Right),
            "esc" => Ok(KeySpec::Esc),
```

`FromStr` のエラーメッセージ（`expected one character or ...`）に `left/right` を含める:

```rust
                    _ => Err(format!(
                        "invalid key {s:?}: expected one character or tab/backtab/enter/up/down/left/right/esc"
                    )),
```

`Display` の match に追加（`KeySpec::Down => ...` の後）:

```rust
            KeySpec::Up => write!(f, "up"),
            KeySpec::Down => write!(f, "down"),
            KeySpec::Left => write!(f, "left"),
            KeySpec::Right => write!(f, "right"),
            KeySpec::Esc => write!(f, "esc"),
```

- [ ] **Step 4: `main.rs` の `key_matches` を網羅させる**

`crates/ghbox/src/main.rs` の `key_matches`（`KeySpec::Down => ...` の後）に追加:

```rust
        KeySpec::Up => code == KeyCode::Up,
        KeySpec::Down => code == KeyCode::Down,
        KeySpec::Left => code == KeyCode::Left,
        KeySpec::Right => code == KeyCode::Right,
        KeySpec::Esc => code == KeyCode::Esc,
```

- [ ] **Step 5: テストが通り、ワークスペースがビルドできることを確認**

Run: `cargo test -p ghbox-core left_right_keys_parse && cargo build --workspace`
Expected: テスト PASS、ビルド成功。

- [ ] **Step 6: コミット**

```bash
git add crates/ghbox-core/src/config.rs crates/ghbox/src/main.rs
git commit -m "feat(core): add Left/Right key specs"
```

---

### Task 2: 複数キー対応 `KeyBinding` とデフォルト・判定の刷新

`Keybindings` の各フィールドを `KeyBinding(Vec<KeySpec>)` に置き換え、デフォルトを
矢印主体（vim 併記）にし、`handle_key` を複数キー判定に更新、上下矢印フォールバックを撤去する。
config と main.rs を同タスクで変更し、ワークスペースを常にビルド可能に保つ。

**Files:**
- Modify: `crates/ghbox-core/src/config.rs`（`KeyBinding` 型 / `Keybindings` フィールド / `Default` / 重複検出 / テスト）
- Modify: `crates/ghbox/src/main.rs`（`binding_matches` / `handle_key` / フォールバック撤去 / import）

**Interfaces:**
- Consumes: `KeySpec`（`Left` / `Right` 含む、Task 1）。
- Produces:
  - `pub struct KeyBinding(pub Vec<KeySpec>)`（`Debug, Clone, PartialEq, Eq`）。文字列単体・配列を Deserialize。
  - `Keybindings` の各フィールド型 = `KeyBinding`。
  - `fn binding_matches(binding: &KeyBinding, code: KeyCode) -> bool`（`main.rs`）。

- [ ] **Step 1: `KeyBinding` の失敗するテストを書く**

`crates/ghbox-core/src/config.rs` の `mod tests` 内に追加:

```rust
    #[test]
    fn keybinding_accepts_string_and_array() {
        let cfg = parse("[keybindings]\ndown = [\"down\", \"j\"]\nopen = \"o\"\n").unwrap();
        assert_eq!(
            cfg.keybindings.down,
            KeyBinding(vec![KeySpec::Down, KeySpec::Char('j')])
        );
        assert_eq!(cfg.keybindings.open, KeyBinding(vec![KeySpec::Char('o')]));
    }

    #[test]
    fn keybinding_empty_array_is_error() {
        assert!(parse("[keybindings]\nquit = []\n").is_err());
    }

    #[test]
    fn keybinding_defaults_are_arrow_first_with_vim() {
        let cfg = parse("").unwrap();
        assert_eq!(
            cfg.keybindings.next_section,
            KeyBinding(vec![KeySpec::Right, KeySpec::Char('l')])
        );
        assert_eq!(
            cfg.keybindings.prev_section,
            KeyBinding(vec![KeySpec::Left, KeySpec::Char('h')])
        );
        assert_eq!(
            cfg.keybindings.down,
            KeyBinding(vec![KeySpec::Down, KeySpec::Char('j')])
        );
        assert_eq!(cfg.keybindings.open, KeyBinding(vec![KeySpec::Char('o')]));
    }
```

- [ ] **Step 2: 既存テストを新型に合わせて更新**

`keybindings_defaults_and_partial_override` を更新:

```rust
    #[test]
    fn keybindings_defaults_and_partial_override() {
        let cfg = parse("[keybindings]\nquit = \"x\"\n").unwrap();
        assert_eq!(cfg.keybindings.quit, KeyBinding(vec![KeySpec::Char('x')]));
        assert_eq!(
            cfg.keybindings.next_section,
            KeyBinding(vec![KeySpec::Right, KeySpec::Char('l')])
        );
    }
```

`special_key_names_parse` を更新:

```rust
    #[test]
    fn special_key_names_parse() {
        let cfg = parse("[keybindings]\nquit = \"esc\"\nopen = \"o\"\n").unwrap();
        assert_eq!(cfg.keybindings.quit, KeyBinding(vec![KeySpec::Esc]));
        assert_eq!(cfg.keybindings.open, KeyBinding(vec![KeySpec::Char('o')]));
    }
```

`duplicate_keybinding_is_error` はそのまま（`quit = "j"` はデフォルト `down`（`j` を含む）と衝突）。
念のため確認のみ。

- [ ] **Step 3: テストが失敗することを確認**

Run: `cargo test -p ghbox-core`
Expected: コンパイルエラー（`KeyBinding` 未定義 / フィールド型不一致）で FAIL。

- [ ] **Step 4: `KeyBinding` 型を実装**

`crates/ghbox-core/src/config.rs` の `KeySpec` の `Deserialize` impl の直後に追加:

```rust
/// One config action's keys. Accepts a single key string or a non-empty
/// array of key strings, so `open = "o"` and `down = ["down", "j"]` both work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding(pub Vec<KeySpec>);

impl<'de> serde::Deserialize<'de> for KeyBinding {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = KeyBinding;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a key string or a non-empty array of key strings")
            }

            fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<KeyBinding, E> {
                let spec = s.parse::<KeySpec>().map_err(E::custom)?;
                Ok(KeyBinding(vec![spec]))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<KeyBinding, A::Error> {
                let mut keys = Vec::new();
                while let Some(s) = seq.next_element::<String>()? {
                    keys.push(s.parse::<KeySpec>().map_err(serde::de::Error::custom)?);
                }
                if keys.is_empty() {
                    return Err(serde::de::Error::custom(
                        "keybinding must have at least one key",
                    ));
                }
                Ok(KeyBinding(keys))
            }
        }
        d.deserialize_any(V)
    }
}
```

- [ ] **Step 5: `Keybindings` のフィールドとデフォルトを更新**

`Keybindings` 構造体のフィールド型を `KeyBinding` に変更:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Keybindings {
    pub up: KeyBinding,
    pub down: KeyBinding,
    pub next_section: KeyBinding,
    pub prev_section: KeyBinding,
    pub open: KeyBinding,
    pub done: KeyBinding,
    pub refresh: KeyBinding,
    pub quit: KeyBinding,
}
```

`Default` impl を新デフォルト（矢印主体・vim 併記）に:

```rust
impl Default for Keybindings {
    fn default() -> Self {
        Self {
            up: KeyBinding(vec![KeySpec::Up, KeySpec::Char('k')]),
            down: KeyBinding(vec![KeySpec::Down, KeySpec::Char('j')]),
            next_section: KeyBinding(vec![KeySpec::Right, KeySpec::Char('l')]),
            prev_section: KeyBinding(vec![KeySpec::Left, KeySpec::Char('h')]),
            open: KeyBinding(vec![KeySpec::Char('o')]),
            done: KeyBinding(vec![KeySpec::Char('d')]),
            refresh: KeyBinding(vec![KeySpec::Char('r')]),
            quit: KeyBinding(vec![KeySpec::Char('q')]),
        }
    }
}
```

- [ ] **Step 6: 重複キー検出を複数キー対応に更新**

`Config` の validate 内の重複検出ブロック（`let bindings = [ ... ]` から二重ループ）を置換:

```rust
        let kb = &self.keybindings;
        let bindings = [
            ("up", &kb.up),
            ("down", &kb.down),
            ("next_section", &kb.next_section),
            ("prev_section", &kb.prev_section),
            ("open", &kb.open),
            ("done", &kb.done),
            ("refresh", &kb.refresh),
            ("quit", &kb.quit),
        ];
        for (i, (name_a, binding_a)) in bindings.iter().enumerate() {
            for (name_b, binding_b) in &bindings[i + 1..] {
                for key in &binding_a.0 {
                    if binding_b.0.contains(key) {
                        return Err(Error::Config(format!(
                            "keybindings: {name_a} and {name_b} are both bound to \"{key}\""
                        )));
                    }
                }
            }
        }
        Ok(())
```

- [ ] **Step 7: `main.rs` を複数キー判定に更新**

`crates/ghbox/src/main.rs` の import に `KeyBinding` を追加:

```rust
use ghbox_core::config::{Config, KeyBinding, KeySpec, Section};
```

`key_matches` の直後に `binding_matches` を追加:

```rust
fn binding_matches(binding: &KeyBinding, code: KeyCode) -> bool {
    binding.0.iter().any(|spec| key_matches(*spec, code))
}
```

`handle_key` 内の各 `key_matches(kb.X, code)` を `binding_matches(&kb.X, code)` に置換し、
末尾の矢印フォールバック arm を削除。置換後の分岐部（`let kb = ...;` 以降）は次の通り:

```rust
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
```

削除する末尾コメントと arm（`// Configured bindings take precedence; ...` および
`} else if code == KeyCode::Down { app.next(); } else if code == KeyCode::Up { app.prev(); }`）は
残さないこと。

- [ ] **Step 8: テスト・ビルド・lint を確認**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all --check`
Expected: 全テスト PASS、clippy 警告なし、fmt 差分なし（差分が出たら `cargo fmt --all` 後に再確認）。

- [ ] **Step 9: コミット**

```bash
git add crates/ghbox-core/src/config.rs crates/ghbox/src/main.rs
git commit -m "feat: multi-key bindings with arrow-first gh-dash defaults"
```

---

### Task 3: ドキュメント更新

新デフォルトと複数キー config を README / CLAUDE.md に反映する。

**Files:**
- Modify: `README.md`（キーバインド表 L24-31 / 説明文 L33 / `[keybindings]` 注記 L65）
- Modify: `CLAUDE.md`（キーバインド表）

- [ ] **Step 1: README のキーバインド表と注記を更新**

`README.md` の表（現行 L24-31）を置換:

```markdown
| キー | 動作 |
|---|---|
| ↓ / j | 下移動 |
| ↑ / k | 上移動 |
| → / l | 次セクション |
| ← / h | 前セクション |
| o | ブラウザでPRを開く |
| d | 対応済みマーク |
| r | 手動リフレッシュ |
| q | 終了 |
```

説明文（現行 L33）を置換:

```markdown
キーは `[keybindings]` でリマップできる(1アクションに複数キーを配列で割り当て可)。
```

`[keybindings]` の注記コメント（現行 L65）と例を置換:

```toml
[keybindings]                       # 省略キーはデフォルト。値は1文字/tab/backtab/enter/up/down/left/right/esc、または配列
quit = "q"
done = "d"
next_section = ["right", "l"]       # 配列で複数キー割り当て
```

- [ ] **Step 2: CLAUDE.md のキーバインド表を更新**

`CLAUDE.md` の「## キーバインド」表を新デフォルトに置換:

```markdown
| キー | アクション | 動作 |
|---|---|---|
| ↓ / j | down | 下移動 |
| ↑ / k | up | 上移動 |
| → / l | next_section | 次セクション |
| ← / h | prev_section | 前セクション |
| o | open | ブラウザでPRを開く |
| d | done | 対応済みマーク |
| r | refresh | 手動リフレッシュ |
| q | quit | 終了 |
```

見出し行「## キーバインド(デフォルト、config でリマップ可)」の直後や近辺に矢印常時有効の
旧記述があれば削除（矢印はデフォルト binding に含まれるため）。

- [ ] **Step 3: コミット**

```bash
git add README.md CLAUDE.md
git commit -m "docs: update keybindings for gh-dash-style defaults"
```

---

## 完了時

`superpowers:finishing-a-development-branch` で main への統合方法（merge / PR）を確認する。
