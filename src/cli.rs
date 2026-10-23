//! Command-Line Interface and Interactive REPL for RustKV.
//!
//! Provides command parsing, execution, interactive terminal REPL,
//! and batch command support.

use crate::engine::KvEngine;
use crate::error::Result;
use std::io::{self, BufRead, Write};
use std::time::Instant;

/// Supported database commands.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Set { key: String, value: String },
    Get { key: String },
    Delete { key: String },
    Exists { key: String },
    Scan { prefix: Option<String> },
    Sync,
    Compact,
    Stats,
    Help,
    Exit,
}

impl Command {
    /// Parses a raw user input line into a `Command`.
    pub fn parse(input: &str) -> std::result::Result<Self, String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err("Empty command".to_string());
        }

        let tokens = tokenize(trimmed)?;
        if tokens.is_empty() {
            return Err("Empty command".to_string());
        }

        let cmd_name = tokens[0].to_uppercase();
        match cmd_name.as_str() {
            "SET" => {
                if tokens.len() < 2 {
                    return Err("Usage: SET <key> <value>".to_string());
                }
                let key = tokens[1].clone();
                // If there are more than 2 tokens and value wasn't quoted, join the rest as value
                let value = if tokens.len() == 2 {
                    String::new()
                } else if tokens.len() == 3 {
                    tokens[2].clone()
                } else {
                    // Combine trailing tokens as value
                    tokens[2..].join(" ")
                };

                if key.is_empty() {
                    return Err("Key cannot be empty".to_string());
                }
                Ok(Command::Set { key, value })
            }
            "GET" => {
                if tokens.len() < 2 {
                    return Err("Usage: GET <key>".to_string());
                }
                Ok(Command::Get {
                    key: tokens[1].clone(),
                })
            }
            "DELETE" | "DEL" => {
                if tokens.len() < 2 {
                    return Err("Usage: DELETE <key>".to_string());
                }
                Ok(Command::Delete {
                    key: tokens[1].clone(),
                })
            }
            "EXISTS" => {
                if tokens.len() < 2 {
                    return Err("Usage: EXISTS <key>".to_string());
                }
                Ok(Command::Exists {
                    key: tokens[1].clone(),
                })
            }
            "SCAN" => {
                let prefix = if tokens.len() >= 2 {
                    Some(tokens[1].clone())
                } else {
                    None
                };
                Ok(Command::Scan { prefix })
            }
            "SYNC" | "FLUSH" => Ok(Command::Sync),
            "COMPACT" => Ok(Command::Compact),
            "STATS" | "INFO" => Ok(Command::Stats),
            "HELP" => Ok(Command::Help),
            "EXIT" | "QUIT" => Ok(Command::Exit),
            unknown => Err(format!(
                "Unknown command: '{}'. Type 'HELP' to see available commands.",
                unknown
            )),
        }
    }
}

/// Tokenizes an input string, properly respecting quoted strings (`"..."` or `'...'`).
fn tokenize(input: &str) -> std::result::Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    let mut current = String::new();

    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }

        if ch == '"' || ch == '\'' {
            let quote_char = ch;
            chars.next(); // consume opening quote
            current.clear();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == quote_char {
                    closed = true;
                    break;
                } else if c == '\\' {
                    if let Some(escaped) = chars.next() {
                        current.push(escaped);
                    }
                } else {
                    current.push(c);
                }
            }
            if !closed {
                return Err(format!(
                    "Unterminated quoted string starting with {}",
                    quote_char
                ));
            }
            tokens.push(current.clone());
            current.clear();
        } else {
            current.clear();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                current.push(c);
                chars.next();
            }
            tokens.push(current.clone());
            current.clear();
        }
    }

    Ok(tokens)
}

/// Executes a single command on the engine and prints the formatted response.
pub fn execute_command(engine: &mut KvEngine, cmd: Command) -> Result<bool> {
    match cmd {
        Command::Set { key, value } => {
            let start = Instant::now();
            engine.set(key.as_bytes(), value.as_bytes())?;
            println!("OK ({} µs)", start.elapsed().as_micros());
        }
        Command::Get { key } => {
            let start = Instant::now();
            match engine.get(key.as_bytes())? {
                Some(val) => {
                    let text = String::from_utf8_lossy(&val);
                    println!("\"{}\" ({} µs)", text, start.elapsed().as_micros());
                }
                None => {
                    println!("(nil) ({} µs)", start.elapsed().as_micros());
                }
            }
        }
        Command::Delete { key } => {
            let start = Instant::now();
            let deleted = engine.delete(key.as_bytes())?;
            if deleted {
                println!("(integer) 1 [deleted] ({} µs)", start.elapsed().as_micros());
            } else {
                println!(
                    "(integer) 0 [key not found] ({} µs)",
                    start.elapsed().as_micros()
                );
            }
        }
        Command::Exists { key } => {
            let exists = engine.exists(key.as_bytes());
            if exists {
                println!("(integer) 1 [true]");
            } else {
                println!("(integer) 0 [false]");
            }
        }
        Command::Scan { prefix } => {
            let pref_bytes = prefix.as_deref().map(|p| p.as_bytes());
            let keys = engine.scan(pref_bytes);
            if keys.is_empty() {
                println!("(empty set)");
            } else {
                for (idx, k) in keys.iter().enumerate() {
                    let k_str = String::from_utf8_lossy(k);
                    println!("{}: \"{}\"", idx + 1, k_str);
                }
            }
        }
        Command::Sync => {
            let start = Instant::now();
            engine.sync()?;
            println!(
                "OK, flushed and fsynced to disk ({} µs)",
                start.elapsed().as_micros()
            );
        }
        Command::Compact => {
            let start = Instant::now();
            let stats = engine.compact()?;
            println!(
                "Compaction completed in {} ms:\n  - Old size: {} bytes\n  - New size: {} bytes\n  - Space reclaimed: {} bytes\n  - Preserved keys: {}",
                start.elapsed().as_millis(),
                stats.old_log_size,
                stats.new_log_size,
                stats.reclaimed_bytes,
                stats.keys_preserved
            );
        }
        Command::Stats => {
            let stats = engine.stats();
            println!("=== RustKV Database Statistics ===");
            println!("Active live keys:     {}", stats.active_keys);
            println!(
                "Log file size on disk: {} bytes ({:.2} KiB)",
                stats.log_file_size,
                stats.log_file_size as f64 / 1024.0
            );
            println!("Active key payload:   {} bytes", stats.active_key_bytes);
            println!("Active value payload: {} bytes", stats.active_value_bytes);
            println!("Unsynced operations:  {}", stats.uncommitted_ops);
            println!("Sync durability:      {:?}", stats.sync_policy);
            println!("Database file:        {}", engine.path().display());
        }
        Command::Help => {
            print_help();
        }
        Command::Exit => {
            println!("Goodbye!");
            return Ok(false); // Signal REPL to terminate
        }
    }
    Ok(true)
}

/// Prints formatted database help and command reference.
pub fn print_help() {
    println!(
        r#"
RustKV Interactive Commands:
  SET <key> <value>    Insert or update a key with a value.
                       Quotes are supported: SET "user:1" "Alice Smith"
  GET <key>            Retrieve the value of a key from disk.
  DELETE <key>         Remove a key (writes a tombstone to log).
  EXISTS <key>         Check whether a key exists (in-memory lookup).
  SCAN [prefix]        List all keys, or keys beginning with [prefix].
  SYNC                 Explicitly flush operating-system buffers to disk (fsync).
  COMPACT              Reclaim dead space from overwritten and deleted records.
  STATS                Display database storage engine statistics.
  HELP                 Display this help message.
  EXIT (or QUIT)       Cleanly close the database and exit.
"#
    );
}

/// Runs the interactive Read-Eval-Print Loop (REPL).
pub fn run_repl(engine: &mut KvEngine) -> Result<()> {
    println!("RustKV Database Engine v0.1.0");
    println!("Data file: {}", engine.path().display());
    println!("Type 'HELP' for available commands or 'EXIT' to quit.\n");

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!("rustkv> ");
        stdout.flush()?;

        let mut line = String::new();
        let bytes_read = stdin.lock().read_line(&mut line)?;
        if bytes_read == 0 {
            // EOF reached (Ctrl+D)
            println!("\nGoodbye!");
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match Command::parse(trimmed) {
            Ok(cmd) => match execute_command(engine, cmd) {
                Ok(continue_running) => {
                    if !continue_running {
                        break;
                    }
                }
                Err(err) => {
                    eprintln!("Error executing command: {}", err);
                }
            },
            Err(err_msg) => {
                eprintln!("Error: {}", err_msg);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_commands() {
        assert_eq!(
            Command::parse("SET mykey myvalue").unwrap(),
            Command::Set {
                key: "mykey".to_string(),
                value: "myvalue".to_string(),
            }
        );

        assert_eq!(
            Command::parse("SET \"key with spaces\" \"value with spaces\"").unwrap(),
            Command::Set {
                key: "key with spaces".to_string(),
                value: "value with spaces".to_string(),
            }
        );

        assert_eq!(
            Command::parse("GET user:100").unwrap(),
            Command::Get {
                key: "user:100".to_string(),
            }
        );

        assert_eq!(
            Command::parse("DELETE user:100").unwrap(),
            Command::Delete {
                key: "user:100".to_string(),
            }
        );

        assert_eq!(
            Command::parse("EXISTS user:100").unwrap(),
            Command::Exists {
                key: "user:100".to_string(),
            }
        );

        assert_eq!(
            Command::parse("SCAN user:").unwrap(),
            Command::Scan {
                prefix: Some("user:".to_string()),
            }
        );

        assert_eq!(
            Command::parse("SCAN").unwrap(),
            Command::Scan { prefix: None }
        );

        assert_eq!(Command::parse("HELP").unwrap(), Command::Help);
        assert_eq!(Command::parse("EXIT").unwrap(), Command::Exit);
        assert_eq!(Command::parse("QUIT").unwrap(), Command::Exit);
        assert_eq!(Command::parse("COMPACT").unwrap(), Command::Compact);
        assert_eq!(Command::parse("STATS").unwrap(), Command::Stats);
    }

    #[test]
    fn test_missing_args_error() {
        assert!(Command::parse("SET").is_err());
        assert!(Command::parse("GET").is_err());
        assert!(Command::parse("DELETE").is_err());
        assert!(Command::parse("EXISTS").is_err());
    }
}
