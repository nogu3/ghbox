# ghbox

GitHub上で「ボールが自分にあるPR」を常時表示・操作するTUI。
マージ依頼(コメント内メンション)とレビュー依頼をリポジトリごとに一覧し、既読(対応済み)管理する。

## 存在理由

GitHub検索は「同一コメント内に @自分 と merge/マージ の両方を含む」という条件を表現できない(`mentions:@me` と `in:comments` は各々PR内のどこかでマッチすれば良いため)。この同一コメント内フィルタが本ツールのコアロジック。

## アーキテクチャ

casa/casad パターン踏襲。コアをlibに分離し、frontendを差し替え可能にする。

```
ghbox/
├── crates/
│   ├── ghbox-core/   # GraphQL fetch、コメントフィルタ、型、SQLite状態管理
│   └── ghbox/        # ratatui TUI frontend
└── CLAUDE.md
```

将来: mando からも ghbox-core を叩けるようにする(HTTPエンドポイント or 直接依存)。

## 技術スタック

- Rust (edition 2024)
- ratatui + crossterm
- tokio(バックグラウンド定期fetch)
- reqwest + GitHub GraphQL API v4
- rusqlite(既読状態。DBはNAS上に置きマシン間同期: schliemann-drill と同じ方式)
- 認証: `gh auth token` の出力を利用。トークンを自前管理しない

## データフロー

1. config の各セクションから GraphQL search を alias(s0, s1, ...)で並べた1クエリを動的組み立てし、`viewer { login }` と合わせて1リクエストで取得(検索文字列は variables で渡す)
2. comment-mention フィルタを持つセクションのみ `comments(last: 50)` を追加取得
3. セクションごとにフィルタ適用: なし / comment-mention(同一コメント本文内に `@viewer` と `(?i)(merge|マージ)` または extra_patterns) / command(外部コマンドに JSONL を渡し残す id を受け取る)
4. 既読除外 → repo 昇順・時刻降順ソート → タブ+テーブルで表示

## セクション

config.toml の `[[sections]]` で自由定義(タイトル + GitHub 検索クエリ + フィルタ + カラム)。
config がなければ組み込みデフォルト2セクション(マージ依頼 + レビュー依頼)で動作する。

- フィルタ種別: なし / `comment-mention`(同一コメント内 mention+merge。コアロジック) / `command`(外部コマンド。stdin に JSONL、stdout に残す id)
- カラム: `repo` / `number` / `title` / `author` / `comment` / `updated` / `created`

## 既読管理の原則

- コメントアイテム: **コメントID単位**(kind=`merge_comment`)。同一PRに新しい依頼コメントが来たら再浮上する
- PRアイテム: **PR + updatedAt 単位**(kind=`pr`、upsert)。マーク後に PR が更新されたら再浮上する
- 既読はセクション横断でグローバル(既読キーはアイテム自体から導出)
- SQLiteスキーマ変更時はマイグレーションを書く(append-only。NAS上のDBを壊さない)。DB の user_version がバイナリより新しい場合は起動拒否

## キーバインド(デフォルト、config でリマップ可)

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

## 開発コマンド

```sh
cargo run -p ghbox          # TUI起動
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all
```

## 規約

- UNIX哲学: ghbox-core は副作用(端末描画)を持たない。TUIは表示と入力のみ
- エラーは anyhow(バイナリ) / thiserror(lib)
- GraphQLクエリは .graphql ファイルに分離せず、まずは文字列リテラルで開始(小さく始める)
- 設定ファイルは `$XDG_CONFIG_HOME/ghbox/config.toml`(sections / theme / keybindings / ポーリング間隔 / DB パス。`deny_unknown_fields` で typo 検出、不正 config は起動時に即エラー終了)
- コミットメッセージは英語、conventional commits

## 未確定事項(実装しながら決める)

- [ ] GraphQL search でコメント本文まで一発取得できるか、レート制限に収まるか(最初に素振りで確認)
- [ ] マージ依頼の検索対象を open のみにするか merged 含むか
- [ ] review thread 内コメント(PullRequestReviewComment)も対象にするか、issue comment のみか
- [ ] ポーリング間隔のデフォルト(案: 5分)
- [ ] NAS上のSQLiteパス
