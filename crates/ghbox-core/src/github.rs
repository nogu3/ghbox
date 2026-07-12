use std::collections::HashMap;

use serde::Deserialize;

use crate::config::{Section, SectionFilter};
use crate::item::CommentInfo;
use crate::types::{MergeRequest, ReviewRequest};
use crate::{Error, Result};

/// Single request fetching both sections. Verified 2026-07-11: cost = 1
/// rate-limit point per call.
const QUERY: &str = r#"
query {
  viewer { login }
  merge: search(query: "is:pr is:open mentions:@me", type: ISSUE, first: 50) {
    nodes {
      ... on PullRequest {
        number
        title
        url
        repository { nameWithOwner }
        comments(last: 50) {
          nodes {
            databaseId
            author { login }
            body
            createdAt
          }
        }
      }
    }
  }
  review: search(query: "is:pr is:open review-requested:@me", type: ISSUE, first: 50) {
    nodes {
      ... on PullRequest {
        number
        title
        url
        createdAt
        author { login }
        repository { nameWithOwner }
      }
    }
  }
}
"#;

#[derive(Debug)]
pub struct Parsed {
    pub viewer_login: String,
    /// All comment candidates (pre-filter). Same shape as MergeRequest.
    pub comments: Vec<MergeRequest>,
    pub review_requests: Vec<ReviewRequest>,
}

#[derive(Deserialize)]
struct GqlResponse {
    data: Option<Data>,
    errors: Option<Vec<GqlError>>,
}

#[derive(Deserialize)]
struct GqlError {
    message: String,
}

#[derive(Deserialize)]
struct Data {
    viewer: Actor,
    merge: Search<MergePr>,
    review: Search<ReviewPr>,
}

#[derive(Deserialize)]
struct Actor {
    login: String,
}

#[derive(Deserialize)]
struct Search<T> {
    nodes: Vec<Option<T>>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct MergePr {
    number: u64,
    title: String,
    url: String,
    repository: Option<Repo>,
    comments: CommentConnection,
}

#[derive(Deserialize)]
struct Repo {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Deserialize, Default)]
struct CommentConnection {
    nodes: Vec<Option<Comment>>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Comment {
    database_id: Option<i64>,
    author: Option<Actor>,
    body: String,
    created_at: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ReviewPr {
    number: u64,
    title: String,
    url: String,
    created_at: String,
    author: Option<Actor>,
    repository: Option<Repo>,
}

pub fn get_token() -> Result<String> {
    let output = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .map_err(|e| Error::Token(e.to_string()))?;
    if !output.status.success() {
        return Err(Error::Token(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err(Error::Token("`gh auth token` returned empty output".into()));
    }
    Ok(token)
}

pub async fn fetch(token: &str) -> Result<Parsed> {
    let client = reqwest::Client::new();
    let response = client
        .post("https://api.github.com/graphql")
        .bearer_auth(token)
        .header("User-Agent", "ghbox")
        .json(&serde_json::json!({ "query": QUERY }))
        .send()
        .await?
        .error_for_status()?;
    let text = response.text().await?;
    parse_response(&text)
}

pub fn parse_response(json: &str) -> Result<Parsed> {
    let resp: GqlResponse = serde_json::from_str(json)?;
    // GitHub may return HTTP 200 with both `errors` and usable `data` (e.g. one
    // SAML-protected org node is FORBIDDEN while the rest of the query succeeds).
    // Prefer partial data over failing the whole fetch; only treat `errors` as
    // fatal when there is no data to fall back on.
    let data = match resp.data {
        Some(data) => data,
        None => {
            let messages: Vec<String> = resp
                .errors
                .unwrap_or_default()
                .into_iter()
                .map(|e| e.message)
                .collect();
            let message = if messages.is_empty() {
                "response has neither data nor errors".into()
            } else {
                messages.join("; ")
            };
            return Err(Error::Api(message));
        }
    };

    let mut comments = Vec::new();
    for pr in data.merge.nodes.into_iter().flatten() {
        let Some(repo) = pr.repository else { continue };
        for comment in pr.comments.nodes.into_iter().flatten() {
            let Some(comment_id) = comment.database_id else {
                continue;
            };
            comments.push(MergeRequest {
                comment_id,
                repo: repo.name_with_owner.clone(),
                pr_number: pr.number,
                pr_title: pr.title.clone(),
                pr_url: pr.url.clone(),
                author: comment
                    .author
                    .map(|a| a.login)
                    .unwrap_or_else(|| "(unknown)".into()),
                body: comment.body,
                created_at: comment.created_at,
            });
        }
    }

    let mut review_requests = Vec::new();
    for pr in data.review.nodes.into_iter().flatten() {
        let Some(repo) = pr.repository else { continue };
        review_requests.push(ReviewRequest {
            repo: repo.name_with_owner,
            pr_number: pr.number,
            pr_title: pr.title,
            pr_url: pr.url,
            author: pr
                .author
                .map(|a| a.login)
                .unwrap_or_else(|| "(unknown)".into()),
            created_at: pr.created_at,
        });
    }

    Ok(Parsed {
        viewer_login: data.viewer.login,
        comments,
        review_requests,
    })
}

/// Result of one multi-section fetch. `sections` is parallel to the
/// `Section` slice passed to `build_query` / `fetch_sections`.
#[derive(Debug)]
pub struct Fetched {
    pub viewer_login: String,
    pub sections: Vec<Vec<PrData>>,
}

/// One PR as returned by search, before filtering.
#[derive(Debug, Clone)]
pub struct PrData {
    pub repo: String,
    pub pr_number: u64,
    pub pr_title: String,
    pub pr_url: String,
    pub pr_author: String,
    pub pr_updated_at: String,
    pub pr_created_at: String,
    /// Populated only for sections whose filter needs comment bodies.
    pub comments: Vec<CommentInfo>,
}

/// Builds one GraphQL request covering every section: `viewer` plus one
/// aliased `search` per section (s0, s1, ...). Search strings travel as
/// variables to avoid escaping issues. Comment bodies are requested only
/// for comment-mention sections. Verified 2026-07-11 with 2 searches:
/// cost = 1 rate-limit point per call.
pub fn build_query(sections: &[Section]) -> (String, serde_json::Value) {
    let mut query = String::from("query(");
    for i in 0..sections.len() {
        if i > 0 {
            query.push_str(", ");
        }
        query.push_str(&format!("$q{i}: String!"));
    }
    query.push_str(") {\n  viewer { login }\n");
    for (i, section) in sections.iter().enumerate() {
        let comments = if matches!(section.filter, SectionFilter::CommentMention { .. }) {
            "\n        comments(last: 50) { nodes { databaseId author { login } body createdAt } }"
        } else {
            ""
        };
        query.push_str(&format!(
            "  s{i}: search(query: $q{i}, type: ISSUE, first: 50) {{\n    nodes {{\n      ... on PullRequest {{\n        number\n        title\n        url\n        updatedAt\n        createdAt\n        author {{ login }}\n        repository {{ nameWithOwner }}{comments}\n      }}\n    }}\n  }}\n"
        ));
    }
    query.push('}');
    let variables: serde_json::Map<String, serde_json::Value> = sections
        .iter()
        .enumerate()
        .map(|(i, s)| (format!("q{i}"), serde_json::Value::String(s.query.clone())))
        .collect();
    (query, serde_json::Value::Object(variables))
}

#[derive(Deserialize)]
struct SectionsResponse {
    data: Option<SectionsData>,
    errors: Option<Vec<GqlError>>,
}

#[derive(Deserialize)]
struct SectionsData {
    viewer: Actor,
    #[serde(flatten)]
    searches: HashMap<String, Option<Search<PrNode>>>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct PrNode {
    number: u64,
    title: String,
    url: String,
    updated_at: String,
    created_at: String,
    author: Option<Actor>,
    repository: Option<Repo>,
    comments: CommentConnection,
}

pub async fn fetch_sections(token: &str, sections: &[Section]) -> Result<Fetched> {
    let (query, variables) = build_query(sections);
    let client = reqwest::Client::new();
    let response = client
        .post("https://api.github.com/graphql")
        .bearer_auth(token)
        .header("User-Agent", "ghbox")
        .json(&serde_json::json!({ "query": query, "variables": variables }))
        .send()
        .await?
        .error_for_status()?;
    let text = response.text().await?;
    parse_sections(&text, sections.len())
}

pub fn parse_sections(json: &str, section_count: usize) -> Result<Fetched> {
    let resp: SectionsResponse = serde_json::from_str(json)?;
    // GitHub may return HTTP 200 with both `errors` and usable `data` (e.g.
    // one SAML-protected org node is FORBIDDEN while the rest succeeds).
    // Prefer partial data; only treat `errors` as fatal without data.
    let mut data = match resp.data {
        Some(data) => data,
        None => {
            let messages: Vec<String> = resp
                .errors
                .unwrap_or_default()
                .into_iter()
                .map(|e| e.message)
                .collect();
            let message = if messages.is_empty() {
                "response has neither data nor errors".into()
            } else {
                messages.join("; ")
            };
            return Err(Error::Api(message));
        }
    };

    let mut sections = Vec::with_capacity(section_count);
    for i in 0..section_count {
        let search = data.searches.remove(&format!("s{i}")).flatten();
        let mut prs = Vec::new();
        for node in search
            .map(|s| s.nodes)
            .unwrap_or_default()
            .into_iter()
            .flatten()
        {
            let Some(repo) = node.repository else {
                continue;
            };
            let comments = node
                .comments
                .nodes
                .into_iter()
                .flatten()
                .filter_map(|c| {
                    let id = c.database_id?;
                    Some(CommentInfo {
                        id,
                        author: c
                            .author
                            .map(|a| a.login)
                            .unwrap_or_else(|| "(unknown)".into()),
                        body: c.body,
                        created_at: c.created_at,
                    })
                })
                .collect();
            prs.push(PrData {
                repo: repo.name_with_owner,
                pr_number: node.number,
                pr_title: node.title,
                pr_url: node.url,
                pr_author: node
                    .author
                    .map(|a| a.login)
                    .unwrap_or_else(|| "(unknown)".into()),
                pr_updated_at: node.updated_at,
                pr_created_at: node.created_at,
                comments,
            });
        }
        sections.push(prs);
    }

    Ok(Fetched {
        viewer_login: data.viewer.login,
        sections,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "data": {
        "viewer": { "login": "nogu3" },
        "merge": {
          "nodes": [
            {
              "number": 9,
              "title": "Implement Device List Management",
              "url": "https://github.com/nogu3/hestia/pull/9",
              "repository": { "nameWithOwner": "nogu3/hestia" },
              "comments": {
                "nodes": [
                  {
                    "databaseId": 4275373830,
                    "author": { "login": "google-labs-jules" },
                    "body": "@nogu3 please merge this",
                    "createdAt": "2026-04-19T06:51:49Z"
                  },
                  {
                    "databaseId": 4275373999,
                    "author": null,
                    "body": "ghost comment",
                    "createdAt": "2026-04-19T07:00:00Z"
                  }
                ]
              }
            },
            {}
          ]
        },
        "review": {
          "nodes": [
            {
              "number": 12,
              "title": "Fix logger",
              "url": "https://github.com/nogu3/hestia/pull/12",
              "createdAt": "2026-07-01T00:00:00Z",
              "author": { "login": "someone" },
              "repository": { "nameWithOwner": "nogu3/hestia" }
            }
          ]
        }
      }
    }"#;

    #[test]
    fn parses_viewer_comments_and_reviews() {
        let parsed = parse_response(FIXTURE).unwrap();
        assert_eq!(parsed.viewer_login, "nogu3");
        assert_eq!(parsed.comments.len(), 2);
        let first = &parsed.comments[0];
        assert_eq!(first.comment_id, 4275373830);
        assert_eq!(first.repo, "nogu3/hestia");
        assert_eq!(first.pr_number, 9);
        assert_eq!(first.author, "google-labs-jules");
        assert_eq!(parsed.review_requests.len(), 1);
        assert_eq!(parsed.review_requests[0].key(), "nogu3/hestia#12");
        assert_eq!(parsed.review_requests[0].author, "someone");
    }

    #[test]
    fn ghost_author_becomes_unknown() {
        let parsed = parse_response(FIXTURE).unwrap();
        assert_eq!(parsed.comments[1].author, "(unknown)");
    }

    #[test]
    fn empty_pr_node_is_skipped() {
        // second merge node is {} (non-PR search node); must not panic or emit items
        let parsed = parse_response(FIXTURE).unwrap();
        assert!(parsed.comments.iter().all(|c| c.repo == "nogu3/hestia"));
    }

    #[test]
    fn graphql_errors_become_api_error() {
        let json = r#"{ "data": null, "errors": [ { "message": "rate limited" } ] }"#;
        let err = parse_response(json).unwrap_err();
        assert!(matches!(err, Error::Api(m) if m.contains("rate limited")));
    }

    #[test]
    fn partial_data_with_errors_still_parses() {
        // GitHub returns 200 with both `errors` and usable `data` (e.g. SAML-protected org);
        // data must win
        let json = r#"{
          "data": {
            "viewer": { "login": "nogu3" },
            "merge": { "nodes": [] },
            "review": { "nodes": [] }
          },
          "errors": [ { "message": "Resource protected by organization SAML enforcement" } ]
        }"#;
        let parsed = parse_response(json).unwrap();
        assert_eq!(parsed.viewer_login, "nogu3");
    }

    #[test]
    fn neither_data_nor_errors_is_api_error() {
        let err = parse_response(r#"{}"#).unwrap_err();
        assert!(matches!(err, Error::Api(_)));
    }

    #[test]
    fn comment_without_database_id_is_skipped() {
        let json = r#"{
          "data": {
            "viewer": { "login": "nogu3" },
            "merge": {
              "nodes": [
                {
                  "number": 1,
                  "title": "t",
                  "url": "u",
                  "repository": { "nameWithOwner": "o/r" },
                  "comments": {
                    "nodes": [
                      { "author": { "login": "bot" }, "body": "no id", "createdAt": "2026-01-01T00:00:00Z" }
                    ]
                  }
                }
              ]
            },
            "review": { "nodes": [] }
          }
        }"#;
        let parsed = parse_response(json).unwrap();
        assert!(parsed.comments.is_empty());
    }

    #[test]
    fn review_node_without_repository_is_skipped() {
        let json = r#"{
          "data": {
            "viewer": { "login": "nogu3" },
            "merge": { "nodes": [] },
            "review": { "nodes": [ {} ] }
          }
        }"#;
        let parsed = parse_response(json).unwrap();
        assert!(parsed.review_requests.is_empty());
    }

    use crate::config::{Section, SectionFilter};

    fn section(query: &str, filter: SectionFilter) -> Section {
        Section {
            title: "t".into(),
            query: query.into(),
            columns: vec![],
            filter,
        }
    }

    #[test]
    fn build_query_aliases_sections_and_passes_variables() {
        let sections = vec![
            section(
                "is:pr mentions:@me",
                SectionFilter::CommentMention {
                    extra_patterns: vec![],
                },
            ),
            section("is:pr review-requested:@me", SectionFilter::None),
        ];
        let (query, vars) = build_query(&sections);
        assert!(query.contains("$q0: String!, $q1: String!"), "got: {query}");
        assert!(query.contains("s0: search(query: $q0, type: ISSUE, first: 50)"));
        assert!(query.contains("s1: search(query: $q1, type: ISSUE, first: 50)"));
        assert!(query.contains("viewer { login }"));
        // comments requested only for the comment-mention section
        assert_eq!(query.matches("comments(last: 50)").count(), 1);
        let comments_pos = query.find("comments(last: 50)").unwrap();
        assert!(query.find("s0:").unwrap() < comments_pos);
        assert!(comments_pos < query.find("s1:").unwrap());
        assert_eq!(vars["q0"], "is:pr mentions:@me");
        assert_eq!(vars["q1"], "is:pr review-requested:@me");
    }

    const SECTIONS_FIXTURE: &str = r#"{
      "data": {
        "viewer": { "login": "nogu3" },
        "s0": {
          "nodes": [
            {
              "number": 9,
              "title": "Implement Device List Management",
              "url": "https://github.com/nogu3/hestia/pull/9",
              "updatedAt": "2026-04-20T00:00:00Z",
              "createdAt": "2026-04-01T00:00:00Z",
              "author": { "login": "jules" },
              "repository": { "nameWithOwner": "nogu3/hestia" },
              "comments": {
                "nodes": [
                  {
                    "databaseId": 4275373830,
                    "author": { "login": "google-labs-jules" },
                    "body": "@nogu3 please merge this",
                    "createdAt": "2026-04-19T06:51:49Z"
                  },
                  {
                    "author": null,
                    "body": "no database id",
                    "createdAt": "2026-04-19T07:00:00Z"
                  }
                ]
              }
            },
            {}
          ]
        },
        "s1": {
          "nodes": [
            {
              "number": 12,
              "title": "Fix logger",
              "url": "https://github.com/nogu3/hestia/pull/12",
              "updatedAt": "2026-07-02T00:00:00Z",
              "createdAt": "2026-07-01T00:00:00Z",
              "author": null,
              "repository": { "nameWithOwner": "nogu3/hestia" }
            }
          ]
        }
      }
    }"#;

    #[test]
    fn parse_sections_returns_ordered_sections() {
        let fetched = parse_sections(SECTIONS_FIXTURE, 2).unwrap();
        assert_eq!(fetched.viewer_login, "nogu3");
        assert_eq!(fetched.sections.len(), 2);
        // empty {} node (non-PR search result) is skipped
        assert_eq!(fetched.sections[0].len(), 1);
        let pr = &fetched.sections[0][0];
        assert_eq!(pr.repo, "nogu3/hestia");
        assert_eq!(pr.pr_number, 9);
        assert_eq!(pr.pr_author, "jules");
        assert_eq!(pr.pr_updated_at, "2026-04-20T00:00:00Z");
        // comment without databaseId is skipped
        assert_eq!(pr.comments.len(), 1);
        assert_eq!(pr.comments[0].id, 4275373830);
        assert_eq!(pr.comments[0].author, "google-labs-jules");
        // section without comments in the query parses with empty comments
        let rr = &fetched.sections[1][0];
        assert_eq!(rr.pr_number, 12);
        assert_eq!(rr.pr_author, "(unknown)"); // ghost author
        assert!(rr.comments.is_empty());
    }

    #[test]
    fn parse_sections_null_data_with_errors_is_api_error() {
        let json = r#"{ "data": null, "errors": [ { "message": "rate limited" } ] }"#;
        let err = parse_sections(json, 1).unwrap_err();
        assert!(matches!(err, Error::Api(m) if m.contains("rate limited")));
    }

    #[test]
    fn parse_sections_partial_data_with_errors_still_parses() {
        let json = r#"{
          "data": { "viewer": { "login": "nogu3" }, "s0": { "nodes": [] } },
          "errors": [ { "message": "SAML enforcement" } ]
        }"#;
        let fetched = parse_sections(json, 1).unwrap();
        assert_eq!(fetched.viewer_login, "nogu3");
        assert!(fetched.sections[0].is_empty());
    }

    #[test]
    fn parse_sections_missing_or_null_alias_yields_empty_section() {
        // a section GitHub nulled out (partial failure) must not kill the fetch
        let json = r#"{
          "data": { "viewer": { "login": "nogu3" }, "s0": null }
        }"#;
        let fetched = parse_sections(json, 1).unwrap();
        assert!(fetched.sections[0].is_empty());
    }
}
