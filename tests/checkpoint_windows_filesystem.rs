//! Check checkpoint durability on the host filesystem.
use bead_rs::service::checkpoint_io;
use std::fs;
#[test]
fn closed_written_checkpoint_can_flush_without_changing_its_bytes() {
    let dir = tempfile::tempdir().expect("create isolated test directory");
    let path = dir.path().join("checkpoint.jsonl");
    let expected = b"checkpoint contains real committed tasks and events\n";
    fs::write(&path, expected).expect("write checkpoint");
    checkpoint_io::sync_written_file(&path)
        .expect("checkpoint must flush on the actual host filesystem");
    assert_eq!(fs::read(path).expect("read flushed file"), expected);
}

#[test]
fn atomic_checkpoint_publication_replaces_the_previous_pointer_and_flushes() {
    let dir = tempfile::tempdir().expect("create isolated test directory");
    let source = dir.path().join("current.tmp");
    let destination = dir.path().join("current.json");
    fs::write(&destination, b"previous checkpoint\n").expect("previous pointer");
    fs::write(&source, b"complete next checkpoint\n").expect("next pointer");
    checkpoint_io::replace_durable_file(&source, &destination)
        .expect("durable replacement must work on the actual host filesystem");
    assert_eq!(
        fs::read(destination).expect("new pointer"),
        b"complete next checkpoint\n"
    );
    assert_eq!(
        fs::read(source).expect_err("source must be moved").kind(),
        std::io::ErrorKind::NotFound
    );
}

#[test]
fn publication_failure_preserves_the_previous_checkpoint() {
    let dir = tempfile::tempdir().expect("create isolated test directory");
    let destination = dir.path().join("current.json");
    fs::write(&destination, b"known good checkpoint\n").expect("previous pointer");
    let result = checkpoint_io::replace_durable_file(&dir.path().join("missing.tmp"), &destination);
    assert_eq!(
        result.expect_err("missing source must fail").kind(),
        std::io::ErrorKind::NotFound
    );
    assert_eq!(
        fs::read(destination).expect("old checkpoint"),
        b"known good checkpoint\n"
    );
}
