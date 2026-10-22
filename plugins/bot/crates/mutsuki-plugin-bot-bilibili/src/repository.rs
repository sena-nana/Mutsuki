//! SQLite persistence for poll cursors, link cooldowns, QR sessions and binding challenges.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};

pub struct SqliteBilibiliRepository {
    connection: Mutex<Connection>,
}

impl SqliteBilibiliRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)
                .map_err(|_| rusqlite::Error::InvalidPath(parent.into()))?;
        }
        let connection = Connection::open(path)?;
        // Poll cursors and cooldown stamps are written on every poll tick and every
        // notification. The rusqlite default (rollback journal, `synchronous=FULL`)
        // makes each of those a full fsync. `mutsuki-bot-state-db` already settles on
        // WAL + NORMAL + a busy timeout for the same access shape; match it so a
        // poll interval is not bounded by disk flushes. WAL can be refused on some
        // filesystems, in which case SQLite keeps the previous mode and open still
        // succeeds, so the result is read back rather than asserted.
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let _journal_mode: String =
            connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS cursor (key TEXT PRIMARY KEY, value TEXT NOT NULL);\
             CREATE TABLE IF NOT EXISTS cooldown (key TEXT PRIMARY KEY, seen_ms INTEGER NOT NULL);\
             CREATE TABLE IF NOT EXISTS qr_session (actor_id TEXT PRIMARY KEY, qr_key TEXT NOT NULL);\
             CREATE TABLE IF NOT EXISTS binding_challenge (actor_id TEXT PRIMARY KEY, uid INTEGER NOT NULL, code TEXT NOT NULL);",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub(crate) fn cursor(&self, key: &str) -> Result<Option<String>, rusqlite::Error> {
        self.connection
            .lock()
            .expect("sqlite mutex")
            .query_row("SELECT value FROM cursor WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
    }

    pub(crate) fn set_cursor(&self, key: &str, value: &str) -> Result<(), rusqlite::Error> {
        self.connection.lock().expect("sqlite mutex").execute(
            "INSERT INTO cursor(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub(crate) fn cooldown_ready(
        &self,
        key: &str,
        now_ms: u64,
        cooldown_ms: u64,
    ) -> Result<bool, rusqlite::Error> {
        let connection = self.connection.lock().expect("sqlite mutex");
        let previous: Option<i64> = connection
            .query_row(
                "SELECT seen_ms FROM cooldown WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?;
        let previous = previous
            .map(|previous| {
                u64::try_from(previous)
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, previous))
            })
            .transpose()?;
        if previous.is_some_and(|previous| now_ms.saturating_sub(previous) < cooldown_ms) {
            return Ok(false);
        }
        Ok(true)
    }

    pub(crate) fn record_cooldown(&self, key: &str, now_ms: u64) -> Result<(), rusqlite::Error> {
        let now_ms = i64::try_from(now_ms)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        self.connection.lock().expect("sqlite mutex").execute(
            "INSERT INTO cooldown(key,seen_ms) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET seen_ms=excluded.seen_ms",
            params![key, now_ms],
        )?;
        Ok(())
    }

    pub fn set_qr_session(&self, actor_id: &str, key: &str) -> Result<(), rusqlite::Error> {
        self.connection.lock().expect("sqlite mutex").execute(
            "INSERT INTO qr_session(actor_id,qr_key) VALUES(?1,?2) ON CONFLICT(actor_id) DO UPDATE SET qr_key=excluded.qr_key",
            params![actor_id, key],
        )?;
        Ok(())
    }

    pub fn qr_session(&self, actor_id: &str) -> Result<Option<String>, rusqlite::Error> {
        self.connection
            .lock()
            .expect("sqlite mutex")
            .query_row(
                "SELECT qr_key FROM qr_session WHERE actor_id = ?1",
                [actor_id],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn clear_qr_session(&self, actor_id: &str) -> Result<(), rusqlite::Error> {
        self.connection
            .lock()
            .expect("sqlite mutex")
            .execute("DELETE FROM qr_session WHERE actor_id = ?1", [actor_id])?;
        Ok(())
    }

    pub fn set_binding_challenge(
        &self,
        actor_id: &str,
        uid: u64,
        code: &str,
    ) -> Result<(), rusqlite::Error> {
        let uid = i64::try_from(uid)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        self.connection.lock().expect("sqlite mutex").execute(
            "INSERT INTO binding_challenge(actor_id,uid,code) VALUES(?1,?2,?3) ON CONFLICT(actor_id) DO UPDATE SET uid=excluded.uid,code=excluded.code",
            params![actor_id, uid, code],
        )?;
        Ok(())
    }

    pub fn binding_challenge(
        &self,
        actor_id: &str,
    ) -> Result<Option<(u64, String)>, rusqlite::Error> {
        self.connection
            .lock()
            .expect("sqlite mutex")
            .query_row(
                "SELECT uid,code FROM binding_challenge WHERE actor_id = ?1",
                [actor_id],
                |row| {
                    let uid: i64 = row.get(0)?;
                    let uid = u64::try_from(uid)
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, uid))?;
                    Ok((uid, row.get(1)?))
                },
            )
            .optional()
    }

    pub fn clear_binding_challenge(&self, actor_id: &str) -> Result<(), rusqlite::Error> {
        self.connection.lock().expect("sqlite mutex").execute(
            "DELETE FROM binding_challenge WHERE actor_id = ?1",
            [actor_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_cursor_is_persisted_without_history() {
        let repo = SqliteBilibiliRepository::open(":memory:").unwrap();
        assert!(repo.cursor("dynamic:1").unwrap().is_none());
        repo.set_cursor("dynamic:1", "newest").unwrap();
        assert_eq!(repo.cursor("dynamic:1").unwrap().as_deref(), Some("newest"));
    }

    #[test]
    fn cooldown_is_persisted_in_sqlite() {
        let repo = SqliteBilibiliRepository::open(":memory:").unwrap();
        assert!(repo.cooldown_ready("account:url", 100, 50).unwrap());
        repo.record_cooldown("account:url", 100).unwrap();
        assert!(!repo.cooldown_ready("account:url", 120, 50).unwrap());
        assert!(repo.cooldown_ready("account:url", 151, 50).unwrap());
    }
}
