# gh-dash 準拠キーバインド設計

日付: 2026-07-12

## 目的

キーバインドを gh-dash に寄せる。特にセクション切替を `Tab/BackTab` から矢印キー
（`←`/`→`、vim の `h`/`l` 併記）に置き換える。gh-dash の肝である「1アクションに複数キー」
を表現できるよう、現在の「1アクション=1キー」設計を複数キー対応に拡張する。

## 背景（現状）

- `KeySpec`（`crates/ghbox-core/src/config.rs`）は単一キーを表す enum
  （`Char / Tab / BackTab / Enter / Up / Down / Esc`）。
- `Keybindings` は各アクションが単一 `KeySpec`。デフォルト:
  `up=k / down=j / next_section=Tab / prev_section=BackTab / open=Enter / done=d / refresh=r / quit=q`。
- 上下移動のみ、`crates/ghbox/src/main.rs` の `handle_key` 末尾に矢印キーの
  ハードコードフォールバックがある（単一キー設計では矢印と `j/k` を両立できないため）。
  セクション切替には矢印フォールバックが無い。
- config バリデーションで、2アクションが同一キーに割り当たるとエラー
  （`config.rs` の重複検出ループ）。

## 変更方針

### 1. 複数キー対応（`KeyBinding`）

- 新型 `KeyBinding(Vec<KeySpec>)` を導入し、`Keybindings` の各フィールドを
  `KeySpec` から `KeyBinding` に変更する。
- `KeyBinding` の `Deserialize` は後方互換で以下2形式を受理:
  - 文字列単体: `open = "o"` → `[KeySpec::Char('o')]`
  - 配列: `down = ["down", "j"]` → `[KeySpec::Down, KeySpec::Char('j')]`
  - 実装は untagged 相当（`String` か `Vec<String>` を受けて各要素を `KeySpec` にパース）。
- 空配列（`[]`）は「そのアクションを無効化」ではなくエラーにする
  （typo 検出の一貫性のため。`deny_unknown_fields` の思想に合わせる）。

### 2. デフォルトキー（矢印を主・vim を副で併記 = gh-dash 相当）

| アクション | デフォルト |
|---|---|
| down | `↓`, `j` |
| up | `↑`, `k` |
| next_section | `→`, `l` |
| prev_section | `←`, `h` |
| open | `o` |
| done | `d` |
| refresh | `r` |
| quit | `q` |

- `done` は ghbox 独自アクション（gh-dash に相当なし）。`d` を維持。
- `Tab/BackTab` は `KeySpec` の enum バリアントとしては残す（config で使いたい人向け）が、
  デフォルトからは外す。

### 3. キー判定（`main.rs`）

- `key_matches(spec: KeySpec, code)` を `binding_matches(binding: &KeyBinding, code)` に置き換え、
  「binding 内のいずれかの `KeySpec` が `code` に一致」で判定する。
- `handle_key` 末尾の矢印ハードコードフォールバック（`KeyCode::Down` / `KeyCode::Up` の arm）は
  **削除**する。矢印がデフォルト binding に含まれるため不要。
- 各アクションの分岐は `binding_matches(&kb.<action>, code)` に更新。

### 4. 重複キー検出（`config.rs`）

- `bindings` 配列を `(&str, &KeyBinding)` に変更。
- 2つのアクション間で「同一 `KeySpec` が両方に含まれる」場合にエラーとする
  （どのキーが衝突したかをメッセージに含める）。エラーメッセージ書式は既存に準拠。

### 5. ドキュメント更新

- `README.md` のキーバインド表（L24-31）と説明文（L33「矢印キーの上下移動は常時有効」→
  新挙動に合わせて修正）、および `[keybindings]` の注記（L65「1文字 or tab/backtab/...」に
  配列も可である旨）を更新。
- `CLAUDE.md` のキーバインド表を新デフォルト（矢印主体、`open=o`、セクション=`h/l+矢印`）に更新。

## スコープ外

- gh-dash の検索 `/`・ヘルプ `?`・プレビュー・ページ移動 `g/G/d/u` 等、
  このツールに機能自体が無いものは対象外。
- `Enter` は open から外れ、デフォルトでは未割当になる（config で再割当は可能）。

## テスト

- `KeyBinding` deserialize: 文字列単体 / 配列 / 空配列エラー / 不正キー文字列エラー。
- デフォルト値のアサート（次セクションが `→` と `l` を含む 等）。
- 部分オーバーライド（一部アクションだけ config で上書きしても他はデフォルト維持）。
- 重複キー検出: 異なるアクションが同一キーを含むとエラー。
- 既存テスト（`keybindings_defaults_and_partial_override`, `duplicate_keybinding_is_error`,
  `KeySpec::BackTab.to_string()` 等）を新型に合わせて更新。

## 影響ファイル

- `crates/ghbox-core/src/config.rs`（`KeyBinding` 型、`Keybindings`、デフォルト、重複検出、テスト）
- `crates/ghbox/src/main.rs`（`binding_matches`、`handle_key`、フォールバック削除）
- `README.md`、`CLAUDE.md`
