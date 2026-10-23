# RustKV — A Persistent Key-Value Database in Rust

[![Rust](https://img.shields.io/badge/rust-stable-brightgreen.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](Cargo.toml)

**RustKV** is a high-performance, lightweight, persistent key-value storage engine engineered from scratch in Rust. Inspired by the **Bitcask** storage model, it pairs an append-only write-ahead log (WAL) on disk with an in-memory hash index.

Designed as an educational yet production-grade reference, RustKV teaches real-world database systems concepts: **storage engine internals**, **binary serialization**, **file I/O**, **crash recovery**, **corruption detection**, **durability guarantees**, and **online log compaction**.

---

## Table of Contents

- [1. Architecture Overview](#1-architecture-overview)
- [2. Binary Record Layout](#2-binary-record-layout)
- [3. Crash Recovery and Corruption Handling](#3-crash-recovery-and-corruption-handling)
- [4. Durability & Linux File I/O Internals](#4-durability--linux-file-io-internals)
- [5. Online Log Compaction](#5-online-log-compaction)
- [6. Supported Commands](#6-supported-commands)
- [7. Getting Started & Build Instructions](#7-getting-started--build-instructions)
- [8. Running Tests](#8-running-tests)
- [9. Performance Benchmarks](#9-performance-benchmarks)
- [10. Project Structure](#10-project-structure)

---

## 1. Architecture Overview

RustKV implements the classic **Bitcask** architecture, designed for high-write-throughput workloads:

```text
                        ┌──────────────────────────────────────────────┐
                        │              Client / REPL                   │
                        └───────┬──────────────────────────────▲───────┘
                                │ SET / DELETE                 │ GET
                                ▼                              │
                        ┌──────────────────┐                   │
                        │     KvEngine     │                   │
                        └───┬──────────┬───┘                   │
             Append Record  │          │ Update Index          │
                            ▼          ▼                       │
    ┌─────────────────────────┐      ┌─────────────────────────┴───────┐
    │     Write-Ahead Log     │      │        In-Memory Index          │
    │   (Append-Only File)    │      │         (std HashMap)           │
    │                         │      │                                 │
    │ [Header|Key|Val] offset ◄──────┤ key -> IndexEntry {             │
    │ [Header|Key|Val]        │      │   record_offset: u64,           │
    │ [Header|Key|Val]        │      │   value_offset:  u64,           │
    │ [Header|Key|Val]        │      │   value_len:     u32,           │
    │ ...                     │      │   timestamp:     u64            │
    └─────────────────────────┘      │ }                               │
                                     └─────────────────────────────────┘
```

### Key Principles

1. **Append-Only Disk Log:** Every write (`SET`) and removal (`DELETE`) appends a binary record to the end of the log file. No in-place disk overwrites occur, ensuring all disk writes are strictly sequential.
2. **In-Memory Hash Index:** An in-memory hash map stores all active keys pointing to their exact physical file location (`IndexEntry`).
3. **Single Disk Seek Lookups:** Because the index tracks the exact `value_offset` and `value_len`, a `GET` reads only the value bytes using positional I/O (`read_exact_at` on Linux), bypassing the header and key completely.
4. **$O(1)$ Existence Checks:** An `EXISTS` check is resolved entirely in-memory in nanoseconds without touching the disk.

---

## 2. Binary Record Layout

RustKV avoids slow, fragile text/delimiter formats in favor of a robust, length-prefixed binary format with integrity verification:

```text
 0               1               2               3
 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|       Magic (0x52, 0x4B)      | RecordType(1) |   Flags (1)   |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
+                    Timestamp (8 bytes, u64)                   +
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                     Key Length (4 bytes, u32)                 |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                    Value Length (4 bytes, u32)                |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                  CRC-32 Checksum (4 bytes, u32)               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                     Key Payload (KeyLen bytes)                |
|                              ...                              |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                    Value Payload (ValLen bytes)               |
|                              ...                              |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### Field Definitions

| Field | Size | Description |
| :--- | :--- | :--- |
| **Magic Bytes** | 2 bytes | Fixed framing sentinel: `[0x52, 0x4B]` (ASCII `"RK"`). |
| **Record Type** | 1 byte | `0x01` = `Set` (Insert/Update), `0x02` = `Delete` (Tombstone). |
| **Flags** | 1 byte | Reserved for compression or format versioning (default: `0x00`). |
| **Timestamp** | 8 bytes | Milliseconds since Unix epoch (`u64`, little-endian). |
| **Key Length** | 4 bytes | Byte count of the key payload (`u32`, little-endian). |
| **Value Length** | 4 bytes | Byte count of the value payload (`u32`, little-endian). |
| **Checksum** | 4 bytes | IEEE 802.3 CRC-32 computed over all fields & payload (`u32`, little-endian). |
| **Key Payload** | Variable | Raw UTF-8 or arbitrary binary key bytes. |
| **Value Payload** | Variable | Raw binary value payload. |

### Memory Safety Guardrails
Before allocating memory for keys or values during read or recovery, RustKV enforces hard safety bounds:
- `MAX_KEY_SIZE`: 64 KiB
- `MAX_VALUE_SIZE`: 64 MiB

This prevents corrupted length fields from triggering out-of-memory (OOM) crashes.

---

## 3. Crash Recovery and Corruption Handling

When RustKV opens an existing log file, it replays all records sequentially from offset `0` to reconstruct the in-memory index.

```text
[ Record 1: SET k1=v1 ] -> index.insert("k1", offset)
[ Record 2: SET k2=v2 ] -> index.insert("k2", offset)
[ Record 3: SET k1=v3 ] -> index.insert("k1", offset) (overwrites previous pointer)
[ Record 4: DEL k2    ] -> index.remove("k2")         (k2 remains deleted)
```

### The Crash Recovery Dilemma: Trailing Incomplete vs. Mid-Log Corruption

A core challenge in database crash recovery is distinguishing between an **incomplete trailing write** and **mid-log data corruption**:

1. **Incomplete Trailing Record (Safe Truncation):**
   - If the database host loses power while writing the last record, the log file may terminate prematurely in the middle of a 24-byte header or mid-payload.
   - Because no subsequent transactions committed, it is mathematically safe to truncate the file back to the last valid record boundary.
   - RustKV reports the truncated byte count in `RecoveryReport::truncated_bytes` and resumes normal operations.

2. **Mid-Log Corruption (Fatal Error):**
   - If a byte flips in record #2, but valid records exist at later offsets (e.g., records #3 and #4), this is **not** an in-flight crash — it represents data corruption or disk decay!
   - RustKV **never silently discards** mid-log corruption. It aborts startup with an explicit `KvError::LogCorrupted` or `KvError::ChecksumMismatch`, protecting the user from silent data loss.

---

## 4. Durability & Linux File I/O Internals

When a database calls `write()`, where does the data go?

```text
User Space:      db.set(k, v)
                      │
                      ▼ write()
Kernel Space:    Linux Page Cache (Dirty Pages)  <-- Fast, protected against process crash
                      │
                      ▼ fdatasync() / fsync()
Hardware:        Disk Controller Cache
                      │
                      ▼ Physical Media Flush
Storage:         Non-Volatile Storage (SSD / NVMe Flash)  <-- Protected against power loss
```

RustKV distinguishes three levels of write safety via `SyncPolicy`:

| Policy | Implementation | Durability Guarantee | Throughput |
| :--- | :--- | :--- | :--- |
| `SyncPolicy::Always` | Calls `fdatasync()` on every write | **ACID Durable:** Immune to power loss and OS crashes. | ~1,000–5,000 ops/s (bounded by disk fsync latency) |
| `SyncPolicy::EveryN(N)` | Flushes to disk every $N$ operations | **Bounded Loss:** At most $N-1$ operations lost on power loss. | ~50,000–150,000 ops/s |
| `SyncPolicy::Never` | Relies on Linux kernel dirty page writeback | **OS Buffered:** Immune to app crashes; lost on hardware crash. | **>300,000 ops/s** |

Users can also execute an explicit `SYNC` command at any time to flush dirty pages.

---

## 5. Online Log Compaction

Because the log is append-only, updates and deletions cause the log to accumulate stale, dead records over time.

RustKV includes an **Online Compactor** (`COMPACT`):
1. Creates a temporary compacted log file (`data.compact`).
2. Scans the current active keys in the in-memory index.
3. Copies only the active, latest records from the old log to the compacted log.
4. Performs an `fsync` on the new file.
5. Atomically swaps the new log in place of the old log using Linux `rename(2)`.
6. Updates index offsets in-place.

Dead space from superseded keys and tombstone records is reclaimed with zero downtime.

---

## 6. Supported Commands

RustKV provides an interactive REPL with full quoted-string support:

| Command | Syntax | Description |
| :--- | :--- | :--- |
| `SET` | `SET <key> <value>` | Insert or update a key. Quotes supported: `SET "user:1" "John Doe"`. |
| `GET` | `GET <key>` | Retrieve a value by key. |
| `DELETE` | `DELETE <key>` | Delete a key (writes a tombstone record). |
| `EXISTS` | `EXISTS <key>` | Check if a key exists ($O(1)$ in-memory lookup). |
| `SCAN` | `SCAN [prefix]` | List all keys, or keys starting with an optional prefix. |
| `SYNC` | `SYNC` | Flush and sync operating system buffers to disk (`fsync`). |
| `COMPACT` | `COMPACT` | Trigger log compaction and reclaim dead space. |
| `STATS` | `STATS` | View live engine statistics (active keys, log size, durability). |
| `HELP` | `HELP` | Display interactive command help. |
| `EXIT` | `EXIT` (or `QUIT`) | Cleanly close database and exit. |

---

## 7. Getting Started & Build Instructions

### Prerequisites
- Rust stable toolchain (`rustc` and `cargo` $\ge$ 1.80).
- Supported OS: Linux (including Arch Linux, Ubuntu, Debian, Fedora), macOS.

### Building
```bash
# Clone the repository
git clone https://github.com/deepak-pandey2005/RustKV.git
cd RustKV

# Build in release mode
cargo build --release
```

The compiled binary will be located at `target/release/rustkv`.

### Running the Interactive REPL
```bash
# Launch interactive REPL with default database (rustkv.db)
cargo run --release

# Or specify a custom database path and durability policy
cargo run --release -- --path ./mydb.db --sync always
```

### Running One-Off CLI Commands
```bash
# Set a key
cargo run -- --path ./mydb.db SET "greeting" "Hello, Database World!"

# Retrieve the key
cargo run -- --path ./mydb.db GET "greeting"

# Check if key exists
cargo run -- --path ./mydb.db EXISTS "greeting"

# Scan keys
cargo run -- --path ./mydb.db SCAN

# View database statistics
cargo run -- --path ./mydb.db STATS
```

---

## 8. Running Tests

RustKV includes a test suite covering unit tests, crash recovery with partial writes, mid-log corruption detection, and online log compaction:

```bash
# Run all tests (unit tests, integration tests, doctests)
cargo test

# Run specific corruption recovery tests
cargo test --test corruption_tests

# Run compaction tests
cargo test --test compaction_tests
```

---

## 9. Performance Benchmarks

Run the built-in benchmark to measure raw read/write throughput and recovery performance:

```bash
cargo run --example benchmark --release
```

### Sample Benchmark Results (Linux, NVMe SSD)

```text
=======================================================
               RustKV Performance Benchmark           
=======================================================

Benchmarking with 50,000 records...

>>> 1. OS-Buffered Writes (SyncPolicy::Never)
  - Inserted 50,000 records in 149.08ms (335,384 ops/sec)

>>> 2. Random Read Throughput (GET)
  - Completed 25,000 random disk reads in 102.18ms (244,658 ops/sec)
  - Pre-compaction log file size: 4.34 MB

>>> 3. Online Compaction
  - Compacted log in 499.29ms. Preserved 50,000 keys

>>> 4. Cold Startup Recovery
  - Replayed 50,000 records in 221.31ms (225,930 records/sec)
```

---

## 10. Project Structure

```text
RustKV/
├── Cargo.toml                  # Package manifest & metadata
├── README.md                   # Complete architectural guide & documentation
├── src/
│   ├── lib.rs                  # Public library exports & crate-level documentation
│   ├── main.rs                 # CLI entry point, flag parsing & REPL launcher
│   ├── crc.rs                  # Compile-time zero-dependency IEEE 802.3 CRC-32 implementation
│   ├── error.rs                # Typed errors (KvError, Result, ChecksumMismatch, LogCorrupted)
│   ├── record.rs               # Length-prefixed binary record format & header serialization
│   ├── index.rs                # In-memory hash index mapping keys to disk coordinates
│   ├── wal.rs                  # Append-only write-ahead log, positional reads & crash recovery
│   ├── engine.rs               # Storage engine (KvEngine), durability policies & compaction
│   └── cli.rs                  # Command parser, tokenizer, execution & interactive REPL
├── tests/
│   ├── integration_tests.rs    # CRUD, persistence across restart, overwrite, prefix scan
│   ├── corruption_tests.rs     # Partial header/payload crash recovery & mid-log bit corruption
│   └── compaction_tests.rs     # Dead space reclamation & post-compaction integrity
└── examples/
    └── benchmark.rs            # Performance benchmark measuring ops/sec
```

---

## License

Dual-licensed under either of:
- Apache License, Version 2.0 ([LICENSE-APACHE](http://www.apache.org/licenses/LICENSE-2.0))
- MIT license ([LICENSE-MIT](http://opensource.org/licenses/MIT))

at your option.
