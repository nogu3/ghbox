use std::path::Path;

use rusqlite::Connection;

use crate::Result;

pub const KIND_MERGE_COMMENT: &str = "merge_comment";
pub const KIND_PR: &str = "pr";

/// Append-only migration list. NEVER edit an existing entry — the DB lives
/// on a NAS shared across machines; only append new statements.
const MIGRATIONS: &[&str] = &[
    "CREATE TABLE done_items (
        kind TEXT NOT NULL,
        key  TEXT NOT NULL,
        done_at TEXT NOT NULL DEFAULT (datetime('now')),
        PRIMARY KEY (kind, key)
    )",
    // v2: PR items are done per (key, updatedAt) — resurface when the PR is
    // updated after the mark. Copy legacy review_request rows to the new
    // 'pr' kind (done_at reformatted to ISO8601 T/Z for lexicographic
    // comparison with GitHub's updatedAt); keep the old rows so old
    // binaries sharing the NAS DB keep working.
    "ALTER TABLE done_items ADD COLUMN updated_at TEXT;
     INSERT OR IGNORE INTO done_items (kind, key, done_at, updated_at)
       SELECT 'pr', key, done_at, strftime('%Y-%m-%dT%H:%M:%SZ', done_at)
       FROM done_items WHERE kind = 'review_request';",
];

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::from_conn(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version as usize > MIGRATIONS.len() {
            return Err(crate::Error::Schema(format!(
                "db schema version {version} is newer than this binary supports (max {}); update ghbox",
                MIGRATIONS.len()
            )));
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn mark_done(&self, kind: &str, key: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO done_items (kind, key) VALUES (?1, ?2)",
            (kind, key),
        )?;
        Ok(())
    }

    pub fn is_done(&self, kind: &str, key: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM done_items WHERE kind = ?1 AND key = ?2",
            (kind, key),
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Marks a PR item done as of `updated_at` (the item's own updatedAt,
    /// not the wall clock). Upsert: re-marking after a resurface refreshes
    /// the recorded updatedAt.
    pub fn mark_done_pr(&self, key: &str, updated_at: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO done_items (kind, key, done_at, updated_at)
             VALUES ('pr', ?1, datetime('now'), ?2)
             ON CONFLICT(kind, key) DO UPDATE SET
               done_at = excluded.done_at, updated_at = excluded.updated_at",
            (key, updated_at),
        )?;
        Ok(())
    }

    /// A PR item is done iff a mark exists whose recorded updatedAt is >=
    /// the item's current updatedAt (ISO8601 strings compare lexicographically).
    pub fn is_done_pr(&self, key: &str, updated_at: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM done_items
             WHERE kind = 'pr' AND key = ?1 AND updated_at >= ?2",
            (key, updated_at),
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_store_has_nothing_done() {
        let store = Store::open_in_memory().unwrap();
        assert!(!store.is_done(KIND_MERGE_COMMENT, "123").unwrap());
    }

    #[test]
    fn busy_timeout_is_set() {
        // Two connections share the NAS DB (main loop + fetch task); without
        // a busy timeout a write collision surfaces as a spurious SQLITE_BUSY.
        // rusqlite sets 5000ms on every open — this pins that behavior so a
        // rusqlite upgrade dropping the default gets caught here.
        let store = Store::open_in_memory().unwrap();
        let ms: i64 = store
            .conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert!(ms >= 5000, "busy_timeout is {ms}ms");
    }

    #[test]
    fn mark_done_then_is_done() {
        let store = Store::open_in_memory().unwrap();
        store.mark_done(KIND_MERGE_COMMENT, "123").unwrap();
        assert!(store.is_done(KIND_MERGE_COMMENT, "123").unwrap());
        // different kind, same key: independent
        assert!(!store.is_done("review_request", "123").unwrap());
    }

    #[test]
    fn mark_done_is_idempotent() {
        let store = Store::open_in_memory().unwrap();
        store.mark_done("review_request", "o/r#1").unwrap();
        store.mark_done("review_request", "o/r#1").unwrap();
        assert!(store.is_done("review_request", "o/r#1").unwrap());
    }

    #[test]
    fn open_creates_parent_dirs_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("state.db");
        {
            let store = Store::open(&path).unwrap();
            store.mark_done(KIND_MERGE_COMMENT, "42").unwrap();
        }
        let store = Store::open(&path).unwrap(); // reopen: migration must be no-op
        assert!(store.is_done(KIND_MERGE_COMMENT, "42").unwrap());
    }

    #[test]
    fn v1_db_migrates_review_requests_to_pr_kind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        {
            // Build a real v1 DB by hand.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            conn.execute(
                "INSERT INTO done_items (kind, key, done_at) VALUES ('review_request', 'o/r#1', '2026-01-02 03:04:05')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO done_items (kind, key) VALUES ('merge_comment', '42')",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        // review_request copied to pr kind; done_at converted to ISO8601 T/Z
        assert!(store.is_done_pr("o/r#1", "2026-01-02T03:04:05Z").unwrap());
        // PR updated after the mark → resurfaces
        assert!(!store.is_done_pr("o/r#1", "2026-01-02T03:04:06Z").unwrap());
        // merge_comment rows untouched
        assert!(store.is_done(KIND_MERGE_COMMENT, "42").unwrap());
        // old rows kept so old binaries sharing the NAS DB still work
        drop(store);
        let conn = Connection::open(&path).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM done_items WHERE kind = 'review_request'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn pr_mark_done_upserts_and_resurfaces_on_update() {
        let store = Store::open_in_memory().unwrap();
        assert!(!store.is_done_pr("o/r#1", "2026-01-01T00:00:00Z").unwrap());
        store.mark_done_pr("o/r#1", "2026-01-01T00:00:00Z").unwrap();
        assert!(store.is_done_pr("o/r#1", "2026-01-01T00:00:00Z").unwrap());
        // PR updated later → resurfaces
        assert!(!store.is_done_pr("o/r#1", "2026-02-01T00:00:00Z").unwrap());
        // `d` again with the new updatedAt → done again (upsert, no constraint error)
        store.mark_done_pr("o/r#1", "2026-02-01T00:00:00Z").unwrap();
        assert!(store.is_done_pr("o/r#1", "2026-02-01T00:00:00Z").unwrap());
    }

    #[test]
    fn newer_db_version_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99).unwrap();
        }
        assert!(matches!(Store::open(&path), Err(crate::Error::Schema(_))));
    }
}
