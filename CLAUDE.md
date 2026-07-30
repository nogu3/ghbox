# ghbox

A TUI that keeps the PRs "where the ball is in your court" on GitHub visible at all times.
Lists merge requests (in-comment mentions) and review requests per repository, with
done-state (read) tracking.

## Why this exists

GitHub search cannot express "a single comment containing both @me and merge/マージ"
(`mentions:@me` and `in:comments` each match anywhere in the PR). This same-comment
filter is the core logic of this tool.

## Architecture

Follows the casa/casad pattern: core split into a lib, frontend swappable.

```
ghbox/
├── crates/
│   ├── ghbox-core/   # GraphQL fetch, comment filter, types, SQLite state
│   └── ghbox/        # ratatui TUI frontend
└── CLAUDE.md
```

Future: let mando call ghbox-core too (HTTP endpoint or direct dependency).

## Tech stack

- Rust (edition 2024)
- ratatui + crossterm
- tokio (periodic background fetch)
- reqwest + GitHub GraphQL API v4
- rusqlite (done state. DB lives on the NAS, synced across machines — same
  approach as schliemann-drill)
- Auth: output of `gh auth token`. No token management of our own

## Data flow

1. Build one GraphQL request for the sections with a `query`: each search aliased
   (s0, s1, ...) plus `viewer { login }`; comment-mention sections additionally
   request `comments(last: 50)`, review-requested sections request `reviewRequests`
   (search strings passed as variables)
2. Separately enumerate every unique repo named in a `repos` list: one paginated
   GraphQL request per repo, open PRs only — immune to search-index lag. The
   search request and the repo enumerations all run concurrently (repo requests
   capped at a small in-flight limit)
3. Per section, merge candidates = its search matches ∪ its repos' enumerated PRs,
   deduped by repo+PR number (the search entry wins, since it may already carry
   comments)
4. Comment-mention sections: PRs still missing comments (enumeration-only) are
   diff-fetched in a batch request. An in-memory cache keyed by PR `updatedAt`,
   held across polls, means steady-state polls only refetch PRs whose `updatedAt`
   moved
5. Apply the per-section filter: none / comment-mention (same comment body contains
   `@viewer` and `(?i)(merge|マージ)` or extra_patterns) / review-requested (open
   PRs where the viewer is still a requested *user* reviewer — team review
   requests are not matched) / command (pipe JSONL to an external command, read
   back the ids to keep)
6. Drop done items → sort repo asc, sort-key time desc → render as tabs + table

## Sections

Freely defined via `[[sections]]` in config.toml (title + source(s) + filter +
columns + sort). Without a config, two built-in default sections are used
(merge requests + review requests).

- Source: `query` (GitHub search string) and/or `repos` (`["owner/name", ...]`,
  direct open-PR enumeration); at least one is required, both = union deduped by
  PR. `repos` is immune to search-index lag — recommended for "ball is in your
  court" sections over known repos; keep `query` for broad discovery
- Filter types: none / `comment-mention` (same-comment mention+merge; the core
  logic; `include_own = true` also surfaces the viewer's own comments, for
  memos to self) / `review-requested` (open PRs where the viewer is still a
  requested reviewer; user reviewers only — team review requests are not
  matched, so keep a `query` source with `review-requested:@me` if you rely on
  those) / `command` (external command; JSONL on stdin, ids to keep
  on stdout)
- Columns: `state` / `repo` / `number` / `title` / `author` / `comment` /
  `updated` / `created`
- Sort: `updated` (default; PR last update) / `created` (comment creation for
  comment items, PR creation otherwise)

## Done-state principles

- Comment items: keyed **per comment ID** (kind=`merge_comment`). A new request
  comment on the same PR resurfaces it
- PR items: keyed **per PR + updatedAt** (kind=`pr`, upsert). A PR updated after
  being marked resurfaces
- Done state is global across sections (the key derives from the item itself)
- Write a migration for any SQLite schema change (append-only; never break the DB
  on the NAS). Refuse to start if the DB user_version is newer than the binary

## Keybindings (defaults, remappable via config)

| Key | Action | Behavior |
|---|---|---|
| ↓ / j | down | Move down |
| ↑ / k | up | Move up |
| → / l | next_section | Next section |
| ← / h | prev_section | Previous section |
| o | open | Open PR in browser |
| d | done | Mark as done |
| r | refresh | Manual refresh |
| q | quit | Quit |

## Development commands

```sh
cargo run -p ghbox          # launch the TUI
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all
```

## Conventions

- UNIX philosophy: ghbox-core has no side effects (no terminal drawing). The TUI
  does display and input only
- Errors: anyhow (binary) / thiserror (lib)
- GraphQL queries stay as string literals, not separate .graphql files (start small)
- Config file: `$XDG_CONFIG_HOME/ghbox/config.toml` (sections / theme / keybindings /
  poll interval / DB path. `deny_unknown_fields` catches typos; an invalid config
  fails fast at startup)
- Commit messages in English, conventional commits

## Open questions (decide while implementing)

- [ ] Can GraphQL search fetch comment bodies in one shot, and does it fit rate
      limits? (validate with a spike first)
- [ ] Search merge requests in open PRs only, or include merged?
- [ ] Include review thread comments (PullRequestReviewComment), or issue comments only?
- [ ] Default polling interval (proposal: 5 min)
- [ ] SQLite path on the NAS
