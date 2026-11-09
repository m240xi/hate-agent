//! hate-ledger: local SQLite token/cost accounting.
//!
//! Every model request and every task lands here so the UI can answer
//! "what did that cost?" with real numbers. The database is local-only,
//! never synced. All types are plain data — this crate does
//! not depend on `hate-core`, which keeps the dependency arrow one-way.

#![deny(unsafe_code)]

mod db;

pub use db::{
    Ledger, LedgerError, PRICE_TABLE, PRICE_TABLE_DATE, PriceEntry, RecordRequest, TaskRollup,
    price_for, price_table_stale,
};
