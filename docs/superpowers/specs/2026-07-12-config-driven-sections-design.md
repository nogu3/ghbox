# ghbox: config 駆動セクション + gh-dash 風 UI 設計

日付: 2026-07-12
ステータス: 承認待ち(実装は別セッション)

## 目的

ghbox を「固定2セクションのマージ依頼ビューア」から「**config でセクションを自由定義できる PR inbox TUI**」に拡張する。gh-dash の思想(YAML でセクション定義)を TOML + Rust で踏襲しつつ、本ツール固有の価値である**同一コメント内 mention+merge フィルタ**と**既読(対応済み)管理**を組み込みフィルタとして提供する。OSS として公開する前提。

## スコープ

含む:

- セクション完全自由定義(タイトル + GitHub 検索クエリ + フィルタ)
- フィルタ種別: なし / comment-mention(組み込みコアロジック) / command(外部コマンド)
- gh-dash 風レイアウト: 上部タブバー + 全面テーブル + 下部ステータスバー
- カラムをセクションごとに config 指定
- テーマ(配色)の config 化
- キーバインドの config 化(リマップのみ)
- 既読の一般化: コメントアイテム=コメントID単位、PRアイテム=PR+updatedAt 単位(更新で再浮上)

含まない(将来拡張):

- プレビューペイン
- キーへのカスタムコマンド割当(テンプレート展開付きシェル実行)
- PullRequestReviewComment(review thread 内コメント)の取得
- ソート順の config 化

## Config スキーマ

`$XDG_CONFIG_HOME/ghbox/config.toml`。ファイルがなければ組み込みデフォルト(現行の2セクションを再現)で動作する。**config なしで即使える**ことを維持する。

```toml
poll_interval_secs = 300            # 最小 30 にクランプ(現行踏襲)
db_path = "/nas/ghbox/state.db"     # 省略時 $XDG_DATA_HOME/ghbox/state.db

[[sections]]
title = "マージ依頼"
query = "is:pr is:open mentions:@me"
columns = ["repo", "number", "title", "author", "comment"]
filter = { type = "comment-mention", extra_patterns = [] }

[[sections]]
title = "レビュー依頼"
query = "is:pr is:open review-requested:@me"
# filter 省略 = 検索結果そのまま。columns 省略 = デフォルトカラム

[[sections]]
title = "自分が関わるPR"
query = "is:pr is:open involves:@me"
filter = { type = "command", command = "~/.config/ghbox/filters/mine.sh" }

[theme]
tab_active = "yellow"
tab_inactive = "darkgray"
border = "darkgray"
selection_bg = "blue"
selection_fg = "white"
table_header = "cyan"
status_bar = "darkgray"
# 値は ratatui 名前付き色(小文字) または "#rrggbb"

[keybindings]
up = "k"
down = "j"
next_section = "tab"
prev_section = "backtab"
open = "enter"
done = "d"
refresh = "r"
quit = "q"
# 値: 1文字 or 特殊キー名 (tab, backtab, enter, up, down, esc)
```

設計判断:

- **`deny_unknown_fields` 維持**(typo 検出)。既存キー `extra_patterns`(トップレベル)は廃止し、comment-mention フィルタの `extra_patterns` に移動する。**破壊的変更だが未リリースのため許容**。
- `sections` キー**省略時**はデフォルトの2セクション(マージ依頼 + レビュー依頼)。明示的に `sections = []` と書いた場合は config エラー(表示するものがない)。
- セクションに `id` は持たせない。既読キーはアイテム自体から導出され(後述)、セクション横断でグローバル。タイトル変更・セクション並べ替えで既読が失われない。
- 矢印キー(Up/Down)は keybindings とは独立に常時有効(現行踏襲)。
- theme / keybindings は部分指定可。省略キーはデフォルト値。
- キーバインドの重複割当は config エラー(起動時に検出して報告)。

## アイテムモデル

セクションの中身を単一の `Item` 型に統一する。

```rust
pub struct Item {
    pub repo: String,          // nameWithOwner
    pub pr_number: u64,
    pub pr_title: String,
    pub pr_url: String,
    pub pr_author: String,
    pub pr_updated_at: String, // ISO8601
    pub pr_created_at: String,
    /// comment-mention フィルタ産のアイテムのみ Some
    pub comment: Option<CommentInfo>,
}

pub struct CommentInfo {
    pub id: i64,              // databaseId
    pub author: String,
    pub body: String,
    pub created_at: String,
}
```

- 既存の `MergeRequest` / `ReviewRequest` はこの `Item` に統合して廃止。
- 安定 ID: コメントアイテム = `comment:<databaseId>`、PRアイテム = `pr:<repo>#<number>`。外部コマンドフィルタと既読キーの両方で使う。
- serde `Serialize` を実装(外部コマンドへの JSON 出力用)。

## データフロー

```
config.sections
  → GraphQL クエリ動的組み立て(セクションごとに search を alias: s0, s1, ...)
  → 1リクエストで全セクション分 fetch
  → セクションごとにフィルタ適用(none / comment-mention / command)
  → 既読除外
  → ソート(repo 昇順 → 時刻降順。コメントアイテムは created_at、PRアイテムは updated_at)
  → SectionData { title, items } の Vec を TUI に渡す
```

### GraphQL 動的クエリ

- セクションごとに `search(query: $qN, type: ISSUE, first: 50)` を alias で並べ、`viewer { login }` と合わせて**1リクエスト**にまとめる。検索文字列は GraphQL variables で渡す(エスケープ問題を回避)。
- 全 PR ノードで共通取得: `number, title, url, updatedAt, createdAt, author { login }, repository { nameWithOwner }`。
- **comment-mention フィルタを持つセクションのみ** `comments(last: 50) { databaseId, author { login }, body, createdAt }` を追加する(不要なデータを取らない)。
- 同一 PR が複数セクションにヒットするのは正常(セクションごとに独立したアイテムになる)。
- レート制限: 現行2 search で cost=1 を確認済み。セクション数 N でも search 1個 ≈ 1 point なので実用範囲。実装時に素振りで再確認する。
- `first: 50` / `comments(last: 50)` の暗黙上限は現行踏襲(既知の制約として残す)。

### フィルタ

```rust
pub enum SectionFilter {
    None,
    CommentMention { extra_patterns: Vec<String> },
    Command { command: String },
}
```

**comment-mention**: 現行 `CommentFilter` のロジックをそのまま使う。PR の各コメントを走査し、同一コメント本文内に `@viewer` mention と `(?i)(merge|マージ)`(または extra_patterns)の両方を含むものを**コメントアイテム**として出力する。viewer 自身のコメントは除外(現行踏襲)。1 PR から複数アイテムが出うる。

**command**: PRアイテムの JSONL(1行1アイテム、`id` フィールドを含む全フィールド)を子プロセスの stdin に流し、stdout から**残すアイテムの `id`(1行1個、プレーンテキスト)**を受け取る。

- 実行: `sh -c <command>`。`~` はシェルが展開する。
- 1回の poll につきセクションごとに1プロセス(バッチ実行。アイテムごとに起動しない)。
- タイムアウト 10 秒。
- 失敗時(非ゼロ exit / タイムアウト / 不正な出力): そのセクションは**前回の表示を維持**し、ステータスバーにエラーを表示する。空になって「対応漏れゼロ」と誤認させない。
- stdout に含まれる未知の id は無視。
- 例(自分がアサインされた PR だけ残す): `jq -r 'select(.pr_author != "nogu3") | .id'`

**フィルタは1セクション1つ**。合成が必要なら command フィルタ内で任意のロジックを書ける。

## 既読(対応済み)管理

### 単位

| アイテム種別 | kind | key | 再浮上条件 |
|---|---|---|---|
| コメントアイテム | `merge_comment`(現行のまま) | コメント databaseId | しない(新コメントは別 ID で浮上) |
| PRアイテム | `pr`(新設) | `repo#number` | PR の `updatedAt` > 記録時の `updatedAt` |

- 既読は**セクション横断でグローバル**。同じ PR が2つのセクションに出ている場合、片方で `d` すれば両方から消える(コメントアイテムと PRアイテムは kind が違うので独立)。「ボールを処理した」という意味論に合わせる。
- 現行の `review_request` kind は廃止し、PRアイテムの `pr` kind に統合する(レビュー依頼が来れば updatedAt が動くので、request 単位の再浮上と実質同等)。

### SQLite マイグレーション(append-only、NAS 共有 DB を壊さない)

MIGRATIONS に追記する v2:

```sql
ALTER TABLE done_items ADD COLUMN updated_at TEXT;
INSERT OR IGNORE INTO done_items (kind, key, done_at, updated_at)
  SELECT 'pr', key, done_at, strftime('%Y-%m-%dT%H:%M:%SZ', done_at)
  FROM done_items WHERE kind = 'review_request';
```

- 既存の `review_request` 行は `pr` kind へコピーする。`updated_at` には done_at を ISO8601(T/Z 形式、GitHub の updatedAt と辞書順比較可能)で変換して入れる。「マーク時点より後に更新されたら再浮上」という新しい意味論と自然に整合する。
- 旧 `review_request` 行は**削除しない**(古いバイナリが同じ NAS DB を開いても壊れない)。
- `mark_done` は PRアイテムでは upsert にする(再浮上 → 再度 `d` で updated_at を更新)。記録する値は**そのアイテムの `pr_updated_at`**(マーク時刻ではない):
  `INSERT ... ON CONFLICT(kind, key) DO UPDATE SET done_at = excluded.done_at, updated_at = excluded.updated_at`
- `is_done(pr)` 判定: 行が存在し、かつ `記録 updated_at >= アイテムの updatedAt`。
- 既知課題(user_version が MIGRATIONS.len() より大きい場合のガードなし)はこの変更で顕在化しやすくなるため、**このタイミングでガードを追加する**: DB の user_version がバイナリの知る最大値を超えていたらエラーで起動拒否。

## TUI

### レイアウト

```
┌──────────────────────────────────────────────┐
│ [マージ依頼 3] レビュー依頼 2  自分が関わるPR 5  │  ← タブバー(1行)
├──────────────────────────────────────────────┤
│ repo         #     title            author    │  ← テーブルヘッダ
│ nogu3/casa   #12   Fix xxx          @bob      │
│ nogu3/casa   #15   Add yyy          @alice    │  ← 選択行はハイライト
│ acme/api     #301  Bump zzz         @carol    │
│                                              │
├──────────────────────────────────────────────┤
│ updated 12:34:56 | Tab:切替 j/k:移動 d:対応済 …│  ← ステータスバー(1行)
└──────────────────────────────────────────────┘
```

- タブバー: 全セクションのタイトル + 件数。アクティブタブは `theme.tab_active` でハイライト。表示は常に1セクション分のテーブルのみ。
- テーブル: ratatui の `Table` ウィジェット。現行の「リポジトリ見出し行」方式は廃止し、repo はカラムになる(ソートで repo ごとに固まるためグルーピングの視認性は保たれる)。
- 現行の `ui.rs` の List ベース実装は Table ベースに書き換え。

### カラム

利用可能なカラム名:

| 名前 | 内容 | デフォルト幅 |
|---|---|---|
| `repo` | nameWithOwner | 内容に応じ最大 30 |
| `number` | `#<pr_number>` | 6 |
| `title` | PR タイトル | 残り全部(伸縮) |
| `author` | コメントアイテムはコメント author、PRアイテムは PR author | 12 |
| `comment` | コメント本文の1行目(PRアイテムでは空) | 30 |
| `updated` | pr_updated_at(`MM-DD hh:mm` 表示) | 11 |
| `created` | コメントアイテムは comment.created_at、PRアイテムは pr_created_at | 11 |

- config は文字列配列のみ(`columns = ["repo", "number", "title"]`)。幅指定は入れない(YAGNI。伸縮は title が担う)。
- デフォルト(columns 省略時): `["repo", "number", "title", "author", "updated"]`
- 未知のカラム名は config エラー。

### キーバインド

アクション一覧(すべて config でリマップ可):

| アクション | デフォルト | 動作 |
|---|---|---|
| `up` / `down` | k / j | 行移動(矢印キーは常時有効) |
| `next_section` / `prev_section` | Tab / BackTab | セクション巡回 |
| `open` | Enter | ブラウザで PR を開く |
| `done` | d | 対応済みマーク |
| `refresh` | r | 手動リフレッシュ |
| `quit` | q | 終了 |

### テーマ

`[theme]` の7キー(前述)。パーサは ratatui の名前付き色(`"yellow"` 等、小文字)と `"#rrggbb"` を受け付ける。不正な値は config エラー。

## モジュール構成の変更

```
ghbox-core/
├── config.rs   # Section, SectionFilter, Theme, Keybindings を追加。デフォルト config 定義
├── github.rs   # 動的クエリビルダ + 汎用パース(Vec<Vec<Item>> を返す)
├── filter.rs   # CommentFilter(現行維持)+ command フィルタ実行
├── item.rs     # Item, CommentInfo(types.rs を置換)
├── inbox.rs    # セクションごとの filter → 既読除外 → ソート。Vec<SectionData> を返す
├── store.rs    # v2 マイグレーション、pr kind の upsert / updatedAt 比較、version ガード
└── error.rs

ghbox/
├── main.rs     # keybindings 解決、イベントループ(構造は現行踏襲)
├── app.rs      # Section enum を廃止し Vec<SectionData> + アクティブ index に
└── ui.rs       # タブバー + Table + テーマ適用に書き換え
```

- ghbox-core は引き続き端末描画を持たない。theme / keybindings の**型と検証**は core(config の一部)、**描画への適用**は frontend。
- command フィルタの実行(プロセス起動)は core に置く(tokio::process)。「副作用=端末描画を持たない」の原則には反しない。

## エラーハンドリング

- config エラー(typo、不正な正規表現・色・キー名、キー重複、セクション0個): **起動時に検出して即エラー終了**(メッセージに該当箇所を含める)。実行中の挙動不明より起動失敗が親切。
- fetch エラー: 現行踏襲(ステータスバー表示、前回表示維持、partial data 許容)。
- command フィルタ失敗: 該当セクションのみ前回表示維持 + ステータスバーにエラー。
- DB バージョンが新しすぎる: 起動拒否(前述)。

## テスト方針

- config: セクション配列 / theme / keybindings のパース、部分指定デフォルト、各種エラーケース(未知カラム、不正色、キー重複)
- github: 動的クエリビルダの出力(N セクション、comments 有無)、汎用パース
- filter: comment-mention は既存テスト維持。command フィルタは `sh -c` で jq 不要の fake コマンド(`grep` / `head` 等)を使い、正常系・非ゼロ exit・タイムアウト・不正 id を検証
- store: v1 DB を実際に作って v2 マイグレーション(review_request → pr コピー、既存 merge_comment 維持)、pr の updatedAt 再浮上、upsert、version ガード
- app: N セクションの巡回、選択クランプ(既存テストを Vec ベースに書き換え)

## 移行と後続作業

- CLAUDE.md の「セクション」「キーバインド」「設定ファイル」節をこの設計に合わせて更新する(実装計画に含める)。
- README の config 例を更新する。
- 実装は別セッションで writing-plans → subagent-driven で行う。
