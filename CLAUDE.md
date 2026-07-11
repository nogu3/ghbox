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

1. GraphQL `search(query: "is:pr mentions:@me", type: ISSUE)` + `comments(last: 50)` を1クエリで取得
2. 各コメントを正規表現でフィルタ: `(?i)(merge|マージ)` と `@{viewer_login}` を**同一コメント本文内**に両方含むもののみ採用
3. レビュー依頼は `is:pr is:open review-requested:@me` で別クエリ(フィルタ不要)
4. リポジトリごとにグルーピングして表示

## セクション

- **マージ依頼**: 上記コメントフィルタを通過したPR
- **レビュー依頼**: review-requested:@me のopen PR

## 既読管理の原則

- 既読は **コメントID単位**(PR単位ではない)。同一PRに新しい依頼コメントが来たら再浮上する
- レビュー依頼はPR + review request単位
- SQLiteスキーマ変更時はマイグレーションを書く(NAS上のDBを壊さない)

## キーバインド(MVP)

| キー | 動作 |
|---|---|
| j / k | 上下移動 |
| Tab | セクション切替 |
| Enter | ブラウザでPRを開く |
| d | 対応済みマーク(コメントIDをSQLiteに記録) |
| r | 手動リフレッシュ |
| q | 終了 |

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
- 設定ファイルは `$XDG_CONFIG_HOME/ghbox/config.toml`(ポーリング間隔、DB パス、正規表現の追加パターン)
- コミットメッセージは英語、conventional commits

## 未確定事項(実装しながら決める)

- [ ] GraphQL search でコメント本文まで一発取得できるか、レート制限に収まるか(最初に素振りで確認)
- [ ] マージ依頼の検索対象を open のみにするか merged 含むか
- [ ] review thread 内コメント(PullRequestReviewComment)も対象にするか、issue comment のみか
- [ ] ポーリング間隔のデフォルト(案: 5分)
- [ ] NAS上のSQLiteパス
