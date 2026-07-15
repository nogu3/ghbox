use serde::Serialize;

/// PR state for the state-icon column. `Draft` is derived at parse time from
/// GraphQL `state == OPEN && isDraft`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
}

/// A single row in a section: a PR (`comment == None`) or a specific comment
/// on a PR (`comment == Some`, produced by the comment-mention filter).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    /// Repository nameWithOwner, e.g. "nogu3/hestia".
    pub repo: String,
    pub pr_number: u64,
    pub pr_title: String,
    pub pr_url: String,
    /// PR author login.
    pub pr_author: String,
    /// ISO8601. Lexicographic order == chronological order.
    pub pr_updated_at: String,
    pub pr_created_at: String,
    pub state: PrState,
    /// Some only for items produced by the comment-mention filter.
    pub comment: Option<CommentInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommentInfo {
    /// GitHub comment databaseId. Read-state key for comment items.
    pub id: i64,
    /// Comment author login.
    pub author: String,
    pub body: String,
    pub created_at: String,
}

impl Item {
    /// Stable identity shared by the command-filter protocol and read-state:
    /// `comment:<databaseId>` or `pr:<repo>#<number>`.
    pub fn stable_id(&self) -> String {
        match &self.comment {
            Some(c) => format!("comment:{}", c.id),
            None => format!("pr:{}", self.pr_key()),
        }
    }

    /// Read-state key for PR items: `repo#number`.
    pub fn pr_key(&self) -> String {
        format!("{}#{}", self.repo, self.pr_number)
    }

    /// Sort timestamp: comment items by comment creation, PR items by last
    /// update.
    pub fn sort_time(&self) -> &str {
        match &self.comment {
            Some(c) => &c.created_at,
            None => &self.pr_updated_at,
        }
    }

    /// Author for the `author` column: comment author for comment items,
    /// PR author otherwise.
    pub fn display_author(&self) -> &str {
        match &self.comment {
            Some(c) => &c.author,
            None => &self.pr_author,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr_item() -> Item {
        Item {
            repo: "nogu3/hestia".into(),
            pr_number: 9,
            pr_title: "t".into(),
            pr_url: "u".into(),
            pr_author: "alice".into(),
            pr_updated_at: "2026-07-02T00:00:00Z".into(),
            pr_created_at: "2026-07-01T00:00:00Z".into(),
            state: PrState::Open,
            comment: None,
        }
    }

    fn comment_item() -> Item {
        Item {
            comment: Some(CommentInfo {
                id: 42,
                author: "bob".into(),
                body: "@nogu3 merge please".into(),
                created_at: "2026-07-03T00:00:00Z".into(),
            }),
            ..pr_item()
        }
    }

    #[test]
    fn pr_item_stable_id_and_key() {
        assert_eq!(pr_item().stable_id(), "pr:nogu3/hestia#9");
        assert_eq!(pr_item().pr_key(), "nogu3/hestia#9");
    }

    #[test]
    fn comment_item_stable_id_uses_comment_id() {
        assert_eq!(comment_item().stable_id(), "comment:42");
    }

    #[test]
    fn sort_time_follows_item_kind() {
        assert_eq!(pr_item().sort_time(), "2026-07-02T00:00:00Z");
        assert_eq!(comment_item().sort_time(), "2026-07-03T00:00:00Z");
    }

    #[test]
    fn display_author_follows_item_kind() {
        assert_eq!(pr_item().display_author(), "alice");
        assert_eq!(comment_item().display_author(), "bob");
    }

    #[test]
    fn serializes_all_fields() {
        let json = serde_json::to_value(comment_item()).unwrap();
        assert_eq!(json["repo"], "nogu3/hestia");
        assert_eq!(json["pr_number"], 9);
        assert_eq!(json["state"], "open");
        assert_eq!(json["comment"]["id"], 42);
        assert_eq!(
            serde_json::to_value(pr_item()).unwrap()["comment"],
            serde_json::Value::Null
        );
    }
}
