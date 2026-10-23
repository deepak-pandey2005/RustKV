use rustkv::record::{HEADER_SIZE, Record};
use rustkv::{KvEngine, KvError};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

fn temp_db_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let unique = format!(
        "rustkv_corrupt_{}_{}_{}.db",
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
fn test_recovery_from_trailing_incomplete_header() {
    let path = temp_db_path("trailing_header");
    let _guard = TestDbGuard { path: path.clone() };

    // Step 1: Write 3 valid records
    {
        let mut db = KvEngine::open(&path).unwrap();
        db.set(b"alpha", b"111").unwrap();
        db.set(b"beta", b"222").unwrap();
        db.set(b"gamma", b"333").unwrap();
    }

    let initial_size = fs::metadata(&path).unwrap().len();

    // Step 2: Append 10 bytes of garbage to simulate a power outage during header write
    {
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"1234567890").unwrap();
        f.flush().unwrap();
    }

    assert_eq!(fs::metadata(&path).unwrap().len(), initial_size + 10);

    // Step 3: Reopen database. It must safely truncate the partial header and recover the 3 records
    {
        let mut db = KvEngine::open(&path).expect("Safe recovery should succeed");
        let report = db.recovery_report();
        assert_eq!(report.valid_records, 3);
        assert_eq!(report.truncated_bytes, 10);
        assert_eq!(db.get(b"alpha").unwrap(), Some(b"111".to_vec()));
        assert_eq!(db.get(b"beta").unwrap(), Some(b"222".to_vec()));
        assert_eq!(db.get(b"gamma").unwrap(), Some(b"333".to_vec()));

        // Check that further writes succeed cleanly
        db.set(b"delta", b"444").unwrap();
        assert_eq!(db.get(b"delta").unwrap(), Some(b"444".to_vec()));
    }
}

#[test]
fn test_recovery_from_trailing_incomplete_payload() {
    let path = temp_db_path("trailing_payload");
    let _guard = TestDbGuard { path: path.clone() };

    // Step 1: Write 2 valid records
    {
        let mut db = KvEngine::open(&path).unwrap();
        db.set(b"k1", b"v1").unwrap();
        db.set(b"k2", b"v2").unwrap();
    }

    // Step 2: Create a record with a 60-byte payload, but only write 20 bytes before abrupt EOF
    {
        let incomplete_rec = Record::new_set(b"cut_short".to_vec(), vec![0xEE; 60]).unwrap();
        let serialized = incomplete_rec.serialize();
        // Write only header + 20 bytes (missing 40 bytes)
        let partial_bytes = &serialized[..HEADER_SIZE + 20];

        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(partial_bytes).unwrap();
        f.flush().unwrap();
    }

    // Step 3: Reopen database. It must safely detect EOF mid-record, truncate it, and recover k1, k2
    {
        let db = KvEngine::open(&path).expect("Recovery should succeed");
        let report = db.recovery_report();
        assert_eq!(report.valid_records, 2);
        assert_eq!(report.truncated_bytes, (HEADER_SIZE + 20) as u64);
        assert_eq!(db.get(b"k1").unwrap(), Some(b"v1".to_vec()));
        assert_eq!(db.get(b"k2").unwrap(), Some(b"v2".to_vec()));
        assert_eq!(db.get(b"cut_short").unwrap(), None);
    }
}

#[test]
fn test_mid_log_payload_corruption_detected() {
    let path = temp_db_path("mid_log_corrupt");
    let _guard = TestDbGuard { path: path.clone() };

    // Write 5 records
    {
        let mut db = KvEngine::open(&path).unwrap();
        db.set(b"rec_1", b"value 1").unwrap();
        db.set(b"rec_2", b"value 2").unwrap();
        db.set(b"rec_3", b"value 3").unwrap();
        db.set(b"rec_4", b"value 4").unwrap();
        db.set(b"rec_5", b"value 5").unwrap();
    }

    // Corrupt a byte in the 2nd record (offset roughly 50-80)
    {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        // Seek into the payload of the 2nd record
        let corrupt_offset = HEADER_SIZE + 5 + 7 + 10; // past record 1 into record 2
        f.seek(SeekFrom::Start(corrupt_offset as u64)).unwrap();
        let mut byte = [0u8; 1];
        f.read_exact(&mut byte).unwrap();
        byte[0] ^= 0xFF; // flip bits
        f.seek(SeekFrom::Start(corrupt_offset as u64)).unwrap();
        f.write_all(&byte).unwrap();
        f.flush().unwrap();
    }

    // Reopening the database MUST fail because the corruption is in the middle of the log!
    let open_res = KvEngine::open(&path);
    assert!(
        open_res.is_err(),
        "Database open should fail when mid-log corruption is detected"
    );

    let err = open_res.unwrap_err();
    match err {
        KvError::LogCorrupted { .. } | KvError::ChecksumMismatch { .. } => {
            // Expected!
        }
        other => panic!(
            "Expected LogCorrupted or ChecksumMismatch, got: {:?}",
            other
        ),
    }
}

#[test]
fn test_mid_log_magic_byte_corruption_detected() {
    let path = temp_db_path("magic_corrupt");
    let _guard = TestDbGuard { path: path.clone() };

    // Write 3 records
    {
        let mut db = KvEngine::open(&path).unwrap();
        db.set(b"item:1", b"data1").unwrap();
        db.set(b"item:2", b"data2").unwrap();
        db.set(b"item:3", b"data3").unwrap();
    }

    // Corrupt the magic byte of record 2
    {
        let rec1_size = HEADER_SIZE + 6 + 5;
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        f.seek(SeekFrom::Start(rec1_size as u64)).unwrap();
        // Overwrite magic byte with 0x00
        f.write_all(&[0x00, 0x00]).unwrap();
        f.flush().unwrap();
    }

    let open_res = KvEngine::open(&path);
    assert!(open_res.is_err());
    assert!(matches!(
        open_res.unwrap_err(),
        KvError::LogCorrupted { .. }
    ));
}
