use crate::Result;
use crate::config::{Section, SectionFilter};
use crate::filter::{CommentFilter, run_command_filter};
use crate::github::PrData;
use crate::item::Item;
use crate::store::{KIND_MERGE_COMMENT, Store};

/// One section's rows, ready for display.
#[derive(Debug, Clone)]
pub struct SectionData {
    pub title: String,
    pub items: Vec<Item>,
}

/// Per-section result: Err carries a status-bar message and means the
/// frontend must keep showing the section's previous items (an empty
/// section would falsely read as "all clear").
pub type SectionResult = std::result::Result<SectionData, String>;

/// Per-section filtered items, before read-state exclusion. Err carries the
/// same status-bar message contract as [`SectionResult`].
pub type FilteredSection = std::result::Result<Vec<Item>, String>;

/// The async half of section building: apply each section's filter to the
/// fetched PRs. Takes no `Store`, so the returned future is `Send` — the TUI
/// relies on this to run the whole fetch on `tokio::spawn`, which the runtime
/// cancels cleanly if the user quits mid-fetch.
pub async fn filter_sections(
    sections: &[Section],
    outcome: &crate::fetcher::FetchOutcome,
) -> Result<Vec<FilteredSection>> {
    let mut out = Vec::with_capacity(sections.len());
    for (section, fetched_prs) in sections.iter().zip(&outcome.sections) {
        let prs = match fetched_prs {
            Ok(prs) => prs,
            Err(e) => {
                out.push(Err(format!("{}: {e}", section.title)));
                continue;
            }
        };
        let items = match &section.filter {
            SectionFilter::None => prs.iter().map(pr_item).collect(),
            SectionFilter::CommentMention {
                extra_patterns,
                include_own,
            } => {
                let filter = CommentFilter::new(&outcome.viewer_login, extra_patterns)?;
                comment_items(prs, &filter, &outcome.viewer_login, *include_own)
            }
            SectionFilter::ReviewRequested => prs
                .iter()
                .filter(|pr| {
                    pr.review_requested
                        .iter()
                        .any(|login| login == &outcome.viewer_login)
                })
                .map(pr_item)
                .collect(),
            SectionFilter::Command { command } => {
                let candidates: Vec<Item> = prs.iter().map(pr_item).collect();
                match run_command_filter(command, &candidates).await {
                    Ok(keep) => candidates
                        .into_iter()
                        .filter(|item| keep.contains(&item.stable_id()))
                        .collect(),
                    Err(e) => {
                        out.push(Err(format!("{}: {e}", section.title)));
                        continue;
                    }
                }
            }
        };
        out.push(Ok(items));
    }
    Ok(out)
}

/// The sync half: read-state exclusion → sort. `filtered` must be parallel
/// to `sections` (as produced by [`filter_sections`]).
pub fn finalize_sections(
    sections: &[Section],
    filtered: Vec<FilteredSection>,
    store: &Store,
) -> Result<Vec<SectionResult>> {
    let mut out = Vec::with_capacity(sections.len());
    for (section, items) in sections.iter().zip(filtered) {
        let items = match items {
            Ok(items) => items,
            Err(e) => {
                out.push(Err(e));
                continue;
            }
        };
        let mut items = exclude_done(items, store)?;
        items.sort_by(|a, b| {
            a.repo
                .cmp(&b.repo)
                .then_with(|| b.sort_time(section.sort).cmp(a.sort_time(section.sort)))
        });
        out.push(Ok(SectionData {
            title: section.title.clone(),
            items,
        }));
    }
    Ok(out)
}

/// filter → read-state exclusion → sort, per section. `outcome.sections`
/// must be parallel to `sections`. Holding `&Store` makes this future
/// `!Send`; callers that need `Send` use the two halves directly.
pub async fn build_sections(
    sections: &[Section],
    outcome: &crate::fetcher::FetchOutcome,
    store: &Store,
) -> Result<Vec<SectionResult>> {
    let filtered = filter_sections(sections, outcome).await?;
    finalize_sections(sections, filtered, store)
}

fn pr_item(pr: &PrData) -> Item {
    Item {
        repo: pr.repo.clone(),
        pr_number: pr.pr_number,
        pr_title: pr.pr_title.clone(),
        pr_url: pr.pr_url.clone(),
        pr_author: pr.pr_author.clone(),
        pr_updated_at: pr.pr_updated_at.clone(),
        pr_created_at: pr.pr_created_at.clone(),
        state: pr.state,
        comment: None,
    }
}

fn comment_items(
    prs: &[PrData],
    filter: &CommentFilter,
    viewer: &str,
    include_own: bool,
) -> Vec<Item> {
    let mut items = Vec::new();
    for pr in prs {
        for comment in &pr.comments {
            if !include_own && comment.author == viewer {
                continue; // own comments are not requests to me
            }
            if !filter.is_merge_request(&comment.body) {
                continue;
            }
            items.push(Item {
                comment: Some(comment.clone()),
                ..pr_item(pr)
            });
        }
    }
    items
}

pub(crate) fn exclude_done(items: Vec<Item>, store: &Store) -> Result<Vec<Item>> {
    let mut kept = Vec::new();
    for item in items {
        let done = match &item.comment {
            Some(c) => store.is_done(KIND_MERGE_COMMENT, &c.id.to_string())?,
            None => store.is_done_pr(&item.pr_key(), &item.pr_updated_at)?,
        };
        if !done {
            kept.push(item);
        }
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Column, Section, SectionFilter, SortKey};
    use crate::github::PrData;
    use crate::item::{CommentInfo, PrState};
    use crate::store::KIND_PR;

    fn section(filter: SectionFilter) -> Section {
        Section {
            title: "sec".into(),
            query: Some("q".into()),
            repos: vec![],
            columns: vec![Column::Repo],
            filter,
            sort: SortKey::default(),
        }
    }

    fn pr_data(repo: &str, number: u64, updated_at: &str, comments: Vec<CommentInfo>) -> PrData {
        PrData {
            repo: repo.into(),
            pr_number: number,
            pr_title: "t".into(),
            pr_url: "u".into(),
            pr_author: "author".into(),
            pr_updated_at: updated_at.into(),
            pr_created_at: "2026-01-01T00:00:00Z".into(),
            state: PrState::Open,
            comments,
            review_requested: vec![],
            comments_fetched: true,
        }
    }

    fn cinfo(id: i64, author: &str, body: &str, created_at: &str) -> CommentInfo {
        CommentInfo {
            id,
            author: author.into(),
            body: body.into(),
            created_at: created_at.into(),
        }
    }

    fn fetched(sections: Vec<Vec<PrData>>) -> crate::fetcher::FetchOutcome {
        crate::fetcher::FetchOutcome {
            viewer_login: "nogu3".into(),
            sections: sections.into_iter().map(Ok).collect(),
            errors: vec![],
        }
    }

    /// The TUI runs the fetch+filter future on `tokio::spawn`; if it ever
    /// becomes `!Send` again (e.g. a `&Store` held across an await), quitting
    /// mid-fetch regresses to a panic at runtime shutdown.
    #[test]
    fn filter_sections_future_is_send() {
        fn assert_send<F: Send>(_: &F) {}
        let sections = [section(SectionFilter::None)];
        let outcome = fetched(vec![vec![]]);
        let fut = filter_sections(&sections, &outcome);
        assert_send(&fut);
    }

    #[tokio::test]
    async fn none_filter_yields_pr_items() {
        let store = Store::open_in_memory().unwrap();
        let f = fetched(vec![vec![pr_data(
            "o/r",
            1,
            "2026-07-02T00:00:00Z",
            vec![],
        )]]);
        let results = build_sections(&[section(SectionFilter::None)], &f, &store)
            .await
            .unwrap();
        let data = results[0].as_ref().unwrap();
        assert_eq!(data.title, "sec");
        assert_eq!(data.items.len(), 1);
        assert!(data.items[0].comment.is_none());
        assert_eq!(data.items[0].stable_id(), "pr:o/r#1");
    }

    #[tokio::test]
    async fn comment_mention_emits_item_per_matching_comment() {
        let store = Store::open_in_memory().unwrap();
        let f = fetched(vec![vec![pr_data(
            "o/r",
            1,
            "2026-07-02T00:00:00Z",
            vec![
                cinfo(1, "bot", "@nogu3 merge please", "2026-01-01T00:00:00Z"),
                cinfo(2, "bot", "@nogu3 マージして", "2026-01-02T00:00:00Z"),
                cinfo(3, "bot", "just chatting", "2026-01-03T00:00:00Z"),
                cinfo(
                    4,
                    "nogu3",
                    "@nogu3 merge memo to self",
                    "2026-01-04T00:00:00Z",
                ),
            ],
        )]]);
        let results = build_sections(
            &[section(SectionFilter::CommentMention {
                extra_patterns: vec![],
                include_own: false,
            })],
            &f,
            &store,
        )
        .await
        .unwrap();
        let items = &results[0].as_ref().unwrap().items;
        // non-matching comment and the viewer's own comment are excluded
        let mut ids: Vec<i64> = items
            .iter()
            .map(|i| i.comment.as_ref().unwrap().id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec![1, 2]);
    }
    #[tokio::test]
    async fn include_own_surfaces_viewers_own_memo_comment() {
        let store = Store::open_in_memory().unwrap();
        let f = fetched(vec![vec![pr_data(
            "o/r",
            1,
            "2026-07-02T00:00:00Z",
            vec![
                cinfo(
                    1,
                    "nogu3",
                    "@nogu3 merge memo to self",
                    "2026-01-01T00:00:00Z",
                ),
                cinfo(2, "nogu3", "just chatting", "2026-01-02T00:00:00Z"),
                cinfo(3, "bot", "@nogu3 merge please", "2026-01-03T00:00:00Z"),
            ],
        )]]);
        let results = build_sections(
            &[section(SectionFilter::CommentMention {
                extra_patterns: vec![],
                include_own: true,
            })],
            &f,
            &store,
        )
        .await
        .unwrap();
        let items = &results[0].as_ref().unwrap().items;
        // own memo (1) and bot request (3) surface; non-matching own comment (2) does not
        let mut ids: Vec<i64> = items
            .iter()
            .map(|i| i.comment.as_ref().unwrap().id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec![1, 3]);
    }

    #[tokio::test]
    async fn done_comment_ids_are_excluded() {
        let store = Store::open_in_memory().unwrap();
        store.mark_done(KIND_MERGE_COMMENT, "1").unwrap();
        let f = fetched(vec![vec![pr_data(
            "o/r",
            1,
            "2026-07-02T00:00:00Z",
            vec![
                cinfo(1, "bot", "@nogu3 merge", "2026-01-01T00:00:00Z"),
                cinfo(2, "bot", "@nogu3 merge", "2026-01-02T00:00:00Z"),
            ],
        )]]);
        let results = build_sections(
            &[section(SectionFilter::CommentMention {
                extra_patterns: vec![],
                include_own: false,
            })],
            &f,
            &store,
        )
        .await
        .unwrap();
        let items = &results[0].as_ref().unwrap().items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].comment.as_ref().unwrap().id, 2);
    }

    #[tokio::test]
    async fn done_pr_items_resurface_after_update() {
        let store = Store::open_in_memory().unwrap();
        store.mark_done_pr("o/r#1", "2026-07-02T00:00:00Z").unwrap();
        // same updatedAt → still done
        let f1 = fetched(vec![vec![pr_data(
            "o/r",
            1,
            "2026-07-02T00:00:00Z",
            vec![],
        )]]);
        let r1 = build_sections(&[section(SectionFilter::None)], &f1, &store)
            .await
            .unwrap();
        assert!(r1[0].as_ref().unwrap().items.is_empty());
        // PR updated later → resurfaces
        let f2 = fetched(vec![vec![pr_data(
            "o/r",
            1,
            "2026-07-03T00:00:00Z",
            vec![],
        )]]);
        let r2 = build_sections(&[section(SectionFilter::None)], &f2, &store)
            .await
            .unwrap();
        assert_eq!(r2[0].as_ref().unwrap().items.len(), 1);
        // KIND_PR is what got recorded
        assert!(store.is_done(KIND_PR, "o/r#1").unwrap());
    }

    #[tokio::test]
    async fn command_filter_retains_listed_ids_and_ignores_unknown() {
        let store = Store::open_in_memory().unwrap();
        let f = fetched(vec![vec![
            pr_data("o/r", 1, "2026-07-02T00:00:00Z", vec![]),
            pr_data("o/r", 2, "2026-07-02T00:00:00Z", vec![]),
        ]]);
        // prints one known id and one bogus id; bogus matches nothing
        let results = build_sections(
            &[section(SectionFilter::Command {
                command: "printf 'pr:o/r#2\\npr:bogus#9\\n'".into(),
            })],
            &f,
            &store,
        )
        .await
        .unwrap();
        let items = &results[0].as_ref().unwrap().items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].pr_number, 2);
    }

    #[tokio::test]
    async fn command_filter_failure_is_per_section() {
        let store = Store::open_in_memory().unwrap();
        let f = fetched(vec![
            vec![pr_data("o/r", 1, "2026-07-02T00:00:00Z", vec![])],
            vec![pr_data("o/r", 2, "2026-07-02T00:00:00Z", vec![])],
        ]);
        let sections = [
            section(SectionFilter::Command {
                command: "exit 1".into(),
            }),
            section(SectionFilter::None),
        ];
        let results = build_sections(&sections, &f, &store).await.unwrap();
        // failed section carries an error message including its title
        let err = results[0].as_ref().unwrap_err();
        assert!(err.contains("sec"), "got: {err}");
        // the other section is unaffected
        assert_eq!(results[1].as_ref().unwrap().items.len(), 1);
    }

    /// Two PRs whose comment-creation order is the reverse of their PR-update
    /// order, so the two sort keys produce opposite orderings.
    fn crossed_comment_fetch() -> crate::fetcher::FetchOutcome {
        fetched(vec![vec![
            pr_data(
                "o/r",
                1,
                "2026-07-05T00:00:00Z",
                vec![cinfo(1, "bot", "@nogu3 merge", "2026-01-01T00:00:00Z")],
            ),
            pr_data(
                "o/r",
                2,
                "2026-07-01T00:00:00Z",
                vec![cinfo(2, "bot", "@nogu3 merge", "2026-01-10T00:00:00Z")],
            ),
        ]])
    }

    #[tokio::test]
    async fn comment_items_sort_by_pr_update_by_default() {
        let store = Store::open_in_memory().unwrap();
        let sections = [section(SectionFilter::CommentMention {
            extra_patterns: vec![],
            include_own: false,
        })];
        let results = build_sections(&sections, &crossed_comment_fetch(), &store)
            .await
            .unwrap();
        let numbers: Vec<u64> = results[0]
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|i| i.pr_number)
            .collect();
        assert_eq!(numbers, vec![1, 2]); // PR #1 updated most recently
    }

    #[tokio::test]
    async fn created_sort_key_orders_comment_items_by_comment_creation() {
        let store = Store::open_in_memory().unwrap();
        let sections = [Section {
            sort: SortKey::Created,
            ..section(SectionFilter::CommentMention {
                extra_patterns: vec![],
                include_own: false,
            })
        }];
        let results = build_sections(&sections, &crossed_comment_fetch(), &store)
            .await
            .unwrap();
        let numbers: Vec<u64> = results[0]
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|i| i.pr_number)
            .collect();
        assert_eq!(numbers, vec![2, 1]); // comment on PR #2 is newer
    }

    #[tokio::test]
    async fn items_sorted_by_repo_then_time_desc() {
        let store = Store::open_in_memory().unwrap();
        let f = fetched(vec![vec![
            pr_data("z/repo", 1, "2026-07-01T00:00:00Z", vec![]),
            pr_data("a/repo", 2, "2026-07-01T00:00:00Z", vec![]),
            pr_data("a/repo", 3, "2026-07-02T00:00:00Z", vec![]),
        ]]);
        let results = build_sections(&[section(SectionFilter::None)], &f, &store)
            .await
            .unwrap();
        let numbers: Vec<u64> = results[0]
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|i| i.pr_number)
            .collect();
        assert_eq!(numbers, vec![3, 2, 1]); // a/repo newest first, then z/repo
    }

    #[tokio::test]
    async fn review_requested_filter_keeps_only_viewer_requested_prs() {
        let store = Store::open_in_memory().unwrap();
        let mut requested = pr_data("o/r", 1, "2026-07-02T00:00:00Z", vec![]);
        requested.review_requested = vec!["nogu3".into()];
        let mut other = pr_data("o/r", 2, "2026-07-02T00:00:00Z", vec![]);
        other.review_requested = vec!["alice".into()];
        let nobody = pr_data("o/r", 3, "2026-07-02T00:00:00Z", vec![]);
        let f = fetched(vec![vec![requested, other, nobody]]);
        let results = build_sections(&[section(SectionFilter::ReviewRequested)], &f, &store)
            .await
            .unwrap();
        let items = &results[0].as_ref().unwrap().items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].pr_number, 1);
        assert!(items[0].comment.is_none(), "PR item, not comment item");
    }

    #[tokio::test]
    async fn fetch_error_section_passes_through_with_title() {
        let store = Store::open_in_memory().unwrap();
        let outcome = crate::fetcher::FetchOutcome {
            viewer_login: "nogu3".into(),
            sections: vec![Err("repo o/gone: not found or no access".into())],
            errors: vec![],
        };
        let results = build_sections(&[section(SectionFilter::None)], &outcome, &store)
            .await
            .unwrap();
        let err = results[0].as_ref().unwrap_err();
        assert!(err.contains("sec") && err.contains("o/gone"), "got: {err}");
    }
}
