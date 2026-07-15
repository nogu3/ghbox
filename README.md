# ghbox

GitHub 上で「ボールが自分にあるPR」を常時表示・操作する TUI。

同一コメント内に `@自分` と `merge/マージ` の両方を含むマージ依頼と、
レビュー依頼(`review-requested:@me`)をリポジトリごとに一覧し、
対応済み管理する。GitHub 検索はこの「同一コメント内」条件を表現できない
ため、コメント本文を取得してローカルでフィルタする。

## Install

```sh
cargo install --path crates/ghbox
```

認証は `gh auth token` を利用する。事前に `gh auth login` しておくこと。

## Usage

```sh
ghbox
```

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

キーは `[keybindings]` でリマップできる(1アクションに複数キーを配列で割り当て可)。

## Config

`$XDG_CONFIG_HOME/ghbox/config.toml`(無ければ組み込みデフォルト: マージ依頼 + レビュー依頼の2セクション):

```toml
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
# filter 省略 = 検索結果そのまま。columns 省略 = ["state", "repo", "number", "title", "author", "updated"]

[[sections]]
title = "自分が関わるPR"
query = "is:pr is:open involves:@me"
# 外部コマンドフィルタ: stdin に1行1アイテムの JSON(id フィールド付き)、
# stdout に残すアイテムの id を1行1個返す。タイムアウト10秒
filter = { type = "command", command = "jq -r 'select(.pr_author != \"nogu3\") | .id'" }

icons = true                        # デフォルトの state アイコンは Nerd Font 前提。無い環境は false で ● 表示に

[theme]                             # 省略キーはデフォルト(catppuccin mocha)。ratatui 名前付き色(小文字) or "#rrggbb"
tab_active = "#cba6f7"              # アクティブタブ・タブ下線・選択マーカー・スピナーの accent 色
selection_bg = "#313244"
pr_number = "#89b4fa"               # PR番号カラム。ほかに author / time / faint カラム色も指定可
state_open = "#a6e3a1"              # state カラムのアイコン色: state_draft / state_merged / state_closed も指定可

[keybindings]                       # 省略キーはデフォルト。値は1文字/tab/backtab/enter/up/down/left/right/esc、または配列
quit = "q"
done = "d"
next_section = ["right", "l"]       # 配列で複数キー割り当て
```

## Development

```sh
cargo run -p ghbox          # TUI起動
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all
```
