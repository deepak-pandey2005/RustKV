use rustkv::KvEngine;
use std::fs;
use std::path::PathBuf;

fn temp_db_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let unique = format!(
        "rustkv_compact_{}_{}_{}.db",
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
fn test_compaction_reclaims_dead_space() {
    let path = temp_db_path("reclaim");
    let _guard = TestDbGuard { path: path.clone() };

    let mut db = KvEngine::open(&path).expect("Open db");

    // Perform 300 overwrites across 3 keys
    for round in 0..100 {
        db.set(b"account:1", format!("balance_{}", round).as_bytes())
            .unwrap();
        db.set(b"account:2", format!("balance_{}", round).as_bytes())
            .unwrap();
        db.set(b"account:3", format!("balance_{}", round).as_bytes())
            .unwrap();
    }

    // Add a key and then delete it
    db.set(b"ephemeral_key", b"will_be_deleted").unwrap();
    db.delete(b"ephemeral_key").unwrap();

    let pre_stats = db.stats();
    assert_eq!(pre_stats.active_keys, 3);
    assert!(pre_stats.log_file_size > 10_000); // 302 records in log file

    // Perform compaction
    let comp_stats = db.compact().expect("Compaction should succeed");

    assert_eq!(comp_stats.keys_preserved, 3);
    assert!(comp_stats.new_log_size < comp_stats.old_log_size);
    assert!(comp_stats.reclaimed_bytes > 0);

    let post_stats = db.stats();
    assert_eq!(post_stats.active_keys, 3);
    assert_eq!(post_stats.log_file_size, comp_stats.new_log_size);

    // Verify latest values are preserved
    assert_eq!(db.get(b"account:1").unwrap(), Some(b"balance_99".to_vec()));
    assert_eq!(db.get(b"account:2").unwrap(), Some(b"balance_99".to_vec()));
    assert_eq!(db.get(b"account:3").unwrap(), Some(b"balance_99".to_vec()));
    assert_eq!(db.get(b"ephemeral_key").unwrap(), None);

    // Drop engine and reopen from disk to verify recovery from compacted file
    drop(db);

    let reopened = KvEngine::open(&path).expect("Reopen compacted db");
    assert_eq!(reopened.stats().active_keys, 3);
    assert_eq!(
        reopened.get(b"account:1").unwrap(),
        Some(b"balance_99".to_vec())
    );
    assert_eq!(
        reopened.get(b"account:2").unwrap(),
        Some(b"balance_99".to_vec())
    );
    assert_eq!(
        reopened.get(b"account:3").unwrap(),
        Some(b"balance_99".to_vec())
    );
    assert_eq!(reopened.get(b"ephemeral_key").unwrap(), None);
}
