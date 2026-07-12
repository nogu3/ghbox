use crate::Result;
use crate::config::{Section, SectionFilter};
use crate::filter::{CommentFilter, run_command_filter};
use crate::github::{Fetched, Parsed, PrData};
use crate::item::Item;
use crate::store::{KIND_MERGE_COMMENT, KIND_REVIEW_REQUEST, Store};
use crate::types::{MergeRequest, ReviewRequest};

/// What the TUI displays. Items are sorted by repo (asc), then created_at
/// (desc) within a repo, so the frontend can insert repo headers where the
/// repo name changes between consecutive items.
#[derive(Debug, Default)]
pub struct Inbox {
    pub merge_requests: Vec<MergeRequest>,
    pub review_requests: Vec<ReviewRequest>,
}

pub fn build_inbox(parsed: &Parsed, filter: &CommentFilter, store: &Store) -> Result<Inbox> {
    let mut merge_requests = Vec::new();
    for comment in &parsed.comments {
        if comment.author == parsed.viewer_login {
            continue; // own comments are not requests to me
        }
        if !filter.is_merge_request(&comment.body) {
            continue;
        }
        if store.is_done(KIND_MERGE_COMMENT, &comment.comment_id.to_string())? {
            continue;
        }
        merge_requests.push(comment.clone());
    }
    merge_requests.sort_by(|a, b| {
        a.repo
            .cmp(&b.repo)
            .then_with(|| b.created_at.cmp(&a.created_at))
    });

    let mut review_requests = Vec::new();
    for rr in &parsed.review_requests {
        if store.is_done(KIND_REVIEW_REQUEST, &rr.key())? {
            continue;
        }
        review_requests.push(rr.clone());
    }
    review_requests.sort_by(|a, b| {
        a.repo
            .cmp(&b.repo)
            .then_with(|| b.created_at.cmp(&a.created_at))
    });

    Ok(Inbox {
        merge_requests,
        review_requests,
    })
}

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

/// filter → read-state exclusion → sort, per section. `fetched.sections`
/// must be parallel to `sections`.
pub async fn build_sections(
    sections: &[Section],
    fetched: &Fetched,
    store: &Store,
) -> Result<Vec<SectionResult>> {
    let mut out = Vec::with_capacity(sections.len());
    for (section, prs) in sections.iter().zip(&fetched.sections) {
        let items = match &section.filter {
            SectionFilter::None => prs.iter().map(pr_item).collect(),
            SectionFilter::CommentMention { extra_patterns } => {
                let filter = CommentFilter::new(&fetched.viewer_login, extra_patterns)?;
                comment_items(prs, &filter, &fetched.viewer_login)
            }
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
        let mut items = exclude_done(items, store)?;
        items.sort_by(|a, b| {
            a.repo
                .cmp(&b.repo)
                .then_with(|| b.sort_time().cmp(a.sort_time()))
        });
        out.push(Ok(SectionData {
            title: section.title.clone(),
            items,
        }));
    }
    Ok(out)
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
        comment: None,
    }
}

fn comment_items(prs: &[PrData], filter: &CommentFilter, viewer: &str) -> Vec<Item> {
    let mut items = Vec::new();
    for pr in prs {
        for comment in &pr.comments {
            if comment.author == viewer {
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

fn exclude_done(items: Vec<Item>, store: &Store) -> Result<Vec<Item>> {
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
    use crate::config::{Column, Section, SectionFilter};
    use crate::github::{Fetched, PrData};
    use crate::item::CommentInfo;
    use crate::store::KIND_PR;

    fn comment(id: i64, repo: &str, author: &str, body: &str, at: &str) -> MergeRequest {
        MergeRequest {
            comment_id: id,
            repo: repo.into(),
            pr_number: 1,
            pr_title: "t".into(),
            pr_url: "u".into(),
            author: author.into(),
            body: body.into(),
            created_at: at.into(),
        }
    }

    fn review(repo: &str, number: u64) -> ReviewRequest {
        ReviewRequest {
            repo: repo.into(),
            pr_number: number,
            pr_title: "t".into(),
            pr_url: "u".into(),
            author: "a".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn parsed(comments: Vec<MergeRequest>, reviews: Vec<ReviewRequest>) -> Parsed {
        Parsed {
            viewer_login: "nogu3".into(),
            comments,
            review_requests: reviews,
        }
    }

    fn setup() -> (CommentFilter, Store) {
        (
            CommentFilter::new("nogu3", &[]).unwrap(),
            Store::open_in_memory().unwrap(),
        )
    }

    #[test]
    fn keeps_only_matching_comments() {
        let (filter, store) = setup();
        let p = parsed(
            vec![
                comment(
                    1,
                    "o/r",
                    "bot",
                    "@nogu3 merge please",
                    "2026-01-01T00:00:00Z",
                ),
                comment(2, "o/r", "bot", "just chatting", "2026-01-02T00:00:00Z"),
            ],
            vec![],
        );
        let inbox = build_inbox(&p, &filter, &store).unwrap();
        assert_eq!(inbox.merge_requests.len(), 1);
        assert_eq!(inbox.merge_requests[0].comment_id, 1);
    }

    #[test]
    fn excludes_viewer_own_comments() {
        let (filter, store) = setup();
        let p = parsed(
            vec![comment(
                1,
                "o/r",
                "nogu3",
                "@nogu3 merge memo to self",
                "2026-01-01T00:00:00Z",
            )],
            vec![],
        );
        let inbox = build_inbox(&p, &filter, &store).unwrap();
        assert!(inbox.merge_requests.is_empty());
    }

    #[test]
    fn excludes_done_comment_ids() {
        let (filter, store) = setup();
        store.mark_done(KIND_MERGE_COMMENT, "1").unwrap();
        let p = parsed(
            vec![
                comment(1, "o/r", "bot", "@nogu3 merge", "2026-01-01T00:00:00Z"),
                comment(2, "o/r", "bot", "@nogu3 マージして", "2026-01-02T00:00:00Z"),
            ],
            vec![],
        );
        let inbox = build_inbox(&p, &filter, &store).unwrap();
        assert_eq!(inbox.merge_requests.len(), 1);
        assert_eq!(inbox.merge_requests[0].comment_id, 2);
    }

    #[test]
    fn excludes_done_review_requests() {
        let (filter, store) = setup();
        store.mark_done(KIND_REVIEW_REQUEST, "o/r#1").unwrap();
        let p = parsed(vec![], vec![review("o/r", 1), review("o/r", 2)]);
        let inbox = build_inbox(&p, &filter, &store).unwrap();
        assert_eq!(inbox.review_requests.len(), 1);
        assert_eq!(inbox.review_requests[0].pr_number, 2);
    }

    #[test]
    fn sorts_by_repo_then_newest_first() {
        let (filter, store) = setup();
        let p = parsed(
            vec![
                comment(1, "z/repo", "bot", "@nogu3 merge", "2026-01-01T00:00:00Z"),
                comment(2, "a/repo", "bot", "@nogu3 merge", "2026-01-01T00:00:00Z"),
                comment(3, "a/repo", "bot", "@nogu3 merge", "2026-01-02T00:00:00Z"),
            ],
            vec![],
        );
        let inbox = build_inbox(&p, &filter, &store).unwrap();
        let ids: Vec<i64> = inbox.merge_requests.iter().map(|m| m.comment_id).collect();
        assert_eq!(ids, vec![3, 2, 1]); // a/repo newest first, then z/repo
    }

    fn section(filter: SectionFilter) -> Section {
        Section {
            title: "sec".into(),
            query: "q".into(),
            columns: vec![Column::Repo],
            filter,
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
            comments,
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

    fn fetched(sections: Vec<Vec<PrData>>) -> Fetched {
        Fetched {
            viewer_login: "nogu3".into(),
            sections,
        }
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
}
