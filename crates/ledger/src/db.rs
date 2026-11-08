//! SQLite schema and query layer. One file, one connection behind a
//! mutex — ledger writes are tiny and infrequent, so this is plenty.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

/// Ledger failures. IO/schema problems are fatal to the caller's choice;
/// the session treats ledger errors as non-fatal (accounting must never
/// kill a turn).
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    /// SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Filesystem error creating/opening the db file.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// One model request's usage, as recorded in `requests`.
#[derive(Debug, Clone)]
pub struct RecordRequest {
    /// Session this request belongs to.
    pub session_id: String,
    /// Turn counter within the session.
    pub turn_id: u32,
    /// Correlates retries of the same logical request.
    pub request_id: String,
    /// Model id used.
    pub model: String,
    /// Provider adapter name (`openai_compat`, `anthropic`, …).
    pub provider: String,
    /// Total input tokens.
    pub input_tokens: u64,
    /// Input tokens served from prompt cache.
    pub cached_input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Thinking/reasoning tokens.
    pub thinking_tokens: u64,
    /// Wall-clock latency of the streamed request, ms.
    pub latency_ms: u64,
}

/// Rolled-up totals for one task (= one session's worth of turns).
#[derive(Debug, Clone, Default)]
pub struct TaskRollup {
    /// Total input tokens.
    pub input_tokens: u64,
    /// Total cached input tokens.
    pub cached_input_tokens: u64,
    /// Total output tokens.
    pub output_tokens: u64,
    /// Total thinking tokens.
    pub thinking_tokens: u64,
    /// Total wall-clock latency across requests, ms.
    pub latency_ms: u64,
    /// Number of model requests.
    pub requests: u64,
    /// Computed cost in USD, when the price table covers the model.
    pub cost_usd: Option<f64>,
}

/// USD per 1M tokens for one model family. `cached` is the discounted
/// cache-read rate; `output` covers completion + thinking.
#[derive(Debug, Clone, Copy)]
pub struct PriceEntry {
    /// Substring matched against the model id (first match wins).
    pub model_match: &'static str,
    /// Input price per 1M tokens.
    pub input: f64,
    /// Cached-input price per 1M tokens.
    pub cached: f64,
    /// Output price per 1M tokens.
    pub output: f64,
}

/// The bundled price table, versioned by [`PRICE_TABLE_DATE`]. Users can
/// shadow it with their own table later; for now it ships read-only.
///
/// Prices are USD per 1M tokens, published list prices as of the table
/// date. Models not matching any entry record `cost_usd = NULL` — an
/// honest "unknown" beats a wrong number.
pub const PRICE_TABLE: &[PriceEntry] = &[
    PriceEntry {
        model_match: "claude-opus",
        input: 15.0,
        cached: 1.5,
        output: 75.0,
    },
    PriceEntry {
        model_match: "claude-sonnet",
        input: 3.0,
        cached: 0.30,
        output: 15.0,
    },
    PriceEntry {
        model_match: "claude-haiku",
        input: 0.80,
        cached: 0.08,
        output: 4.0,
    },
    PriceEntry {
        model_match: "gpt-5",
        input: 1.25,
        cached: 0.125,
        output: 10.0,
    },
    PriceEntry {
        model_match: "gpt-4.1",
        input: 2.0,
        cached: 0.50,
        output: 8.0,
    },
    PriceEntry {
        model_match: "gpt-4o",
        input: 2.5,
        cached: 1.25,
        output: 10.0,
    },
    PriceEntry {
        model_match: "gemini-2.5-pro",
        input: 1.25,
        cached: 0.125,
        output: 10.0,
    },
    PriceEntry {
        model_match: "gemini-2.5-flash",
        input: 0.30,
        cached: 0.03,
        output: 2.5,
    },
    PriceEntry {
        model_match: "deepseek",
        input: 0.27,
        cached: 0.07,
        output: 1.10,
    },
    PriceEntry {
        model_match: "qwen3",
        input: 0.30,
        cached: 0.10,
        output: 1.20,
    },
];

/// Unix-day stamp of [`PRICE_TABLE`]. UI warns "price table stale" after
/// 60 days (spec §22).
pub const PRICE_TABLE_DATE: u64 = 20_720; // 2025-11-08

/// Unix epoch seconds now, saturating at 0 on clock weirdness.
fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Look up the price for a model id. Returns `None` when nothing matches —
/// callers must treat that as "cost unknown", never as zero.
pub fn price_for(model: &str) -> Option<&'static PriceEntry> {
    let m = model.to_ascii_lowercase();
    PRICE_TABLE.iter().find(|e| m.contains(e.model_match))
}

/// Whether the bundled table is older than 60 days.
pub fn price_table_stale() -> bool {
    let today = now_epoch() / 86_400;
    today.saturating_sub(PRICE_TABLE_DATE) > 60
}

/// Compute USD cost for one request from the price table.
fn cost_for(model: &str, rec: &RecordRequest) -> Option<f64> {
    let p = price_for(model)?;
    let uncached = rec.input_tokens.saturating_sub(rec.cached_input_tokens);
    let cost = (uncached as f64 * p.input
        + rec.cached_input_tokens as f64 * p.cached
        + (rec.output_tokens + rec.thinking_tokens) as f64 * p.output)
        / 1_000_000.0;
    Some(cost)
}

/// The ledger itself. `Connection` is `Send` but not `Sync`, hence the
/// mutex; all operations are short writes/reads.
pub struct Ledger {
    conn: Mutex<Connection>,
}

impl Ledger {
    /// Open (or create) the ledger at `path`, applying the schema.
    ///
    /// # Errors
    /// Returns [`LedgerError`] on IO or schema failure.
    pub fn open(path: &Path) -> Result<Self, LedgerError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::init_schema(conn)
    }

    /// In-memory ledger for tests.
    ///
    /// # Errors
    /// Returns [`LedgerError`] if SQLite can't open `:memory:`.
    pub fn in_memory() -> Result<Self, LedgerError> {
        Self::init_schema(Connection::open_in_memory()?)
    }

    fn init_schema(conn: Connection) -> Result<Self, LedgerError> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS requests (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               session_id TEXT NOT NULL,
               turn_id INTEGER NOT NULL,
               request_id TEXT NOT NULL,
               model TEXT NOT NULL,
               provider TEXT NOT NULL,
               input_tokens INTEGER NOT NULL,
               cached_input_tokens INTEGER NOT NULL,
               output_tokens INTEGER NOT NULL,
               thinking_tokens INTEGER NOT NULL,
               latency_ms INTEGER NOT NULL,
               cost_usd REAL,
               created_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_requests_session
               ON requests(session_id);
             CREATE TABLE IF NOT EXISTS tasks (
               session_id TEXT PRIMARY KEY,
               model TEXT NOT NULL,
               started_at INTEGER NOT NULL,
               ended_at INTEGER,
               input_tokens INTEGER NOT NULL DEFAULT 0,
               cached_input_tokens INTEGER NOT NULL DEFAULT 0,
               output_tokens INTEGER NOT NULL DEFAULT 0,
               thinking_tokens INTEGER NOT NULL DEFAULT 0,
               latency_ms INTEGER NOT NULL DEFAULT 0,
               requests INTEGER NOT NULL DEFAULT 0,
               cost_usd REAL
             );",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Record one model request and roll it into the task totals.
    /// Returns the computed USD cost (`None` when the model is unpriced).
    ///
    /// # Errors
    /// Returns [`LedgerError::Sqlite`] on write failure.
    pub fn record_request(&self, rec: &RecordRequest) -> Result<Option<f64>, LedgerError> {
        let cost = cost_for(&rec.model, rec);
        let conn = self.conn.lock().expect("ledger mutex poisoned");
        conn.execute(
            "INSERT INTO requests
               (session_id, turn_id, request_id, model, provider,
                input_tokens, cached_input_tokens, output_tokens,
                thinking_tokens, latency_ms, cost_usd, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                rec.session_id,
                rec.turn_id,
                rec.request_id,
                rec.model,
                rec.provider,
                rec.input_tokens.cast_signed(),
                rec.cached_input_tokens.cast_signed(),
                rec.output_tokens.cast_signed(),
                rec.thinking_tokens.cast_signed(),
                rec.latency_ms.cast_signed(),
                cost,
                now_epoch().cast_signed(),
            ],
        )?;
        // Roll into the task row; create it on first sight.
        conn.execute(
            "INSERT INTO tasks (session_id, model, started_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id) DO NOTHING",
            params![rec.session_id, rec.model, now_epoch().cast_signed()],
        )?;
        conn.execute(
            "UPDATE tasks SET
               input_tokens = input_tokens + ?2,
               cached_input_tokens = cached_input_tokens + ?3,
               output_tokens = output_tokens + ?4,
               thinking_tokens = thinking_tokens + ?5,
               latency_ms = latency_ms + ?6,
               requests = requests + 1,
               cost_usd = CASE WHEN cost_usd IS NULL AND ?7 IS NOT NULL THEN ?7
                               WHEN cost_usd IS NULL THEN NULL
                               WHEN ?7 IS NULL THEN cost_usd
                               ELSE cost_usd + ?7 END
             WHERE session_id = ?1",
            params![
                rec.session_id,
                rec.input_tokens.cast_signed(),
                rec.cached_input_tokens.cast_signed(),
                rec.output_tokens.cast_signed(),
                rec.thinking_tokens.cast_signed(),
                rec.latency_ms.cast_signed(),
                cost,
            ],
        )?;
        Ok(cost)
    }

    /// Mark a task finished and return its rollup.
    ///
    /// # Errors
    /// Returns [`LedgerError::Sqlite`] on failure.
    pub fn end_task(&self, session_id: &str) -> Result<TaskRollup, LedgerError> {
        let conn = self.conn.lock().expect("ledger mutex poisoned");
        conn.execute(
            "UPDATE tasks SET ended_at = ?2 WHERE session_id = ?1",
            params![session_id, now_epoch().cast_signed()],
        )?;
        drop(conn);
        self.task_rollup(session_id)
    }

    /// Read the rollup for a session without ending it.
    ///
    /// # Errors
    /// Returns [`LedgerError::Sqlite`] on failure.
    pub fn task_rollup(&self, session_id: &str) -> Result<TaskRollup, LedgerError> {
        let conn = self.conn.lock().expect("ledger mutex poisoned");
        let row = conn
            .query_row(
                "SELECT input_tokens, cached_input_tokens, output_tokens,
                        thinking_tokens, latency_ms, requests, cost_usd
                 FROM tasks WHERE session_id = ?1",
                params![session_id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, Option<f64>>(6)?,
                    ))
                },
            )
            .optional()?;
        Ok(match row {
            Some((input, cached, output, thinking, latency, requests, cost)) => TaskRollup {
                input_tokens: input.cast_unsigned(),
                cached_input_tokens: cached.cast_unsigned(),
                output_tokens: output.cast_unsigned(),
                thinking_tokens: thinking.cast_unsigned(),
                latency_ms: latency.cast_unsigned(),
                requests: requests.cast_unsigned(),
                cost_usd: cost,
            },
            None => TaskRollup::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(session: &str) -> RecordRequest {
        RecordRequest {
            session_id: session.to_string(),
            turn_id: 1,
            request_id: "r1".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            provider: "anthropic".to_string(),
            input_tokens: 10_000,
            cached_input_tokens: 8_000,
            output_tokens: 500,
            thinking_tokens: 100,
            latency_ms: 1_200,
        }
    }

    #[test]
    fn record_and_rollup() {
        let l = Ledger::in_memory().expect("memory ledger");
        let cost = l.record_request(&rec("s1")).expect("record");
        // sonnet: (2000*3 + 8000*0.30 + 600*15)/1e6 = (6000+2400+9000)/1e6
        assert!((cost.expect("priced") - 0.0174).abs() < 1e-9);
        l.record_request(&rec("s1")).expect("record2");
        let roll = l.task_rollup("s1").expect("rollup");
        assert_eq!(roll.requests, 2);
        assert_eq!(roll.input_tokens, 20_000);
        assert_eq!(roll.cached_input_tokens, 16_000);
        assert!((roll.cost_usd.expect("cost") - 0.0348).abs() < 1e-9);
        l.end_task("s1").expect("end");
    }

    #[test]
    fn unknown_model_has_no_cost() {
        let l = Ledger::in_memory().expect("memory ledger");
        let mut r = rec("s2");
        r.model = "nous/hermes-4-405b".to_string();
        assert!(l.record_request(&r).expect("record").is_none());
        assert!(l.task_rollup("s2").expect("rollup").cost_usd.is_none());
    }

    #[test]
    fn price_lookup_is_case_insensitive_substring() {
        assert!(price_for("CLAUDE-SONNET-4-5-20250929").is_some());
        assert!(price_for("deepseek-v3.2").is_some());
        assert!(price_for("totally-unknown-model").is_none());
    }
}
