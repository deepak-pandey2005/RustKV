use rustkv::{Config, KvEngine, SyncPolicy};
use std::fs;
use std::path::PathBuf;

/// Helper function to create a unique temporary database file for isolated testing.
fn temp_db_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let unique = format!(
        "rustkv_test_{}_{}_{}.db",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    path.push(unique);
    path
}

struct TestDbGuard {
    path: PathBuf,
}

impl Drop for TestDbGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.path.with_extension("compact"));
    }
}

#[test]
fn test_crud_operations() {
    let path = temp_db_path("crud");
    let _guard = TestDbGuard { path: path.clone() };

    let mut db = KvEngine::open(&path).expect("Failed to open database");

    // Key doesn't exist initially
    assert_eq!(db.get(b"user:1").unwrap(), None);
    assert!(!db.exists(b"user:1"));

    // Insert key
    db.set(b"user:1", b"Alice").unwrap();
    assert!(db.exists(b"user:1"));
    assert_eq!(db.get(b"user:1").unwrap(), Some(b"Alice".to_vec()));

    // Update key (overwrite)
    db.set(b"user:1", b"Alice Cooper").unwrap();
    assert_eq!(db.get(b"user:1").unwrap(), Some(b"Alice Cooper".to_vec()));

    // Delete key
    let deleted = db.delete(b"user:1").unwrap();
    assert!(deleted);
    assert!(!db.exists(b"user:1"));
    assert_eq!(db.get(b"user:1").unwrap(), None);

    // Deleting again returns false
    let deleted_again = db.delete(b"user:1").unwrap();
    assert!(!deleted_again);
}

#[test]
fn test_restart_and_recovery() {
    let path = temp_db_path("restart");
    let _guard = TestDbGuard { path: path.clone() };

    // Phase 1: Populate database
    {
        let mut db = KvEngine::open(&path).expect("Open db");
        db.set(b"k1", b"v1").unwrap();
        db.set(b"k2", b"v2").unwrap();
        db.set(b"k3", b"v3").unwrap();
        // Overwrite k2
        db.set(b"k2", b"v2_updated").unwrap();
        // Delete k1
        db.delete(b"k1").unwrap();
    } // db is dropped here, closing the file handle

    // Phase 2: Reopen database and verify reconstructed state
    {
        let db = KvEngine::open(&path).expect("Reopen db");
        let report = db.recovery_report();
        assert_eq!(report.valid_records, 5); // 3 inserts + 1 overwrite + 1 delete
        assert_eq!(report.set_count, 4);
        assert_eq!(report.delete_count, 1);
        assert_eq!(report.truncated_bytes, 0);

        // k1 was deleted: must remain deleted
        assert_eq!(db.get(b"k1").unwrap(), None);
        assert!(!db.exists(b"k1"));

        // k2 was updated: must have latest value
        assert_eq!(db.get(b"k2").unwrap(), Some(b"v2_updated".to_vec()));

        // k3 was never touched: must remain intact
        assert_eq!(db.get(b"k3").unwrap(), Some(b"v3".to_vec()));
    }
}

#[test]
fn test_prefix_scan() {
    let path = temp_db_path("scan");
    let _guard = TestDbGuard { path: path.clone() };

    let mut db = KvEngine::open(&path).expect("Open db");

    db.set(b"order:100", b"item A").unwrap();
    db.set(b"user:bob", b"profile").unwrap();
    db.set(b"order:101", b"item B").unwrap();
    db.set(b"order:102", b"item C").unwrap();
    db.set(b"user:alice", b"profile").unwrap();

    // Scan all
    let all = db.scan(None);
    assert_eq!(all.len(), 5);

    // Scan prefix "order:"
    let orders = db.scan(Some(b"order:"));
    assert_eq!(
        orders,
        vec![
            b"order:100".to_vec(),
            b"order:101".to_vec(),
            b"order:102".to_vec()
        ]
    );

    // Scan prefix "user:"
    let users = db.scan(Some(b"user:"));
    assert_eq!(users, vec![b"user:alice".to_vec(), b"user:bob".to_vec()]);

    // Scan non-existent prefix
    let empty = db.scan(Some(b"product:"));
    assert!(empty.is_empty());
}

#[test]
fn test_large_value_storage() {
    let path = temp_db_path("large_val");
    let _guard = TestDbGuard { path: path.clone() };

    let mut db = KvEngine::open(&path).expect("Open db");

    // 256 KiB payload
    let large_value = vec![0xABu8; 256 * 1024];
    db.set(b"big_payload", &large_value).unwrap();

    let fetched = db.get(b"big_payload").unwrap().expect("Value must exist");
    assert_eq!(fetched, large_value);
}

#[test]
fn test_sync_policies() {
    let path = temp_db_path("sync_policy");
    let _guard = TestDbGuard { path: path.clone() };

    let config = Config::new(&path).with_sync_policy(SyncPolicy::EveryN(5));
    let mut db = KvEngine::open_with_config(config).unwrap();

    for i in 0..12 {
        let key = format!("key_{}", i);
        let val = format!("val_{}", i);
        db.set(key.as_bytes(), val.as_bytes()).unwrap();
    }

    // 12 ops with EveryN(5) leaves 2 uncommitted ops since last sync
    let stats = db.stats();
    assert_eq!(stats.uncommitted_ops, 2);

    // Explicit sync resets uncommitted ops to 0
    db.sync().unwrap();
    assert_eq!(db.stats().uncommitted_ops, 0);
}
