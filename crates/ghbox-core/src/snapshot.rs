//! Startup snapshot: persists the last displayed section items so the next
//! launch renders instantly while the first fetch runs in the background.
//! A local cache, not synced state — safe to delete at any time.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::config::Section;
use crate::inbox::{SectionData, exclude_done};
use crate::item::Item;
use crate::store::Store;

/// Bump on any change to the serialized shape (`Item`, `CommentInfo`,
/// `PrState`, or this envelope). A mismatch discards the file silently.
pub const SNAPSHOT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    /// UTC ISO8601, recorded by the caller at save time.
    pub fetched_at: String,
    pub sections: Vec<SnapshotSection>,
}

#[derive(Serialize, Deserialize)]
pub struct SnapshotSection {
    pub title: String,
    pub fingerprint: String,
    pub items: Vec<Item>,
}

/// Restored startup state. `sections` is parallel to the config sections;
/// `None` means this section starts with the normal Loading state.
pub struct Restored {
    pub fetched_at: String,
    pub sections: Vec<Option<Vec<Item>>>,
}

pub fn default_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("ghbox")
        .join("snapshot.json")
}

/// Hash of everything that affects a section's items except its title (the
/// title is the match key). `DefaultHasher` is only stable within one
/// compiled binary — exactly a cache's lifetime guarantee; a mismatch after
/// an upgrade just means one Loading startup.
pub fn section_fingerprint(section: &Section) -> String {
    let mut h = DefaultHasher::new();
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}",
        section.query,
        section.repos,
        section.filter,
        section.sort,
        section.repos_updated_within_days
    )
    .hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Snapshot of what is currently displayed. `displayed` must be parallel to
/// `sections` (the App holds one slot per config section, same order).
pub fn build(sections: &[Section], displayed: &[SectionData], fetched_at: String) -> Snapshot {
    Snapshot {
        version: SNAPSHOT_VERSION,
        fetched_at,
        sections: sections
            .iter()
            .zip(displayed)
            .map(|(section, data)| SnapshotSection {
                title: section.title.clone(),
                fingerprint: section_fingerprint(section),
                items: data.items.clone(),
            })
            .collect(),
    }
}

/// Atomic write: temp file in the same directory, then rename, so a crash
/// mid-write can never leave a torn file.
pub fn save(path: &Path, snapshot: &Snapshot) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(snapshot)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Loads and restores the snapshot for the given config sections, dropping
/// items marked done since the snapshot was written. Returns `None` for any
/// file-level problem (missing, corrupt, other version, store error) — the
/// snapshot is a cache, so the caller falls back to a normal Loading start.
pub fn load(path: &Path, sections: &[Section], store: &Store) -> Option<Restored> {
    let text = std::fs::read_to_string(path).ok()?;
    let snap: Snapshot = serde_json::from_str(&text).ok()?;
    if snap.version != SNAPSHOT_VERSION {
        return None;
    }
    let by_title: HashMap<&str, &SnapshotSection> = snap
        .sections
        .iter()
        .map(|s| (s.title.as_str(), s))
        .collect();
    let mut out = Vec::with_capacity(sections.len());
    for section in sections {
        let entry = by_title
            .get(section.title.as_str())
            .filter(|s| s.fingerprint == section_fingerprint(section));
        match entry {
            Some(s) => out.push(Some(exclude_done(s.items.clone(), store).ok()?)),
            None => out.push(None),
        }
    }
    Some(Restored {
        fetched_at: snap.fetched_at,
        sections: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Column, Section, SectionFilter, SortKey};
    use crate::inbox::SectionData;
    use crate::item::{CommentInfo, Item, PrState};
    use crate::store::{KIND_MERGE_COMMENT, Store};

    fn section(title: &str, query: &str) -> Section {
        Section {
            title: title.into(),
            query: Some(query.into()),
            repos: vec![],
            columns: vec![Column::Repo],
            filter: SectionFilter::None,
            sort: SortKey::default(),
            repos_updated_within_days: None,
        }
    }

    fn pr_item(number: u64) -> Item {
        Item {
            repo: "o/r".into(),
            pr_number: number,
            pr_title: "t".into(),
            pr_url: "u".into(),
            pr_author: "a".into(),
            pr_updated_at: "2026-07-02T00:00:00Z".into(),
            pr_created_at: "2026-07-01T00:00:00Z".into(),
            state: PrState::Open,
            comment: None,
        }
    }

    fn comment_item(id: i64) -> Item {
        Item {
            comment: Some(CommentInfo {
                id,
                author: "bob".into(),
                body: "@nogu3 merge".into(),
                created_at: "2026-07-03T00:00:00Z".into(),
            }),
            ..pr_item(1)
        }
    }

    fn displayed(a: Vec<Item>, b: Vec<Item>) -> Vec<SectionData> {
        vec![
            SectionData {
                title: "A".into(),
                items: a,
            },
            SectionData {
                title: "B".into(),
                items: b,
            },
        ]
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.json");
        let sections = [section("A", "q1"), section("B", "q2")];
        let snap = build(
            &sections,
            &displayed(vec![pr_item(1)], vec![comment_item(7)]),
            "2026-07-31T01:00:00Z".into(),
        );
        save(&path, &snap).unwrap();
        let store = Store::open_in_memory().unwrap();
        let restored = load(&path, &sections, &store).unwrap();
        assert_eq!(restored.fetched_at, "2026-07-31T01:00:00Z");
        assert_eq!(restored.sections.len(), 2);
        assert_eq!(restored.sections[0].as_ref().unwrap()[0].pr_number, 1);
        let b = restored.sections[1].as_ref().unwrap();
        assert_eq!(b[0].comment.as_ref().unwrap().id, 7);
    }

    #[test]
    fn fingerprint_ignores_title_but_tracks_config() {
        assert_eq!(
            section_fingerprint(&section("A", "q")),
            section_fingerprint(&section("B", "q")),
            "title is the match key, not part of the fingerprint"
        );
        assert_ne!(
            section_fingerprint(&section("A", "q")),
            section_fingerprint(&section("A", "other")),
        );
        let mut with_filter = section("A", "q");
        with_filter.filter = SectionFilter::ReviewRequested;
        assert_ne!(
            section_fingerprint(&section("A", "q")),
            section_fingerprint(&with_filter),
        );
    }

    #[test]
    fn fingerprint_changes_with_repos_window() {
        let a = section("A", "q");
        let mut b = section("A", "q");
        b.repos_updated_within_days = Some(7);
        assert_ne!(section_fingerprint(&a), section_fingerprint(&b));
    }

    #[test]
    fn changed_section_config_restores_others_but_not_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.json");
        let saved = [section("A", "q1"), section("B", "q2")];
        let snap = build(
            &saved,
            &displayed(vec![pr_item(1)], vec![pr_item(2)]),
            "t".into(),
        );
        save(&path, &snap).unwrap();
        let store = Store::open_in_memory().unwrap();
        // B's query changed since the snapshot; C never existed
        let now = [
            section("A", "q1"),
            section("B", "changed"),
            section("C", "q3"),
        ];
        let restored = load(&path, &now, &store).unwrap();
        assert!(restored.sections[0].is_some());
        assert!(
            restored.sections[1].is_none(),
            "changed config must not restore"
        );
        assert!(
            restored.sections[2].is_none(),
            "unknown title must not restore"
        );
    }

    #[test]
    fn missing_file_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        assert!(load(&dir.path().join("nope.json"), &[section("A", "q")], &store).is_none());
    }

    #[test]
    fn corrupt_json_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.json");
        std::fs::write(&path, "not json{").unwrap();
        let store = Store::open_in_memory().unwrap();
        assert!(load(&path, &[section("A", "q")], &store).is_none());
    }

    #[test]
    fn version_mismatch_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.json");
        let sections = [section("A", "q")];
        let snap = build(&sections, &displayed(vec![], vec![])[..1], "t".into());
        let mut value = serde_json::to_value(&snap).unwrap();
        value["version"] = serde_json::json!(SNAPSHOT_VERSION + 1);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let store = Store::open_in_memory().unwrap();
        assert!(load(&path, &sections, &store).is_none());
    }

    #[test]
    fn done_items_are_excluded_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.json");
        let sections = [section("A", "q1"), section("B", "q2")];
        let snap = build(
            &sections,
            &displayed(vec![pr_item(1)], vec![comment_item(7), comment_item(8)]),
            "t".into(),
        );
        save(&path, &snap).unwrap();
        let store = Store::open_in_memory().unwrap();
        store.mark_done(KIND_MERGE_COMMENT, "7").unwrap();
        let restored = load(&path, &sections, &store).unwrap();
        let b = restored.sections[1].as_ref().unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].comment.as_ref().unwrap().id, 8);
    }

    #[test]
    fn save_creates_parent_dirs_and_replaces_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("snapshot.json");
        let sections = [section("A", "q")];
        let store = Store::open_in_memory().unwrap();
        let one = build(
            &sections,
            &displayed(vec![pr_item(1)], vec![])[..1],
            "t1".into(),
        );
        save(&path, &one).unwrap();
        let two = build(
            &sections,
            &displayed(vec![pr_item(2)], vec![])[..1],
            "t2".into(),
        );
        save(&path, &two).unwrap();
        let restored = load(&path, &sections, &store).unwrap();
        assert_eq!(restored.fetched_at, "t2");
        assert_eq!(restored.sections[0].as_ref().unwrap()[0].pr_number, 2);
    }
}
