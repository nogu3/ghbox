//! Direct open-PR enumeration for `Section.repos`, plus the batched
//! comment fetch used by the Fetcher's diff cache. Search-index-free:
//! results come straight from the repository, so nothing can be missed
//! by index lag.

use std::collections::HashMap;

use serde::Deserialize;

use crate::github::{
    Actor, CommentConnection, GqlError, PrData, ReviewRequestConnection, comment_infos,
    graphql_post,
};
use crate::item::{CommentInfo, PrState};
use crate::{Error, Result};

/// A PR to fetch comments for. `key()` matches `Item::pr_key` format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrRef {
    pub repo: String,
    pub number: u64,
}

impl PrRef {
    pub(crate) fn key(&self) -> String {
        format!("{}#{}", self.repo, self.number)
    }
}

const REPO_PAGE_QUERY: &str = "query($owner: String!, $name: String!, $cursor: String) {\n  viewer { login }\n  repository(owner: $owner, name: $name) {\n    pullRequests(states: OPEN, first: 100, after: $cursor, orderBy: {field: UPDATED_AT, direction: DESC}) {\n      pageInfo { hasNextPage endCursor }\n      nodes {\n        number\n        title\n        url\n        isDraft\n        updatedAt\n        createdAt\n        author { login }\n        reviewRequests(first: 20) { nodes { requestedReviewer { ... on User { login } } } }\n      }\n    }\n  }\n}";

#[derive(Deserialize)]
struct PageResponse {
    data: Option<PageData>,
    errors: Option<Vec<GqlError>>,
}

#[derive(Deserialize)]
struct PageData {
    viewer: Actor,
    repository: Option<RepoNode>,
}

#[derive(Deserialize)]
struct RepoNode {
    #[serde(rename = "pullRequests")]
    pull_requests: PrConnection,
}

#[derive(Deserialize)]
struct PrConnection {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    nodes: Vec<Option<RepoPrNode>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RepoPrNode {
    number: u64,
    title: String,
    url: String,
    is_draft: bool,
    updated_at: String,
    created_at: String,
    author: Option<Actor>,
    review_requests: ReviewRequestConnection,
}

#[derive(Debug)]
pub(crate) struct RepoPage {
    pub viewer_login: String,
    pub prs: Vec<PrData>,
    pub next_cursor: Option<String>,
    /// Partial GraphQL errors returned alongside usable data.
    pub errors: Vec<String>,
}

pub(crate) fn parse_repo_page(json: &str, repo: &str) -> Result<RepoPage> {
    let resp: PageResponse = serde_json::from_str(json)?;
    let errors: Vec<String> = resp
        .errors
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.message)
        .collect();
    let data = match resp.data {
        Some(data) => data,
        None => {
            let message = if errors.is_empty() {
                "response has neither data nor errors".into()
            } else {
                errors.join("; ")
            };
            return Err(Error::Api(message));
        }
    };
    let Some(repository) = data.repository else {
        return Err(Error::Api(format!("repo {repo}: not found or no access")));
    };
    let conn = repository.pull_requests;
    let next_cursor = if conn.page_info.has_next_page {
        conn.page_info.end_cursor
    } else {
        None
    };
    let prs = conn
        .nodes
        .into_iter()
        .flatten()
        .map(|node| PrData {
            repo: repo.to_string(),
            pr_number: node.number,
            pr_title: node.title,
            pr_url: node.url,
            pr_author: node
                .author
                .map(|a| a.login)
                .unwrap_or_else(|| "(unknown)".into()),
            pr_updated_at: node.updated_at,
            pr_created_at: node.created_at,
            // states: OPEN in the query — only the draft flag varies.
            state: if node.is_draft {
                PrState::Draft
            } else {
                PrState::Open
            },
            comments: Vec::new(),
            review_requested: node.review_requests.user_logins(),
            comments_fetched: false,
        })
        .collect();
    Ok(RepoPage {
        viewer_login: data.viewer.login,
        prs,
        next_cursor,
        errors,
    })
}

pub(crate) struct RepoFetchResult {
    pub viewer_login: String,
    pub prs: Vec<PrData>,
    pub errors: Vec<String>,
}

/// Enumerates every open PR of `repo` ("owner/name", validated at config
/// load) across pages.
pub(crate) async fn fetch_repo_open_prs(token: &str, repo: &str) -> Result<RepoFetchResult> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| Error::Api(format!("invalid repo {repo:?}")))?;
    let mut prs = Vec::new();
    let mut errors = Vec::new();
    let mut viewer_login: Option<String> = None;
    let mut cursor: Option<String> = None;
    loop {
        let body = serde_json::json!({
            "query": REPO_PAGE_QUERY,
            "variables": { "owner": owner, "name": name, "cursor": cursor },
        });
        let text = graphql_post(token, &body)
            .await
            .map_err(|e| Error::Api(format!("repo {repo}: {e}")))?;
        let page = parse_repo_page(&text, repo)?;
        if viewer_login.is_none() {
            viewer_login = Some(page.viewer_login);
        }
        prs.extend(page.prs);
        errors.extend(page.errors);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Ok(RepoFetchResult {
        viewer_login: viewer_login.unwrap_or_default(),
        prs,
        errors,
    })
}

/// Comment batches: ~25 aliased PRs per request keeps each request's node
/// budget small while a cold cache (a few hundred PRs) stays at ~15 requests.
const COMMENT_BATCH_SIZE: usize = 25;

/// Owner/name/number are inlined (not variables): repo strings are
/// charset-validated at config load, so quoting is safe, and per-item
/// aliases can't share one variable set anyway.
pub(crate) fn build_comments_query(refs: &[PrRef]) -> String {
    let mut q = String::from("query {\n");
    for (i, r) in refs.iter().enumerate() {
        let (owner, name) = r.repo.split_once('/').expect("validated at config load");
        q.push_str(&format!(
            "  p{i}: repository(owner: \"{owner}\", name: \"{name}\") {{ pullRequest(number: {}) {{ comments(last: 50) {{ nodes {{ databaseId author {{ login }} body createdAt }} }} }} }}\n",
            r.number
        ));
    }
    q.push('}');
    q
}

#[derive(Deserialize)]
struct CommentsResponse {
    data: Option<HashMap<String, Option<CommentsRepoNode>>>,
    errors: Option<Vec<GqlError>>,
}

#[derive(Deserialize)]
struct CommentsRepoNode {
    #[serde(rename = "pullRequest")]
    pull_request: Option<CommentsPrNode>,
}

#[derive(Deserialize)]
struct CommentsPrNode {
    comments: CommentConnection,
}

#[allow(clippy::type_complexity)]
pub(crate) fn parse_comments(
    json: &str,
    refs: &[PrRef],
) -> Result<(HashMap<String, Vec<CommentInfo>>, Vec<String>)> {
    let resp: CommentsResponse = serde_json::from_str(json)?;
    let errors: Vec<String> = resp
        .errors
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.message)
        .collect();
    let mut data = match resp.data {
        Some(data) => data,
        None => {
            let message = if errors.is_empty() {
                "response has neither data nor errors".into()
            } else {
                errors.join("; ")
            };
            return Err(Error::Api(message));
        }
    };
    let mut map = HashMap::new();
    for (i, r) in refs.iter().enumerate() {
        let Some(Some(node)) = data.remove(&format!("p{i}")) else {
            // Alias missing or nulled by a GraphQL error — leave the key
            // out entirely. The caller must treat this as an incomplete
            // fetch, not as "no comments": the PR may still exist and hold
            // a matching comment we simply failed to read this poll.
            continue;
        };
        match node.pull_request {
            Some(pr) => {
                map.insert(r.key(), comment_infos(pr.comments));
            }
            None => {
                // PR genuinely gone (closed/deleted) between enumeration
                // and this fetch — cache an empty comment list; it will
                // self-heal by leaving the enumeration on the next poll.
                map.insert(r.key(), Vec::new());
            }
        }
    }
    Ok((map, errors))
}

/// Folds per-chunk fetch results into one `(map, errors)`. A failed
/// chunk contributes no keys — absent keys already mean "incomplete
/// fetch, do not cache" to the caller — and surfaces its error message.
#[allow(clippy::type_complexity, dead_code)]
pub(crate) fn merge_chunk_results(
    results: Vec<Result<(HashMap<String, Vec<CommentInfo>>, Vec<String>)>>,
) -> (HashMap<String, Vec<CommentInfo>>, Vec<String>) {
    let mut map = HashMap::new();
    let mut errors = Vec::new();
    for result in results {
        match result {
            Ok((chunk_map, chunk_errors)) => {
                map.extend(chunk_map);
                errors.extend(chunk_errors);
            }
            Err(e) => errors.push(e.to_string()),
        }
    }
    (map, errors)
}

#[allow(clippy::type_complexity)]
pub(crate) async fn fetch_comments(
    token: &str,
    refs: &[PrRef],
) -> Result<(HashMap<String, Vec<CommentInfo>>, Vec<String>)> {
    let mut map = HashMap::new();
    let mut errors = Vec::new();
    for chunk in refs.chunks(COMMENT_BATCH_SIZE) {
        let body = serde_json::json!({ "query": build_comments_query(chunk) });
        let text = graphql_post(token, &body).await?;
        let (chunk_map, chunk_errors) = parse_comments(&text, chunk)?;
        map.extend(chunk_map);
        errors.extend(chunk_errors);
    }
    Ok((map, errors))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE_FIXTURE: &str = r#"{
      "data": {
        "viewer": { "login": "nogu3" },
        "repository": {
          "pullRequests": {
            "pageInfo": { "hasNextPage": true, "endCursor": "CUR1" },
            "nodes": [
              {
                "number": 7,
                "title": "Add thing",
                "url": "https://github.com/o/r/pull/7",
                "isDraft": true,
                "updatedAt": "2026-07-20T00:00:00Z",
                "createdAt": "2026-07-10T00:00:00Z",
                "author": { "login": "alice" },
                "reviewRequests": {
                  "nodes": [
                    { "requestedReviewer": { "login": "nogu3" } },
                    { "requestedReviewer": {} }
                  ]
                }
              },
              null,
              {
                "number": 8,
                "title": "Fix thing",
                "url": "https://github.com/o/r/pull/8",
                "isDraft": false,
                "updatedAt": "2026-07-21T00:00:00Z",
                "createdAt": "2026-07-11T00:00:00Z",
                "author": null,
                "reviewRequests": { "nodes": [] }
              }
            ]
          }
        }
      }
    }"#;

    #[test]
    fn parse_repo_page_maps_prs_and_cursor() {
        let page = parse_repo_page(PAGE_FIXTURE, "o/r").unwrap();
        assert_eq!(page.viewer_login, "nogu3");
        assert_eq!(page.next_cursor.as_deref(), Some("CUR1"));
        assert!(page.errors.is_empty());
        assert_eq!(page.prs.len(), 2); // null node skipped
        let pr = &page.prs[0];
        assert_eq!(pr.repo, "o/r");
        assert_eq!(pr.pr_number, 7);
        assert_eq!(pr.state, crate::item::PrState::Draft);
        assert_eq!(pr.review_requested, vec!["nogu3".to_string()]);
        assert!(!pr.comments_fetched);
        assert!(pr.comments.is_empty());
        assert_eq!(page.prs[1].pr_author, "(unknown)");
        assert_eq!(page.prs[1].state, crate::item::PrState::Open);
    }

    #[test]
    fn parse_repo_page_last_page_has_no_cursor() {
        let json = r#"{
          "data": {
            "viewer": { "login": "nogu3" },
            "repository": {
              "pullRequests": {
                "pageInfo": { "hasNextPage": false, "endCursor": "END" },
                "nodes": []
              }
            }
          }
        }"#;
        let page = parse_repo_page(json, "o/r").unwrap();
        assert_eq!(page.next_cursor, None, "no next page => no cursor");
    }

    #[test]
    fn parse_repo_page_null_repository_is_error_naming_the_repo() {
        let json = r#"{ "data": { "viewer": { "login": "nogu3" }, "repository": null } }"#;
        let err = parse_repo_page(json, "o/gone").unwrap_err();
        assert!(err.to_string().contains("o/gone"), "got: {err}");
    }

    #[test]
    fn parse_repo_page_null_data_with_errors_is_api_error() {
        let json = r#"{ "data": null, "errors": [ { "message": "rate limited" } ] }"#;
        let err = parse_repo_page(json, "o/r").unwrap_err();
        assert!(err.to_string().contains("rate limited"));
    }

    #[test]
    fn build_comments_query_aliases_each_ref() {
        let refs = vec![
            PrRef {
                repo: "o/r".into(),
                number: 7,
            },
            PrRef {
                repo: "x/y".into(),
                number: 12,
            },
        ];
        let q = build_comments_query(&refs);
        assert!(q.contains(r#"p0: repository(owner: "o", name: "r") { pullRequest(number: 7)"#));
        assert!(q.contains(r#"p1: repository(owner: "x", name: "y") { pullRequest(number: 12)"#));
        assert!(q.contains("comments(last: 50)"));
    }

    #[test]
    fn parse_comments_maps_aliases_to_keys_and_skips_null_prs() {
        let refs = vec![
            PrRef {
                repo: "o/r".into(),
                number: 7,
            },
            PrRef {
                repo: "o/r".into(),
                number: 8,
            },
            PrRef {
                repo: "o/r".into(),
                number: 9,
            },
        ];
        let json = r#"{
          "data": {
            "p0": {
              "pullRequest": {
                "comments": {
                  "nodes": [
                    {
                      "databaseId": 11,
                      "author": { "login": "bob" },
                      "body": "@nogu3 merge please",
                      "createdAt": "2026-07-20T00:00:00Z"
                    }
                  ]
                }
              }
            },
            "p1": { "pullRequest": null },
            "p2": null
          }
        }"#;
        let (map, errors) = parse_comments(json, &refs).unwrap();
        assert!(errors.is_empty());
        assert_eq!(map["o/r#7"].len(), 1);
        assert_eq!(map["o/r#7"][0].id, 11);
        // PR genuinely gone: an empty entry is cached (key present).
        assert_eq!(map["o/r#8"].len(), 0);
        // Alias entirely null (error-nulled): key must be absent, so the
        // fetcher treats this as an incomplete fetch rather than caching
        // a false "no comments".
        assert!(!map.contains_key("o/r#9"));
    }

    #[test]
    fn pr_ref_key_format_matches_pr_key() {
        let r = PrRef {
            repo: "o/r".into(),
            number: 7,
        };
        assert_eq!(r.key(), "o/r#7");
    }

    #[test]
    fn merge_chunk_results_merges_maps_and_partial_errors() {
        let r1: crate::Result<(HashMap<String, Vec<CommentInfo>>, Vec<String>)> = Ok((
            [("o/r#1".to_string(), vec![])].into_iter().collect(),
            vec![],
        ));
        let r2 = Ok((
            [("o/r#2".to_string(), vec![])].into_iter().collect(),
            vec!["partial err".to_string()],
        ));
        let (map, errors) = merge_chunk_results(vec![r1, r2]);
        assert!(map.contains_key("o/r#1"));
        assert!(map.contains_key("o/r#2"));
        assert_eq!(errors, vec!["partial err".to_string()]);
    }

    #[test]
    fn merge_chunk_results_failed_chunk_yields_no_keys_but_surfaces_error() {
        let ok: crate::Result<(HashMap<String, Vec<CommentInfo>>, Vec<String>)> = Ok((
            [("o/r#1".to_string(), vec![])].into_iter().collect(),
            vec![],
        ));
        let failed = Err(crate::Error::Api("boom".into()));
        let (map, errors) = merge_chunk_results(vec![ok, failed]);
        // 失敗チャンクのキーは不在のまま = 呼び出し側は「取得不完全」と
        // 解釈してキャッシュしない(既存セマンティクス)
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("o/r#1"));
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("boom"), "got: {}", errors[0]);
    }

    #[test]
    fn merge_chunk_results_accumulates_errors_across_failed_chunks() {
        let e1: crate::Result<(HashMap<String, Vec<CommentInfo>>, Vec<String>)> =
            Err(crate::Error::Api("first".into()));
        let e2 = Err(crate::Error::Api("second".into()));
        let (map, errors) = merge_chunk_results(vec![e1, e2]);
        assert!(map.is_empty());
        assert_eq!(errors.len(), 2);
    }
}
