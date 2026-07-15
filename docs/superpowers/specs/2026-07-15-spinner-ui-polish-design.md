# ghbox: fetch スピナーアニメーション + gh-dash 完成形リスタイル 設計

2026-07-15。gh-dash 風 UI 刷新(2026-07-13)の続き。fetch 中の固定 `⟳` をアニメーションスピナーに置き換え、配色・アイコン・レイアウトを gh-dash の完成形に近づける。

## ゴール

1. fetch 中はステータスバーで braille スピナーが回転する
2. デフォルトテーマが調和のとれた RGB パレット(catppuccin mocha ベース)になる
3. 行頭に PR 状態(open/draft/merged/closed)を示す色付き Nerd Font アイコンが出る
4. アクティブタブの下線・空表示の改善などレイアウト装飾

DB スキーマ変更なし。キーバインド変更なし。config の後方互換を維持する。

## 1. スピナーアニメーション

現在の描画はメッセージ駆動(キー入力・fetch 完了時のみ再描画)で、アニメーション用の tick が存在しない。

- `spawn_fetch` 内で fetch タスクと併せて tick タスクを spawn する: `fetching` フラグが立っている間 100ms ごとに `Msg::Redraw` を送り、フラグが落ちたら終了する。fetch タスクの panic 時も `FetchingGuard` の Drop がフラグを落とすため tick は止まる
- スピナーのフレームは状態を持たず描画時に壁時計から導出する: `spinner_frame(now_millis)` = `(now_millis / 100) % 10` 番目の braille 文字(`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`)。純関数として切り出す
- ステータスバー: fetching 中は `⟳` の代わりにスピナーフレームを accent 色で表示。idle 時は従来どおり `✓`(色は `state_open` の緑)

## 2. 配色刷新

デフォルト `Theme` の値を ANSI 名前色から catppuccin mocha ベースの RGB に差し替える。config の `[theme]` による上書き(RGB・名前色とも)は従来どおり。

| キー | 新デフォルト |
|---|---|
| tab_active | `#cba6f7` (mauve) |
| tab_inactive | `#6c7086` (overlay) |
| border | `#45475a` (surface1) |
| selection_bg | `#313244` (surface0) |
| selection_fg | `#cdd6f4` (text) |
| table_header | `#b4befe` (lavender) |
| status_bar | `#6c7086` (overlay) |
| pr_number | `#89b4fa` (blue) |
| author | `#f5c2e7` (pink) |
| time | `#7f849c` (overlay1) |

新規テーマキー(すべて serde default 付きで後方互換):

| キー | デフォルト | 用途 |
|---|---|---|
| faint | `#6c7086` | repo/comment カラム(現状ハードコードの DarkGray を置換) |
| state_open | `#a6e3a1` (green) | open アイコン |
| state_draft | `#6c7086` (overlay) | draft アイコン |
| state_merged | `#cba6f7` (mauve) | merged アイコン |
| state_closed | `#f38ba8` (red) | closed アイコン |

既存テストの名前色アサーション(Yellow/Green/Blue/White など)は新 RGB 値に更新する。

## 3. PR 状態アイコン(ghbox-core 変更)

- GraphQL search クエリの PullRequest ノードに `state`(OPEN/CLOSED/MERGED)と `isDraft` を追加取得する
- `Item` に `state: PrState` を追加。`PrState` は `Open | Draft | Merged | Closed` の enum で、Draft は `state == OPEN && isDraft` から導出する。command フィルタへの JSONL 出力にもフィールドが増える(追加のみで互換)
- 新カラム `Column::State`: ヘッダーは空文字、幅 2。組み込みデフォルト2セクションの先頭カラムに追加する
- アイコン(Nerd Font):  open /  draft /  merged /  closed。fg は対応する `state_*` テーマ色。選択行では従来どおり selection_fg に潰さず、アイコンセルのみ状態色を維持する(highlight_symbol と同じ理屈で per-cell 指定)
- config トップレベルに `icons: bool`(デフォルト `true`)。`false` の場合は全状態 `●` で表示し、色分けは維持する

## 4. レイアウト装飾

- タブ下の罫線(`draw_rule`)のうち、アクティブタブ直下の x 範囲だけ accent 色(`tab_active`)の `━` を重ね描きして下線化する。x 範囲はタブ行の span 幅(unicode-width)から計算する
- アクティブタブの件数カウントを accent 色にする(非アクティブは従来どおり dim)
- 空セクションのプレースホルダを `All clear — no items`(faint 色、中央寄せ)に変更する

## エラーハンドリング

既存方針から変更なし。tick タスクは `fetching` フラグ監視のみで自律終了し、チャネル閉鎖時は send 失敗で抜ける。

## テスト方針

- `spinner_frame` 純関数の境界テスト(0ms / 100ms / 1000ms で巡回)
- `fetching=true` の描画で braille 文字がステータスバーに現れることをアサート
- 状態別アイコン: 各 `PrState` の色を座標ピンで検証(選択行でも状態色が保持されること)
- config: `icons` フラグと新テーマキーのパース + デフォルト値テスト
- github.rs: `state` / `isDraft` を含むレスポンス fixture のパーステスト(OPEN+draft → Draft 導出を含む)
- タブ下線: アクティブタブ配下のセルが `━` かつ accent 色、それ以外は `─` かつ border 色

## 実装しない項目

- スピナー種類の config 化(braille 固定)
- ライトテーマ対応
- State カラムの並び替え・フィルタ連動
