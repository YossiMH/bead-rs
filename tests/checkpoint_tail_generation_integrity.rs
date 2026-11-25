//! A selected set-v2 generation remains recoverable across the next publish.

use rusqlite::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn run(workspace: &Path, args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_bead"))
        .args(args)
        .current_dir(workspace)
        .output()
        .expect("run bead");
    assert!(
        output.status.success(),
        "bead {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn read_json(file: &Path) -> Value {
    serde_json::from_slice(&fs::read(file).expect("read JSON")).expect("JSON")
}

fn selected_generation_survives_append(legacy_fixed_tail: bool) {
    let source = TempDir::new().expect("source workspace");
    run(
        source.path(),
        &["init", "--prefix", "tail", "--skip-foreign-workspace"],
    );
    run(source.path(), &["create", "--title", "retained issue"]);
    let checkpoint = source.path().join(".beads/checkpoint");
    let current = checkpoint.join("current.json");
    let mut selected = read_json(&current);
    assert_eq!(selected["mode"], "set-v2");
    let manifest_path = checkpoint.join(selected["active_root"]["path"].as_str().unwrap());
    let mut manifest = read_json(&manifest_path);
    let initial_tail = checkpoint.join(manifest["tail"]["path"].as_str().unwrap());
    let tail_bytes = fs::read(&initial_tail).expect("initial tail");
    assert!(!tail_bytes.is_empty());

    if legacy_fixed_tail {
        // Fixture-only recreation of the old writer's exact layout. Keep the
        // pointer and live bookkeeping consistent; the next real publication
        // must preserve this selected legacy generation without rewriting it.
        fs::write(checkpoint.join("set/tail.jsonl"), &tail_bytes).unwrap();
        manifest["tail"]["path"] = Value::String("set/tail.jsonl".into());
        let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        let sha = format!("{:x}", Sha256::digest(&bytes));
        fs::write(&manifest_path, bytes).unwrap();
        selected["active_root"]["sha256"] = Value::String(sha.clone());
        fs::write(&current, serde_json::to_vec_pretty(&selected).unwrap()).unwrap();
        let conn = Connection::open(source.path().join(".beads/beads.db")).unwrap();
        conn.execute(
            "UPDATE checkpoint_state SET current_root_sha256 = ?1 WHERE id = 1",
            [&sha],
        )
        .unwrap();
    }

    let selected_pointer_bytes = fs::read(&current).unwrap();
    let selected_manifest_bytes = fs::read(&manifest_path).unwrap();
    let selected_tail_path = checkpoint.join(manifest["tail"]["path"].as_str().unwrap());
    let original_config = fs::read(source.path().join(".beads/config.json")).unwrap();
    let before_inventory = run(source.path(), &["list", "--all", "--json"]).stdout;
    let generation = selected["generation_id"].as_str().unwrap();

    if legacy_fixed_tail {
        let verified = TempDir::new().expect("legacy fixture verification target");
        let report: Value = serde_json::from_slice(
            &run(
                verified.path(),
                &[
                    "restore",
                    "--source",
                    checkpoint.to_str().unwrap(),
                    "--generation",
                    generation,
                    "--actor",
                    "legacy-fixture-verification",
                    "--format",
                    "json",
                ],
            )
            .stdout,
        )
        .unwrap();
        assert_eq!(report["generation_id"], selected["generation_id"]);
        assert_eq!(report["source_root_sha256"], selected["active_root"]["sha256"]);
        assert_eq!(report["issues_restored"], selected["issue_count"]);
        assert_eq!(report["non_empty_override"], false);
        let verified_inventory = run(verified.path(), &["list", "--all", "--json"]).stdout;
        assert_eq!(
            serde_json::from_slice::<Value>(&verified_inventory).unwrap(),
            serde_json::from_slice::<Value>(&before_inventory).unwrap(),
            "the legacy fixture must restore correctly before the append"
        );
        assert_eq!(fs::read(&current).unwrap(), selected_pointer_bytes);
        assert_eq!(fs::read(&manifest_path).unwrap(), selected_manifest_bytes);
        assert_eq!(fs::read(&selected_tail_path).unwrap(), tail_bytes);
    }

    // A small append changes the open tail without crossing the seal bound.
    // Old code overwrote set/tail.jsonl here and made this generation unusable.
    run(
        source.path(),
        &["create", "--title", "later issue", "--no-auto-flush"],
    );
    run(source.path(), &["sync", "flush-only"]);
    let advanced = read_json(&current);
    assert_ne!(advanced["generation_id"], selected["generation_id"]);
    assert_eq!(
        advanced["issue_count"],
        selected["issue_count"].as_u64().unwrap() + 1
    );
    assert_eq!(
        fs::read(checkpoint.join("previous.json")).unwrap(),
        selected_pointer_bytes,
        "promotion must retain the exact selected pointer"
    );
    assert_eq!(fs::read(&manifest_path).unwrap(), selected_manifest_bytes);
    assert_eq!(
        fs::read(&selected_tail_path).unwrap(),
        tail_bytes,
        "selected tail bytes changed after append"
    );
    assert_eq!(
        fs::read(source.path().join(".beads/config.json")).unwrap(),
        original_config
    );
    let new_manifest = read_json(&checkpoint.join(advanced["active_root"]["path"].as_str().unwrap()));
    assert_ne!(new_manifest["tail"]["path"], manifest["tail"]["path"]);
    assert_eq!(
        new_manifest["segments"].as_array().unwrap().len(),
        manifest["segments"].as_array().unwrap().len(),
        "a small append must not change the sealing boundary"
    );

    let restored = TempDir::new().expect("empty recovery target");
    let report: Value = serde_json::from_slice(
        &run(
            restored.path(),
            &[
                "restore",
                "--source",
                checkpoint.to_str().unwrap(),
                "--generation",
                generation,
                "--actor",
                "tail-retention-test",
                "--format",
                "json",
            ],
        )
        .stdout,
    )
    .unwrap();
    assert_eq!(report["generation_id"], selected["generation_id"]);
    assert_eq!(report["source_root_sha256"], selected["active_root"]["sha256"]);
    assert_eq!(report["issues_restored"], selected["issue_count"]);
    assert_eq!(report["events_restored"], selected["event_count"]);
    assert_eq!(report["non_empty_override"], false);
    let recovered_inventory = run(restored.path(), &["list", "--all", "--json"]).stdout;
    assert_eq!(
        serde_json::from_slice::<Value>(&recovered_inventory).unwrap(),
        serde_json::from_slice::<Value>(&before_inventory).unwrap(),
        "recovery must select the saved generation, excluding the later issue"
    );
    assert_eq!(fs::read(&selected_tail_path).unwrap(), tail_bytes);
    assert_eq!(fs::read(&manifest_path).unwrap(), selected_manifest_bytes);

    if !legacy_fixed_tail {
        assert!(manifest["tail"]["path"]
            .as_str()
            .unwrap()
            .starts_with("set/tail/"));
        assert_eq!(
            manifest["tail"]["path"],
            format!(
                "set/tail/{}.jsonl",
                manifest["tail"]["sha256"].as_str().unwrap()
            )
        );
    }
}

#[test]
fn selected_set_v2_generation_remains_restorable_after_tail_append() {
    selected_generation_survives_append(false);
}

#[test]
fn legacy_fixed_tail_generation_remains_restorable_after_upgrade() {
    selected_generation_survives_append(true);
}
