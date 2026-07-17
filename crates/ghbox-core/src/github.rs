use std::collections::HashMap;

use serde::Deserialize;

use crate::config::{Section, SectionFilter};
use crate::item::{CommentInfo, PrState};
use crate::{Error, Result};

#[derive(Deserialize)]
struct GqlError {
    message: String,
}

#[derive(Deserialize)]
struct Actor {
    login: String,
}

#[derive(Deserialize)]
struct Search<T> {
    nodes: Vec<Option<T>>,
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

/// Result of one multi-section fetch. `sections` is parallel to the
/// `Section` slice passed to `build_query` / `fetch_sections`.
#[derive(Debug)]
pub struct Fetched {
    pub viewer_login: String,
    pub sections: Vec<Vec<PrData>>,
    /// GraphQL `errors` returned alongside usable `data` (e.g. one
    /// SAML-protected org node FORBIDDEN while the rest succeeded). Surfaced
    /// so a partially-empty result is not mistaken for "all clear".
    pub errors: Vec<String>,
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
    pub state: PrState,
    /// Populated only for sections whose filter needs comment bodies.
    pub comments: Vec<CommentInfo>,
}

/// Builds one GraphQL request covering every section: `viewer` plus one
/// aliased `search` per section (s0, s1, ...). Search strings travel as
/// variables to avoid escaping issues. Comment bodies are requested only
/// for comment-mention sections.
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
            "  s{i}: search(query: $q{i}, type: ISSUE, first: 50) {{\n    nodes {{\n      ... on PullRequest {{\n        number\n        title\n        url\n        state\n        isDraft\n        updatedAt\n        createdAt\n        author {{ login }}\n        repository {{ nameWithOwner }}{comments}\n      }}\n    }}\n  }}\n"
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
    state: String,
    is_draft: bool,
    author: Option<Actor>,
    repository: Option<Repo>,
    comments: CommentConnection,
}

/// The shared HTTP client: connection pooling across polls instead of a
/// fresh TLS handshake per fetch. Building can only fail on TLS/resolver
/// init, so a failure is surfaced per call rather than panicking.
fn http_client() -> Result<&'static reqwest::Client> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    // A concurrent init may have won the race; its client is used, ours dropped.
    Ok(CLIENT.get_or_init(|| client))
}

/// One rate-limit point per call regardless of section count (verified
/// 2026-07-11 with 2 aliased searches in one request).
pub async fn fetch_sections(token: &str, sections: &[Section]) -> Result<Fetched> {
    let (query, variables) = build_query(sections);
    let response = http_client()?
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
    let errors: Vec<String> = resp
        .errors
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.message)
        .collect();
    // GitHub may return HTTP 200 with both `errors` and usable `data` (e.g.
    // one SAML-protected org node is FORBIDDEN while the rest succeeds).
    // Prefer partial data; only treat `errors` as fatal without data.
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
            // Draft only exists while OPEN; MERGED/CLOSED win regardless of
            // the flag. Missing `state` (defaulted "") reads as Open.
            let state = match node.state.as_str() {
                "MERGED" => PrState::Merged,
                "CLOSED" => PrState::Closed,
                _ if node.is_draft => PrState::Draft,
                _ => PrState::Open,
            };
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
                state,
                comments,
            });
        }
        sections.push(prs);
    }

    Ok(Fetched {
        viewer_login: data.viewer.login,
        sections,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::{Section, SectionFilter};
    use crate::item::PrState;

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
        assert!(query.contains("state"), "PR state requested");
        assert!(query.contains("isDraft"), "draft flag requested");
    }

    const SECTIONS_FIXTURE: &str = r#"{
      "data": {
        "viewer": { "login": "nogu3" },
        "s0": {
          "nodes": [
            {
              "number": 9,
              "state": "OPEN", "isDraft": true,
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
              "state": "MERGED",
              "title": "Fix logger",
              "url": "https://github.com/nogu3/hestia/pull/12",
              "updatedAt": "2026-07-02T00:00:00Z",
              "createdAt": "2026-07-01T00:00:00Z",
              "author": null,
              "repository": { "nameWithOwner": "nogu3/hestia" }
            },
            {
              "number": 13,
              "title": "No state field",
              "url": "https://github.com/nogu3/hestia/pull/13",
              "updatedAt": "2026-07-02T00:00:00Z",
              "createdAt": "2026-07-01T00:00:00Z",
              "author": { "login": "alice" },
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
        assert_eq!(pr.state, PrState::Draft, "OPEN + isDraft => Draft");
        // section without comments in the query parses with empty comments
        assert_eq!(fetched.sections[1].len(), 2);
        let rr = &fetched.sections[1][0];
        assert_eq!(rr.pr_number, 12);
        assert_eq!(rr.pr_author, "(unknown)"); // ghost author
        assert!(rr.comments.is_empty());
        assert_eq!(rr.state, PrState::Merged);
        // Responses without a state field (old behavior) fall back to Open
        assert_eq!(fetched.sections[1][1].state, PrState::Open);
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
        // the partial-failure errors must be surfaced, not silently dropped:
        // an empty section from a SAML block otherwise reads as "all clear"
        assert_eq!(fetched.errors, vec!["SAML enforcement".to_string()]);
    }

    #[test]
    fn parse_sections_clean_response_has_no_errors() {
        let fetched = parse_sections(SECTIONS_FIXTURE, 2).unwrap();
        assert!(fetched.errors.is_empty());
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
