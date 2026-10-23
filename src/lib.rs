//! # RustKV
//!
//! **RustKV** is a lightweight, educational, persistent key-value database built in Rust.
//! It implements an append-only log architecture inspired by Bitcask, featuring:
//!
//! - **Append-only log storage:** Writes and deletes are sequentially appended.
//! - **In-memory hash index:** Provides sub-microsecond $O(1)$ key lookups and prefix scans.
//! - **Crash recovery:** Replays valid records on startup, safe trailing truncation, and mid-log corruption detection.
//! - **ACID Durability:** Configurable synchronization (`fsync` / `fdatasync`).
//! - **Online log compaction:** Garbage collection that reclaims dead space from overwritten and deleted records.
//!
//! ## Quick Start
//!
//! ```no_run
//! use rustkv::{KvEngine, Result};
//!
//! fn main() -> Result<()> {
//!     let mut db = KvEngine::open("data.db")?;
//!
//!     db.set(b"user:101", b"Alice")?;
//!     if let Some(val) = db.get(b"user:101")? {
//!         println!("Found: {}", String::from_utf8_lossy(&val));
//!     }
//!
//!     db.delete(b"user:101")?;
//!     assert!(!db.exists(b"user:101"));
//!
//!     Ok(())
//! }
//! ```

pub mod cli;
pub mod crc;
pub mod engine;
pub mod error;
pub mod index;
pub mod record;
pub mod wal;

// Re-exports for convenient public consumption
pub use engine::{CompactionStats, Config, EngineStats, KvEngine, SyncPolicy};
pub use error::{KvError, Result};
pub use index::IndexEntry;
pub use record::{Record, RecordHeader, RecordType};
pub use wal::{RecoveryMode, RecoveryReport};
