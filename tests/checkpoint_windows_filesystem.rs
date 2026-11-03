//! Run with rustc --test on the real host filesystem; no mocks or stubs.
use bead_rs::service::checkpoint_io;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn directory(case: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os("BEAD_TEST_TMP").expect("BEAD_TEST_TMP required"))
        .join(format!(
            "{case}-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir_all(&path).expect("create isolated test directory");
    path
}

#[test]
fn closed_written_checkpoint_can_flush_without_changing_its_bytes() {
    let path = directory("flush-closed-file").join("checkpoint.jsonl");
    let expected = b"checkpoint contains real committed tasks and events\n";
    fs::write(&path, expected).expect("write checkpoint");
    checkpoint_io::sync_written_file(&path)
        .expect("checkpoint must flush on the actual host filesystem");
    assert_eq!(fs::read(path).expect("read flushed file"), expected);
}

#[test]
fn atomic_checkpoint_publication_replaces_the_previous_pointer_and_flushes() {
    let dir = directory("publish-pointer");
    let source = dir.join("current.tmp");
    let destination = dir.join("current.json");
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
    let dir = directory("failed-publication");
    let destination = dir.join("current.json");
    fs::write(&destination, b"known good checkpoint\n").expect("previous pointer");
    let result = checkpoint_io::replace_durable_file(&dir.join("missing.tmp"), &destination);
    assert_eq!(
        result.expect_err("missing source must fail").kind(),
        std::io::ErrorKind::NotFound
    );
    assert_eq!(
        fs::read(destination).expect("old checkpoint"),
        b"known good checkpoint\n"
    );
}
