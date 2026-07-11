use serde::Deserialize;

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
    if let Some(errors) = resp.errors {
        let messages: Vec<String> = errors.into_iter().map(|e| e.message).collect();
        return Err(Error::Api(messages.join("; ")));
    }
    let data = resp
        .data
        .ok_or_else(|| Error::Api("response has neither data nor errors".into()))?;

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
}
