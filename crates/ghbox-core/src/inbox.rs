use crate::Result;
use crate::filter::CommentFilter;
use crate::github::Parsed;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
