use std::path::Path;

use rusqlite::Connection;

use crate::Result;

pub const KIND_MERGE_COMMENT: &str = "merge_comment";
pub const KIND_REVIEW_REQUEST: &str = "review_request";

/// Append-only migration list. NEVER edit an existing entry — the DB lives
/// on a NAS shared across machines; only append new statements.
const MIGRATIONS: &[&str] = &["CREATE TABLE done_items (
        kind TEXT NOT NULL,
        key  TEXT NOT NULL,
        done_at TEXT NOT NULL DEFAULT (datetime('now')),
        PRIMARY KEY (kind, key)
    )"];

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
    fn mark_done_then_is_done() {
        let store = Store::open_in_memory().unwrap();
        store.mark_done(KIND_MERGE_COMMENT, "123").unwrap();
        assert!(store.is_done(KIND_MERGE_COMMENT, "123").unwrap());
        // different kind, same key: independent
        assert!(!store.is_done(KIND_REVIEW_REQUEST, "123").unwrap());
    }

    #[test]
    fn mark_done_is_idempotent() {
        let store = Store::open_in_memory().unwrap();
        store.mark_done(KIND_REVIEW_REQUEST, "o/r#1").unwrap();
        store.mark_done(KIND_REVIEW_REQUEST, "o/r#1").unwrap();
        assert!(store.is_done(KIND_REVIEW_REQUEST, "o/r#1").unwrap());
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
}
