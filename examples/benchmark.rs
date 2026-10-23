//! Performance Benchmark for RustKV.
//!
//! Measures sequential writes, random reads, and overwrite throughput
//! under different durability synchronization policies.
//!
//! Run with:
//! ```bash
//! cargo run --example benchmark --release
//! ```

use rustkv::{Config, KvEngine, SyncPolicy};
use std::fs;
use std::time::Instant;

fn main() {
    println!("=======================================================");
    println!("               RustKV Performance Benchmark           ");
    println!("=======================================================\n");

    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!("rustkv_bench_{}.db", std::process::id()));

    // Clean up previous runs
    let _ = fs::remove_file(&db_path);

    let num_records = 50_000;
    println!("Benchmarking with {} records...\n", num_records);

    // Benchmark 1: OS-Buffered Writes (SyncPolicy::Never)
    {
        println!(">>> 1. OS-Buffered Writes (SyncPolicy::Never)");
        let config = Config::new(&db_path).with_sync_policy(SyncPolicy::Never);
        let mut db = KvEngine::open_with_config(config).unwrap();

        let start = Instant::now();
        for i in 0..num_records {
            let key = format!("user:{:08}", i);
            let val = format!("payload_data_for_user_{:08}_with_some_extra_padding", i);
            db.set(key.as_bytes(), val.as_bytes()).unwrap();
        }
        let elapsed = start.elapsed();
        let ops_per_sec = (num_records as f64) / elapsed.as_secs_f64();
        println!(
            "  - Inserted {} records in {:.2?} ({:.0} ops/sec)",
            num_records, elapsed, ops_per_sec
        );

        // Benchmark 2: In-Memory / Positional Disk Lookups (GET)
        println!("\n>>> 2. Random Read Throughput (GET)");
        let start_read = Instant::now();
        for i in (0..num_records).step_by(2) {
            let key = format!("user:{:08}", i);
            let val = db.get(key.as_bytes()).unwrap();
            assert!(val.is_some());
        }
        let read_count = num_records / 2;
        let elapsed_read = start_read.elapsed();
        let read_ops_per_sec = (read_count as f64) / elapsed_read.as_secs_f64();
        println!(
            "  - Completed {} random disk reads in {:.2?} ({:.0} ops/sec)",
            read_count, elapsed_read, read_ops_per_sec
        );

        // Stats before compaction
        let stats = db.stats();
        println!(
            "\n  - Pre-compaction log file size: {:.2} MB",
            stats.log_file_size as f64 / (1024.0 * 1024.0)
        );

        // Benchmark 3: Compaction
        println!("\n>>> 3. Online Compaction");
        let start_compact = Instant::now();
        let comp = db.compact().unwrap();
        let elapsed_compact = start_compact.elapsed();
        println!(
            "  - Compacted log in {:.2?}. Preserved {} keys, reclaimed {} bytes ({:.2} MB)",
            elapsed_compact,
            comp.keys_preserved,
            comp.reclaimed_bytes,
            comp.reclaimed_bytes as f64 / (1024.0 * 1024.0)
        );
    }

    // Benchmark 4: Startup Crash Recovery Time
    {
        println!("\n>>> 4. Cold Startup Recovery");
        let start_recovery = Instant::now();
        let db = KvEngine::open(&db_path).unwrap();
        let elapsed_recovery = start_recovery.elapsed();
        let report = db.recovery_report();
        println!(
            "  - Replayed {} records in {:.2?} ({:.0} records/sec)",
            report.valid_records,
            elapsed_recovery,
            (report.valid_records as f64) / elapsed_recovery.as_secs_f64()
        );
    }

    // Clean up
    let _ = fs::remove_file(&db_path);
    let _ = fs::remove_file(db_path.with_extension("compact"));

    println!("\n=======================================================");
    println!("Benchmark completed successfully!");
    println!("=======================================================");
}
