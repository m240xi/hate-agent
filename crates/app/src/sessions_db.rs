//! Session persistence for the desktop app: a SQLite `sessions.db` in the
//! app data dir. Two writers share each row without clobbering:
//!
//! - the session actor (via [`SqliteJournal`]) owns `data` — the
//!   [`PersistedSession`] JSON with message history and counters;
//! - the frontend owns `ui_state` — its transcript/draft blob, written
//!   through `session_save_ui`.
//!
//! Connections open per operation; writes are once-per-turn so the cost
//! is noise next to a provider round-trip.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use hate_core::{PersistedSession, SessionJournal, SessionMeta};
use rusqlite::{Connection, params};

/// What `session_open` returns: list metadata plus both payloads.
pub struct StoredSession {
    /// Row metadata (`title`/`model`/`workspace`/`updated_at`).
    pub meta: SessionMeta,
    /// Actor state to restore; `None` for rows that never ran a turn.
    pub persisted: Option<PersistedSession>,
    /// Frontend blob, raw JSON text (`None` until first save).
    pub ui_state: Option<String>,
}

/// Path handle + schema owner for `sessions.db`.
pub struct SessionDb {
    path: PathBuf,
}

impl SessionDb {
    /// Open (creating if needed) the database and ensure the schema.
    pub fn open() -> Result<Self, String> {
        let dir = dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("hate-agent");
        std::fs::create_dir_all(&dir).map_err(|e| format!("data dir: {e}"))?;
        let path = dir.join("sessions.db");
        let conn = Connection::open(&path).map_err(|e| format!("sessions.db: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id          TEXT PRIMARY KEY,
                mode        TEXT NOT NULL,
                title       TEXT NOT NULL DEFAULT '',
                provider    TEXT NOT NULL DEFAULT '',
                model       TEXT NOT NULL DEFAULT '',
                workspace   TEXT,
                updated_at  INTEGER NOT NULL,
                ui_state    TEXT,
                data        TEXT NOT NULL DEFAULT '{}'
            );
            CREATE INDEX IF NOT EXISTS idx_sessions_mode ON sessions(mode, updated_at DESC);",
        )
        .map_err(|e| format!("sessions schema: {e}"))?;
        Ok(Self { path })
    }

    fn conn(&self) -> Result<Connection, String> {
        Connection::open(&self.path).map_err(|e| format!("sessions.db: {e}"))
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    }

    /// Insert an empty row so the session appears in lists before its
    /// first turn. `INSERT OR IGNORE` keeps re-opens idempotent.
    pub fn create(&self, key: &str, mode: &str) -> Result<(), String> {
        self.conn()?
            .execute(
                "INSERT OR IGNORE INTO sessions (id, mode, updated_at, data)
                 VALUES (?1, ?2, ?3, '{}')",
                params![key, mode, Self::now()],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Full row read for `session_open`.
    pub fn load(&self, key: &str) -> Result<Option<StoredSession>, String> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT mode, title, provider, model, workspace, updated_at, ui_state, data
                 FROM sessions WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let row = stmt
            .query_row(params![key], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, String>(7)?,
                ))
            })
            .ok();
        let Some((mode, title, provider, model, workspace, updated_at, ui_state, data)) = row
        else {
            return Ok(None);
        };
        let persisted = serde_json::from_str::<PersistedSession>(&data).ok();
        Ok(Some(StoredSession {
            meta: SessionMeta {
                id: key.to_string(),
                mode,
                title,
                provider,
                model,
                workspace,
                updated_at: u32::try_from(updated_at).unwrap_or(u32::MAX),
            },
            persisted,
            ui_state,
        }))
    }

    /// Metadata rows for the session browser, newest first.
    pub fn list(&self, mode: Option<&str>) -> Result<Vec<SessionMeta>, String> {
        let conn = self.conn()?;
        let sql = "SELECT id, mode, title, provider, model, workspace, updated_at
                   FROM sessions";
        let mut out = Vec::new();
        if let Some(m) = mode {
            let mut stmt = conn
                .prepare(&format!("{sql} WHERE mode = ?1 ORDER BY updated_at DESC"))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![m], meta_row)
                .map_err(|e| e.to_string())?;
            for r in rows {
                out.push(r.map_err(|e| e.to_string())?);
            }
        } else {
            let mut stmt = conn
                .prepare(&format!("{sql} ORDER BY updated_at DESC"))
                .map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], meta_row).map_err(|e| e.to_string())?;
            for r in rows {
                out.push(r.map_err(|e| e.to_string())?);
            }
        }
        Ok(out)
    }

    /// Persist the frontend's UI blob (transcript/draft/etc).
    pub fn save_ui(&self, key: &str, ui_state: &str) -> Result<(), String> {
        self.conn()?
            .execute(
                "UPDATE sessions SET ui_state = ?1 WHERE id = ?2",
                params![ui_state, key],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete(&self, key: &str) -> Result<(), String> {
        self.conn()?
            .execute("DELETE FROM sessions WHERE id = ?1", params![key])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Fixed column order shared by both `list` queries.
fn meta_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionMeta> {
    Ok(SessionMeta {
        id: r.get(0)?,
        mode: r.get(1)?,
        title: r.get(2)?,
        provider: r.get(3)?,
        model: r.get(4)?,
        workspace: r.get(5)?,
        updated_at: u32::try_from(r.get::<_, i64>(6)?).unwrap_or(u32::MAX),
    })
}

/// [`SessionJournal`] over [`SessionDb`] — the actor's save sink.
pub struct SqliteJournal {
    db: SessionDb,
}

impl SqliteJournal {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            db: SessionDb::open()?,
        })
    }
}

impl SessionJournal for SqliteJournal {
    fn load(&self, key: &str) -> Option<PersistedSession> {
        self.db.load(key).ok().flatten().and_then(|s| s.persisted)
    }

    fn save(&self, snap: &PersistedSession) {
        let data = match serde_json::to_string(snap) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("session serialize failed: {e}");
                return;
            }
        };
        let title: String = snap.title.chars().take(120).collect();
        let res = (|| -> Result<(), String> {
            let conn = self.db.conn()?;
            let updated = conn
                .execute(
                    "UPDATE sessions SET mode=?1, title=?2, provider=?3, model=?4,
                    workspace=?5, updated_at=?6, data=?7 WHERE id=?8",
                    params![
                        snap.mode,
                        title,
                        snap.provider,
                        snap.model,
                        snap.workspace,
                        SessionDb::now(),
                        data,
                        snap.key,
                    ],
                )
                .map_err(|e| e.to_string())?;
            if updated == 0 {
                // Row missing (shouldn't happen — session_new inserts) —
                // create it rather than drop the history.
                conn.execute(
                    "INSERT INTO sessions (id, mode, title, provider, model, workspace,
                        updated_at, data) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        snap.key,
                        snap.mode,
                        title,
                        snap.provider,
                        snap.model,
                        snap.workspace,
                        SessionDb::now(),
                        data,
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })();
        if let Err(e) = res {
            tracing::warn!("session journal save failed: {e}");
        }
    }

    fn delete(&self, key: &str) {
        if let Err(e) = self.db.delete(key) {
            tracing::warn!("session journal delete failed: {e}");
        }
    }

    fn list(&self, mode: Option<&str>) -> Vec<SessionMeta> {
        self.db.list(mode).unwrap_or_default()
    }
}
