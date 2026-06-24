//! Integration tests for the storage engine: durability across reopen and
//! crash recovery from the WAL.

use squrust_core::{BTree, StorageEngine};

/// Encode the root page id in page 1's header-adjacent scratch so we can find
/// the table again after reopening. For these tests we instead use a fixed,
/// known layout: the first write transaction always creates the tree as the
/// first allocated page (page 2).
const ROOT: u32 = 2;

#[test]
fn write_close_reopen_10k_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.db");

    {
        let engine = StorageEngine::open(&path).unwrap();
        let tx = engine.begin_write();
        let root = BTree::create(&tx).unwrap();
        assert_eq!(root, ROOT, "first allocated page should be 2");
        let tree = BTree::open(root);
        for i in 0..10_000i64 {
            tree.insert(&tx, i, format!("row-number-{i}").as_bytes())
                .unwrap();
        }
        tx.commit().unwrap();
        engine.checkpoint().unwrap();
        engine.sync().unwrap();
    }

    // Reopen from disk and verify everything is present.
    let engine = StorageEngine::open(&path).unwrap();
    let tx = engine.begin_read();
    let tree = BTree::open(ROOT);
    for i in 0..10_000i64 {
        let v = tree.get(&tx, i).unwrap().unwrap();
        assert_eq!(v, format!("row-number-{i}").as_bytes());
    }
    assert_eq!(tree.get(&tx, 10_000).unwrap(), None);
}

#[test]
fn crash_recovery_from_wal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crash.db");

    {
        // Write rows and commit (fsyncs the WAL) but DO NOT checkpoint.
        // Then drop the engine without folding the WAL into the main file,
        // simulating a crash after a durable commit.
        let engine = StorageEngine::open(&path).unwrap();
        let tx = engine.begin_write();
        let root = BTree::create(&tx).unwrap();
        let tree = BTree::open(root);
        for i in 0..2000i64 {
            tree.insert(&tx, i, format!("v{i}").as_bytes()).unwrap();
        }
        tx.commit().unwrap();
        // No checkpoint: the data lives only in the WAL.
        drop(engine);
    }

    // Reopen: the WAL must be replayed so reads see committed data.
    let engine = StorageEngine::open(&path).unwrap();
    {
        let tx = engine.begin_read();
        let tree = BTree::open(ROOT);
        for i in 0..2000i64 {
            assert_eq!(tree.get(&tx, i).unwrap().unwrap(), format!("v{i}").as_bytes());
        }
    }
    // Now checkpoint and reopen again to confirm the data also survives the
    // fold into the main file.
    engine.checkpoint().unwrap();
    engine.sync().unwrap();
    drop(engine);

    let engine = StorageEngine::open(&path).unwrap();
    let tx = engine.begin_read();
    let tree = BTree::open(ROOT);
    assert_eq!(tree.get(&tx, 1999).unwrap().unwrap(), b"v1999");
}

#[test]
fn partial_write_is_discarded() {
    // A WAL whose tail is a half-written (uncommitted) transaction must be
    // truncated on replay, leaving only durably committed data.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partial.db");
    let wal_path = {
        let mut s = path.clone().into_os_string();
        s.push("-squrust-wal");
        std::path::PathBuf::from(s)
    };

    {
        let engine = StorageEngine::open(&path).unwrap();
        let tx = engine.begin_write();
        let root = BTree::create(&tx).unwrap();
        let tree = BTree::open(root);
        tree.insert(&tx, 1, b"committed").unwrap();
        tx.commit().unwrap();
        drop(engine);
    }

    // Append garbage (a partial frame) to the end of the WAL.
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&wal_path)
        .unwrap();
    f.write_all(&[0xFFu8; 100]).unwrap();
    f.sync_all().unwrap();
    drop(f);

    // Reopen: the committed row survives, the garbage tail is dropped.
    let engine = StorageEngine::open(&path).unwrap();
    let tx = engine.begin_read();
    let tree = BTree::open(ROOT);
    assert_eq!(tree.get(&tx, 1).unwrap().unwrap(), b"committed");
}
