/// A merge request: a single comment that mentions the viewer and asks to merge.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeRequest {
    /// GitHub comment databaseId. Read-state key for merge requests.
    pub comment_id: i64,
    /// Repository nameWithOwner, e.g. "nogu3/hestia".
    pub repo: String,
    pub pr_number: u64,
    pub pr_title: String,
    pub pr_url: String,
    /// Comment author login.
    pub author: String,
    pub body: String,
    /// ISO8601. Lexicographic order == chronological order.
    pub created_at: String,
}

/// An open PR where a review is requested from the viewer.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewRequest {
    pub repo: String,
    pub pr_number: u64,
    pub pr_title: String,
    pub pr_url: String,
    /// PR author login.
    pub author: String,
    pub created_at: String,
}

impl ReviewRequest {
    /// Read-state key: PR + review request unit.
    pub fn key(&self) -> String {
        format!("{}#{}", self.repo, self.pr_number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_request_key_is_repo_and_number() {
        let rr = ReviewRequest {
            repo: "nogu3/hestia".into(),
            pr_number: 9,
            pr_title: "t".into(),
            pr_url: "u".into(),
            author: "a".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        assert_eq!(rr.key(), "nogu3/hestia#9");
    }
}
