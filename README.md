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
| j / k | 上下移動 |
| Tab | セクション切替 |
| Enter | ブラウザでPRを開く |
| d | 対応済みマーク |
| r | 手動リフレッシュ |
| q | 終了 |

## Config

`$XDG_CONFIG_HOME/ghbox/config.toml` (無ければデフォルト値):

```toml
poll_interval_secs = 300                 # ポーリング間隔(秒、最小30)
db_path = "/path/to/nas/ghbox/state.db"  # 既読DB。デフォルト: $XDG_DATA_HOME/ghbox/state.db
extra_patterns = ["(?i)ship\\s*it"]      # merge/マージ に追加するキーワード正規表現
```

## Development

```sh
cargo run -p ghbox          # TUI起動
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all
```
