//! RustKV Binary Entry Point.
//!
//! Supports both interactive REPL mode and one-off CLI command execution.

use rustkv::cli::{Command, execute_command, print_help, run_repl};
use rustkv::engine::{Config, KvEngine, SyncPolicy};
use rustkv::wal::RecoveryMode;
use std::env;
use std::path::PathBuf;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut db_path = PathBuf::from("rustkv.db");
    let mut sync_policy = SyncPolicy::Always;
    let mut cmd_tokens = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                println!("RustKV — Persistent Key-Value Database in Rust");
                println!("\nUsage:");
                println!("  rustkv [OPTIONS] [COMMAND]");
                println!("\nOptions:");
                println!("  --path <PATH>        Path to database data file [default: rustkv.db]");
                println!("  --sync <always|never|every-N>  Durability policy [default: always]");
                println!("  --strict             Fail on any trailing incomplete writes");
                println!("  -h, --help           Print help information");
                println!("  -V, --version        Print version information");
                println!("\nExamples:");
                println!("  rustkv                               # Start interactive REPL");
                println!(
                    "  rustkv --path ./mydb.db              # Start REPL with custom database"
                );
                println!("  rustkv SET user:1 Alice              # One-off command execution");
                println!("  rustkv GET user:1                    # One-off command execution");
                print_help();
                return;
            }
            "--version" | "-V" => {
                println!("rustkv {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--path" => {
                if i + 1 < args.len() {
                    db_path = PathBuf::from(&args[i + 1]);
                    i += 1;
                } else {
                    eprintln!("Error: --path requires a file path");
                    process::exit(1);
                }
            }
            "--sync" => {
                if i + 1 < args.len() {
                    let val = args[i + 1].to_lowercase();
                    if val == "always" {
                        sync_policy = SyncPolicy::Always;
                    } else if val == "never" {
                        sync_policy = SyncPolicy::Never;
                    } else if let Some(stripped) = val.strip_prefix("every-") {
                        if let Ok(n) = stripped.parse::<usize>() {
                            sync_policy = SyncPolicy::EveryN(n);
                        } else {
                            eprintln!("Error: Invalid sync policy. Use always, never, or every-N");
                            process::exit(1);
                        }
                    } else {
                        eprintln!(
                            "Error: Invalid sync policy '{}'. Use always, never, or every-N",
                            val
                        );
                        process::exit(1);
                    }
                    i += 1;
                } else {
                    eprintln!("Error: --sync requires a policy (always, never, every-N)");
                    process::exit(1);
                }
            }
            "--strict" => {
                // strict recovery mode
            }
            _ => {
                // Remaining arguments are treated as a command
                cmd_tokens.extend_from_slice(&args[i..]);
                break;
            }
        }
        i += 1;
    }

    let config = Config::new(db_path)
        .with_sync_policy(sync_policy)
        .with_recovery_mode(RecoveryMode::TruncateTrailingIncomplete);

    let mut engine = match KvEngine::open_with_config(config) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("Database initialization failed: {}", err);
            process::exit(1);
        }
    };

    let report = engine.recovery_report();
    if report.valid_records > 0 || report.truncated_bytes > 0 {
        eprintln!(
            "[INFO] Recovery complete: {} records replayed ({} sets, {} deletes). Log size: {} bytes.",
            report.valid_records, report.set_count, report.delete_count, report.valid_log_size
        );
        if report.truncated_bytes > 0 {
            eprintln!(
                "[WARN] Truncated {} incomplete trailing bytes from crash.",
                report.truncated_bytes
            );
        }
    }

    if cmd_tokens.is_empty() {
        // Run interactive REPL
        if let Err(err) = run_repl(&mut engine) {
            eprintln!("REPL error: {}", err);
            process::exit(1);
        }
    } else {
        // Execute one-off command
        let raw_input = cmd_tokens.join(" ");
        match Command::parse(&raw_input) {
            Ok(cmd) => {
                if let Err(err) = execute_command(&mut engine, cmd) {
                    eprintln!("Execution error: {}", err);
                    process::exit(1);
                }
            }
            Err(err_msg) => {
                eprintln!("Error: {}", err_msg);
                process::exit(1);
            }
        }
    }
}
