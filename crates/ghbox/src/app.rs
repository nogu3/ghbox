use std::collections::HashSet;

use ghbox_core::inbox::{SectionData, SectionResult};
use ghbox_core::item::Item;

/// What pressing the done key should record for the selected item.
#[derive(Debug, Clone, PartialEq)]
pub enum DoneEntry {
    Comment(i64),
    Pr { key: String, updated_at: String },
}

pub struct App {
    /// One slot per config section, same order. Titles are filled at startup
    /// so the tab bar renders before the first fetch completes.
    /// Invariant: never empty (Config::validate rejects empty sections).
    pub sections: Vec<SectionData>,
    pub active: usize,
    pub selected: usize,
    pub status: String,
    pub should_quit: bool,
    /// Stable ids marked done since the last fetch result was applied. A
    /// fetch in flight at mark time was built before the mark and would
    /// briefly resurface the item; these ids suppress that one result.
    pending_done: HashSet<String>,
}

impl App {
    pub fn new(titles: Vec<String>) -> Self {
        Self {
            sections: titles
                .into_iter()
                .map(|title| SectionData {
                    title,
                    items: Vec::new(),
                })
                .collect(),
            active: 0,
            selected: 0,
            status: "loading...".into(),
            should_quit: false,
            pending_done: HashSet::new(),
        }
    }

    pub fn active_section(&self) -> &SectionData {
        &self.sections[self.active]
    }

    pub fn items_len(&self) -> usize {
        self.active_section().items.len()
    }

    fn clamp_selected(&mut self) {
        let len = self.items_len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    /// Applies one fetch's per-section results. A section that failed (Err)
    /// keeps its previous items; the first error message is returned for
    /// the status bar.
    pub fn apply_results(&mut self, results: Vec<SectionResult>) -> Option<String> {
        let mut first_error = None;
        for (slot, result) in self.sections.iter_mut().zip(results) {
            match result {
                Ok(mut data) => {
                    // Drop items marked done after this fetch started; later
                    // fetches consult the store, so this is one-shot (cleared
                    // below) and cannot block a legitimate resurface.
                    if !self.pending_done.is_empty() {
                        data.items
                            .retain(|item| !self.pending_done.contains(&item.stable_id()));
                    }
                    *slot = data;
                }
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }
        self.pending_done.clear();
        self.clamp_selected();
        first_error
    }

    pub fn next(&mut self) {
        if self.selected + 1 < self.items_len() {
            self.selected += 1;
        }
    }

    pub fn prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn next_section(&mut self) {
        self.active = (self.active + 1) % self.sections.len();
        self.selected = 0;
    }

    pub fn prev_section(&mut self) {
        self.active = (self.active + self.sections.len() - 1) % self.sections.len();
        self.selected = 0;
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.active_section().items.get(self.selected)
    }

    pub fn selected_url(&self) -> Option<&str> {
        self.selected_item().map(|i| i.pr_url.as_str())
    }

    pub fn selected_done_entry(&self) -> Option<DoneEntry> {
        self.selected_item().map(|i| match &i.comment {
            Some(c) => DoneEntry::Comment(c.id),
            None => DoneEntry::Pr {
                key: i.pr_key(),
                updated_at: i.pr_updated_at.clone(),
            },
        })
    }

    /// Removes the selected (just-marked-done) item from every section —
    /// done is global, so a PR matching several sections' queries must
    /// disappear everywhere — and suppresses it in the next fetch result in
    /// case one was already in flight when the mark happened.
    pub fn remove_selected(&mut self) {
        let Some(id) = self.selected_item().map(Item::stable_id) else {
            return;
        };
        for section in &mut self.sections {
            section.items.retain(|item| item.stable_id() != id);
        }
        self.pending_done.insert(id);
        self.clamp_selected();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghbox_core::item::CommentInfo;

    fn pr_item(number: u64) -> Item {
        Item {
            repo: "o/r".into(),
            pr_number: number,
            pr_title: "t".into(),
            pr_url: format!("https://example.com/pr/{number}"),
            pr_author: "a".into(),
            pr_updated_at: "2026-07-02T00:00:00Z".into(),
            pr_created_at: "2026-07-01T00:00:00Z".into(),
            comment: None,
        }
    }

    fn comment_item(id: i64) -> Item {
        Item {
            pr_url: format!("https://example.com/{id}"),
            comment: Some(CommentInfo {
                id,
                author: "bob".into(),
                body: "@nogu3 merge".into(),
                created_at: "2026-07-03T00:00:00Z".into(),
            }),
            ..pr_item(1)
        }
    }

    fn app3() -> App {
        let mut app = App::new(vec!["A".into(), "B".into(), "C".into()]);
        app.sections[0].items = vec![comment_item(7), comment_item(8)];
        app.sections[1].items = vec![pr_item(3)];
        app
    }

    #[test]
    fn navigation_clamps_at_boundaries() {
        let mut app = app3();
        app.prev();
        assert_eq!(app.selected, 0);
        app.next();
        assert_eq!(app.selected, 1);
        app.next();
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn section_cycling_wraps_and_resets_selection() {
        let mut app = app3();
        app.next();
        app.next_section();
        assert_eq!(app.active, 1);
        assert_eq!(app.selected, 0);
        app.next_section();
        app.next_section();
        assert_eq!(app.active, 0); // wrapped
        app.prev_section();
        assert_eq!(app.active, 2); // wrapped backwards
    }

    #[test]
    fn done_entry_follows_item_kind() {
        let mut app = app3();
        assert_eq!(app.selected_done_entry(), Some(DoneEntry::Comment(7)));
        assert_eq!(app.selected_url(), Some("https://example.com/7"));
        app.next_section();
        assert_eq!(
            app.selected_done_entry(),
            Some(DoneEntry::Pr {
                key: "o/r#3".into(),
                updated_at: "2026-07-02T00:00:00Z".into()
            })
        );
    }

    #[test]
    fn empty_section_yields_none() {
        let mut app = app3();
        app.active = 2;
        assert_eq!(app.selected_url(), None);
        assert_eq!(app.selected_done_entry(), None);
    }

    #[test]
    fn apply_results_replaces_ok_and_keeps_err_sections() {
        let mut app = app3();
        let err = app.apply_results(vec![
            Ok(SectionData {
                title: "A".into(),
                items: vec![comment_item(9)],
            }),
            Err("B: command filter exited with 1".into()),
            Ok(SectionData {
                title: "C".into(),
                items: vec![],
            }),
        ]);
        assert_eq!(err.as_deref(), Some("B: command filter exited with 1"));
        assert_eq!(app.sections[0].items.len(), 1); // replaced
        assert_eq!(app.sections[1].items.len(), 1); // previous items kept
    }

    #[test]
    fn apply_results_clamps_selection() {
        let mut app = app3();
        app.next(); // selected = 1
        app.apply_results(vec![
            Ok(SectionData {
                title: "A".into(),
                items: vec![comment_item(9)],
            }),
            Ok(SectionData {
                title: "B".into(),
                items: vec![],
            }),
            Ok(SectionData {
                title: "C".into(),
                items: vec![],
            }),
        ]);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn remove_selected_removes_same_item_from_all_sections() {
        // the same PR can match several sections' queries; done is global,
        // so it must disappear everywhere, not linger until the next poll
        let mut app = app3();
        app.sections[2].items = vec![pr_item(3), pr_item(4)];
        app.active = 1; // section B holds pr_item(3)
        app.remove_selected();
        assert!(app.sections[1].items.is_empty());
        let remaining: Vec<u64> = app.sections[2].items.iter().map(|i| i.pr_number).collect();
        assert_eq!(remaining, vec![4]); // 3 removed here too, 4 untouched
    }

    #[test]
    fn apply_results_drops_items_done_since_fetch_started() {
        let mut app = app3();
        app.remove_selected(); // marks comment 7 done locally
        // a fetch that was already in flight was built before the mark and
        // still carries comment 7 — it must not flash back
        let results = |ids: Vec<i64>| {
            vec![
                Ok(SectionData {
                    title: "A".into(),
                    items: ids.into_iter().map(comment_item).collect(),
                }),
                Ok(SectionData {
                    title: "B".into(),
                    items: vec![],
                }),
                Ok(SectionData {
                    title: "C".into(),
                    items: vec![],
                }),
            ]
        };
        app.apply_results(results(vec![7, 9]));
        let ids: Vec<i64> = app.sections[0]
            .items
            .iter()
            .map(|i| i.comment.as_ref().unwrap().id)
            .collect();
        assert_eq!(ids, vec![9]);
        // suppression is one-shot: later fetches consult the store, so a
        // legitimately resurfaced item must show again
        app.apply_results(results(vec![7]));
        assert_eq!(app.sections[0].items.len(), 1);
    }

    #[test]
    fn remove_selected_clamps_selection() {
        let mut app = app3();
        app.next(); // select last of section 0
        app.remove_selected();
        assert_eq!(app.items_len(), 1);
        assert_eq!(app.selected, 0);
        app.remove_selected();
        assert_eq!(app.items_len(), 0);
        assert_eq!(app.selected, 0);
    }
}
