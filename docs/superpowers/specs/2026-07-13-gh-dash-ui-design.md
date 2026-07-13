# gh-dash 風 UI 刷新 Design

TUI の見た目を gh-dash に寄せて全面刷新する。ロジック(fetch / filter / store)には触れない。

## ゴール

- 外枠ボーダーを廃した軽いレイアウト
- カラムごとの色分けで一覧の視認性を上げる
- 相対時刻(`2d` / `5h`)で鮮度を直感的に読めるようにする
- fetch 中であることをステータスバーで示す

## レイアウト

```
  マージ依頼 2 │ レビュー依頼 1
 ──────────────────────────────────────
  REPO         #     TITLE      AUTHOR   UPDATED
▌ nogu3/casa   #12   Fix xxx    alice    2d
  nogu3/mando  #7    Add yyy    bob      5h

 ✓ 12:34:56 · ↓↑ move · ←→ section · o open · d done · r refresh · q quit
```

- **タブ行**: `{title} {count}` を `│`(dim)区切りで並べる。アクティブタブは accent 色 + bold、カウントは dim。ratatui `Tabs` ウィジェットは divider・カウント色制御のため `Line` 手組みに置き換えてよい
- **罫線**: タブ直下に水平罫線1本(`border` 色)。テーブルの `Block`(全周ボーダー)は廃止
- **テーブルヘッダ**: 大文字ラベル(`REPO` / `#` / `TITLE` / `AUTHOR` / `COMMENT` / `UPDATED` / `CREATED`)、`table_header` 色 + bold
- **選択行**: 行頭に `▌` マーカー(accent 色)。行全体は従来どおり `selection_bg` / `selection_fg`。非選択行の行頭は空白2文字でインデントを揃える
- **空セクション**: テーブル領域の中央に `no items`(dim)
- **ステータスバー**: 左に fetch 状態(`⟳ fetching…` or `✓ HH:MM:SS`)、続けて `·` 区切りの英語ヘルプ。スピナーはアニメーションしない(再描画タイマーを増やさない)

## カラム色分け

| カラム | 色 |
|---|---|
| repo | DarkGray(固定。テーマ化しない) |
| number | `theme.pr_number`(デフォルト Green) |
| title | デフォルト前景色 |
| author | `theme.author`(デフォルト Magenta) |
| comment | DarkGray(固定) |
| updated / created | `theme.time`(デフォルト DarkGray) |

`Theme` に `pr_number` / `author` / `time` の3フィールドを追加する。`#[serde(default)]`
なので既存 config と後方互換。選択マーカー `▌` と `⟳` / `✓` は既存 `tab_active` を
accent として流用し、フィールドは増やさない。

## 相対時刻

`fmt_ts` を相対表記に置換する:

- < 1分: `now`
- < 60分: `{X}m`
- < 24時間: `{X}h`
- < 30日: `{X}d`
- それ以降: `MM-DD`(文字列スライス、従来方式)
- パース不能(非 ASCII 等): 入力をそのまま表示(従来同様)

実装は `fmt_relative(ts: &str, now_epoch: i64) -> String` として now を注入しテスト可能にする。
ISO8601 のパースに `time` crate(`parsing` feature、`OffsetDateTime::parse` + Rfc3339)を
ghbox crate に追加。now は `SystemTime::now()` から epoch 秒を取る。

## fetch 中表示

main.rs の fetching フラグ(AtomicBool)の現在値を、描画直前に読み取り `draw` の引数
(`fetching: bool`)として渡す。`App` には状態を持たせない(真実は AtomicBool 側にあり、
複製すると同期漏れの余地が生まれるため)。fetch 完了時は従来どおり status 文字列が更新される。

## テスト

- `fmt_relative` の境界値テスト(now / m / h / d / MM-DD / 不正入力)
- 既存レンダリングテスト(`renders_tabs_table_and_status`)を新レイアウトに更新:
  大文字ヘッダ、タブカウント、`▌` マーカー、英語ヘルプ行、`no items` 表示
- ヘルプ行テストを英語表記に更新

## 変更範囲

- `crates/ghbox/src/ui.rs` — レイアウト・色・相対時刻(主変更)
- `crates/ghbox/src/main.rs` / `app.rs` — fetching 状態の受け渡し
- `crates/ghbox-core/src/config.rs` — `Theme` 3フィールド追加
- `crates/ghbox/Cargo.toml` — `time` crate 追加

## 非スコープ

- スピナーアニメーション(再描画タイマー追加)
- 詳細ペイン
- セクション名・タブ構成の変更
