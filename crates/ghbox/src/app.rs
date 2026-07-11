use ghbox_core::inbox::Inbox;
use ghbox_core::store::{KIND_MERGE_COMMENT, KIND_REVIEW_REQUEST};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Merge,
    Review,
}

pub struct App {
    pub inbox: Inbox,
    pub section: Section,
    pub selected: usize,
    pub status: String,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            inbox: Inbox::default(),
            section: Section::Merge,
            selected: 0,
            status: "loading...".into(),
            should_quit: false,
        }
    }

    pub fn items_len(&self) -> usize {
        match self.section {
            Section::Merge => self.inbox.merge_requests.len(),
            Section::Review => self.inbox.review_requests.len(),
        }
    }

    fn clamp_selected(&mut self) {
        let len = self.items_len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    pub fn set_inbox(&mut self, inbox: Inbox) {
        self.inbox = inbox;
        self.clamp_selected();
    }

    pub fn next(&mut self) {
        if self.selected + 1 < self.items_len() {
            self.selected += 1;
        }
    }

    pub fn prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn toggle_section(&mut self) {
        self.section = match self.section {
            Section::Merge => Section::Review,
            Section::Review => Section::Merge,
        };
        self.selected = 0;
    }

    pub fn selected_url(&self) -> Option<&str> {
        match self.section {
            Section::Merge => self
                .inbox
                .merge_requests
                .get(self.selected)
                .map(|m| m.pr_url.as_str()),
            Section::Review => self
                .inbox
                .review_requests
                .get(self.selected)
                .map(|r| r.pr_url.as_str()),
        }
    }

    pub fn selected_done_entry(&self) -> Option<(&'static str, String)> {
        match self.section {
            Section::Merge => self
                .inbox
                .merge_requests
                .get(self.selected)
                .map(|m| (KIND_MERGE_COMMENT, m.comment_id.to_string())),
            Section::Review => self
                .inbox
                .review_requests
                .get(self.selected)
                .map(|r| (KIND_REVIEW_REQUEST, r.key())),
        }
    }

    pub fn remove_selected(&mut self) {
        match self.section {
            Section::Merge => {
                if self.selected < self.inbox.merge_requests.len() {
                    self.inbox.merge_requests.remove(self.selected);
                }
            }
            Section::Review => {
                if self.selected < self.inbox.review_requests.len() {
                    self.inbox.review_requests.remove(self.selected);
                }
            }
        }
        self.clamp_selected();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghbox_core::types::{MergeRequest, ReviewRequest};

    fn merge_item(id: i64) -> MergeRequest {
        MergeRequest {
            comment_id: id,
            repo: "o/r".into(),
            pr_number: 1,
            pr_title: "t".into(),
            pr_url: format!("https://example.com/{id}"),
            author: "a".into(),
            body: "b".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn review_item(number: u64) -> ReviewRequest {
        ReviewRequest {
            repo: "o/r".into(),
            pr_number: number,
            pr_title: "t".into(),
            pr_url: format!("https://example.com/pr/{number}"),
            author: "a".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn app_with(merges: Vec<MergeRequest>, reviews: Vec<ReviewRequest>) -> App {
        let mut app = App::new();
        app.set_inbox(Inbox {
            merge_requests: merges,
            review_requests: reviews,
        });
        app
    }

    #[test]
    fn navigation_clamps_at_boundaries() {
        let mut app = app_with(vec![merge_item(1), merge_item(2)], vec![]);
        app.prev();
        assert_eq!(app.selected, 0);
        app.next();
        assert_eq!(app.selected, 1);
        app.next();
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn toggle_switches_section_and_resets_selection() {
        let mut app = app_with(vec![merge_item(1), merge_item(2)], vec![review_item(1)]);
        app.next();
        app.toggle_section();
        assert_eq!(app.section, Section::Review);
        assert_eq!(app.selected, 0);
        app.toggle_section();
        assert_eq!(app.section, Section::Merge);
    }

    #[test]
    fn selected_url_and_done_entry_follow_section() {
        let mut app = app_with(vec![merge_item(7)], vec![review_item(3)]);
        assert_eq!(app.selected_url(), Some("https://example.com/7"));
        assert_eq!(
            app.selected_done_entry(),
            Some((ghbox_core::store::KIND_MERGE_COMMENT, "7".to_string()))
        );
        app.toggle_section();
        assert_eq!(app.selected_url(), Some("https://example.com/pr/3"));
        assert_eq!(
            app.selected_done_entry(),
            Some((ghbox_core::store::KIND_REVIEW_REQUEST, "o/r#3".to_string()))
        );
    }

    #[test]
    fn empty_section_yields_none() {
        let app = app_with(vec![], vec![]);
        assert_eq!(app.selected_url(), None);
        assert_eq!(app.selected_done_entry(), None);
    }

    #[test]
    fn remove_selected_clamps_selection() {
        let mut app = app_with(vec![merge_item(1), merge_item(2)], vec![]);
        app.next(); // select last
        app.remove_selected();
        assert_eq!(app.items_len(), 1);
        assert_eq!(app.selected, 0);
        app.remove_selected();
        assert_eq!(app.items_len(), 0);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn set_inbox_clamps_selection() {
        let mut app = app_with(vec![merge_item(1), merge_item(2), merge_item(3)], vec![]);
        app.next();
        app.next(); // selected = 2
        app.set_inbox(Inbox {
            merge_requests: vec![merge_item(1)],
            review_requests: vec![],
        });
        assert_eq!(app.selected, 0);
    }
}
