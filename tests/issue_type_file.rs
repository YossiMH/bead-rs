//! UTF-8 type input must reach the existing guarded edit without changing its
//! transaction, audit, content checks or stored document shape.

use bead_rs::service::capabilities::Capabilities;
use bead_rs::service::get_issue_by_id;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Each command uses only its temporary store and an explicit test actor.
fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bead"));
    command
        .current_dir(root)
        .env("BEAD_WORKSPACE_ROOT", root)
        .env("BEAD_ORG_SECRET_SCANNER", "off")
        .env("BEAD_ACTOR", "type-file-test")
        .env_remove("NEEDLE_BEAD_ID")
        .env_remove("NEEDLE_BEAD_FENCING_TOKEN")
        .args(["--skip-foreign-workspace", "--no-auto-flush"]);
    command
}

/// Retain both streams so argument errors remain visible when an assertion fails.
fn run(root: &Path, arguments: &[&str]) -> Output {
    command(root).args(arguments).output().unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Decode only a successful public CLI response.
fn json_output(root: &Path, arguments: &[&str]) -> Value {
    let output = run(root, arguments);
    assert_success(&output);
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The temporary directory owns the store and text files until test teardown.
fn workspace() -> (tempfile::TempDir, String) {
    let root = tempfile::tempdir().unwrap();
    assert_success(&run(root.path(), &["init", "--prefix", "type"]));
    let created = json_output(
        root.path(),
        &[
            "create",
            "--title",
            "Type input",
            "--issue-type",
            "task",
            "--json",
        ],
    );
    (root, created["id"].as_str().unwrap().to_owned())
}

/// Read the full native row, including the type omitted by compact CLI reads.
fn stored_issue(root: &Path, id: &str) -> Value {
    let connection = Connection::open(root.join(".beads/beads.db")).unwrap();
    serde_json::to_value(get_issue_by_id(&connection, id).unwrap().unwrap()).unwrap()
}

/// Snapshot saved fields and complete public audit records, excluding the feed's
/// observation timestamp so refusals and no-ops can be compared exactly.
fn snapshot(root: &Path, id: &str) -> (Value, Value) {
    (
        stored_issue(root, id),
        json_output(root, &["changes", "--since", "0", "--json"])["mutations"].clone(),
    )
}

/// Exercise the public file transport together with any other update fields.
fn file_update(root: &Path, id: &str, path: &Path, arguments: &[&str]) -> Output {
    command(root)
        .args(["update", id, "--issue-type-file"])
        .arg(path)
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn long_unicode_file_commits_one_guarded_edit_and_preserves_claim() {
    let (workspace, id) = workspace();
    let root = workspace.path();
    let claim = json_output(root, &["claim", "--assignee", "type-worker", "--json"]);
    assert_eq!(claim["bead_id"], id);
    let token = claim["claim_epoch"].as_i64().unwrap().to_string();
    let before = snapshot(root, &id);
    let revision = before.0["revision"].as_i64().unwrap();
    let text = format!("--סוג מותאם\r\n{}\n", "שורת סוג custom\r\n".repeat(3000));
    assert!(text.chars().count() > 32_768);
    let type_file = root.join("type.txt");
    let description_file = root.join("description.txt");
    let notes_file = root.join("notes.txt");
    fs::write(&type_file, &text).unwrap();
    fs::write(&description_file, "--description\r\nתיאור\n").unwrap();
    fs::write(&notes_file, "notes\r\nשורה\n").unwrap();
    let output = command(root)
        .args(["update", &id, "--issue-type-file"])
        .arg(&type_file)
        .arg("--description-file")
        .arg(&description_file)
        .arg("--notes-file")
        .arg(&notes_file)
        .args([
            "--priority",
            "1",
            "--reason",
            "Reviewed type and priority",
            "--fencing-token",
            &token,
            "--if-revision",
            &revision.to_string(),
        ])
        .output()
        .unwrap();
    assert_success(&output);
    let after = snapshot(root, &id);
    assert_eq!(after.0["issue_type"], text);
    assert_eq!(after.0["description"], "--description\r\nתיאור\n");
    assert_eq!(after.0["notes"], "notes\r\nשורה\n");
    assert_eq!(after.0["priority"], 1);
    assert_eq!(after.0["revision"], revision + 1);
    for field in [
        "id",
        "title",
        "created_at",
        "assignee",
        "claim_epoch",
        "base_status",
    ] {
        assert_eq!(after.0[field], before.0[field], "{field}");
    }
    let events = after.1.as_array().unwrap();
    let added = &events[before.1.as_array().unwrap().len()..];
    assert_eq!(added.len(), 2);
    assert_eq!(added[0]["kind"], "updated");
    assert_eq!(added[0]["detail"]["revision"], revision + 1);
    assert_eq!(added[1]["kind"], "priority_changed");
    assert_eq!(added[1]["actor"], "type-file-test");
    assert_eq!(added[1]["detail"]["actor"], "type-file-test");
    assert_eq!(added[1]["detail"]["old_priority"], 2);
    assert_eq!(added[1]["detail"]["new_priority"], 1);
    assert_eq!(added[1]["detail"]["reason"], "Reviewed type and priority");
    assert_eq!(added[1]["detail"]["revision"], revision + 1);
}

#[test]
fn stdin_type_preserves_exact_text() {
    let (workspace, id) = workspace();
    let root = workspace.path();
    let text = "--סוג stdin\r\nlast line\n";
    let mut child = command(root)
        .args(["update", &id, "--issue-type-file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let written = child.stdin.take().unwrap().write_all(text.as_bytes());
    let output = child.wait_with_output().unwrap();
    assert_success(&output);
    written.unwrap();
    assert_eq!(stored_issue(root, &id)["issue_type"], text);
}

#[test]
fn inline_and_file_conflict_before_reading_the_file() {
    let (workspace, id) = workspace();
    let root = workspace.path();
    let before = snapshot(root, &id);
    let output = file_update(
        root,
        &id,
        &root.join("missing.txt"),
        &["--issue-type", "inline"],
    );
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("mutually exclusive"));
    assert_eq!(snapshot(root, &id), before);
}

#[test]
fn only_one_text_field_may_consume_stdin() {
    let (workspace, id) = workspace();
    let root = workspace.path();
    let before = snapshot(root, &id);
    for fields in [
        ["--description-file", "--issue-type-file"],
        ["--notes-file", "--issue-type-file"],
        ["--description-file", "--notes-file"],
    ] {
        let output = command(root)
            .args(["update", &id, fields[0], "-", fields[1], "-"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("only one text field may read stdin")
        );
        assert_eq!(snapshot(root, &id), before);
    }
    let output = file_update(
        root,
        &id,
        Path::new("-"),
        &["--notes-file", "-", "--description-file", "missing.txt"],
    );
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("only one text field may read stdin"));
    assert_eq!(snapshot(root, &id), before);
}

#[test]
fn missing_invalid_utf8_and_empty_files_leave_the_edit_unapplied() {
    let (workspace, id) = workspace();
    let root = workspace.path();
    fs::write(root.join("invalid.txt"), [0xff, 0xfe]).unwrap();
    fs::write(root.join("empty.txt"), b"").unwrap();
    let before = snapshot(root, &id);
    for (name, code, message) in [
        ("missing.txt", 1, "cannot read text input"),
        ("invalid.txt", 2, "text input is not valid UTF-8"),
        ("empty.txt", 2, "issue_type cannot be empty"),
    ] {
        let output = file_update(root, &id, &root.join(name), &["--title", "must not save"]);
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{output:?}"
        );
        assert_eq!(snapshot(root, &id), before);
    }
}

#[test]
fn file_edits_enforce_revision_and_claim_guards_even_for_noops() {
    let (workspace, id) = workspace();
    let root = workspace.path();
    let claim = json_output(root, &["claim", "--assignee", "type-worker", "--json"]);
    let token = claim["claim_epoch"].as_i64().unwrap().to_string();
    let path = root.join("type.txt");
    fs::write(&path, "task").unwrap();
    let before = snapshot(root, &id);
    for arguments in [
        vec![],
        vec!["--fencing-token", &token, "--if-revision", "0"],
    ] {
        let output = file_update(root, &id, &path, &arguments);
        assert_eq!(output.status.code(), Some(4), "{output:?}");
        assert_eq!(snapshot(root, &id), before);
    }
    fs::write(&path, "replacement").unwrap();
    let output = file_update(root, &id, &path, &["--title", "must not save"]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert_eq!(snapshot(root, &id), before);
    let output = file_update(
        root,
        &id,
        &path,
        &["--fencing-token", &token, "--if-revision", "0"],
    );
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert_eq!(snapshot(root, &id), before);
}

#[test]
fn dry_run_and_repeated_file_values_preserve_saved_state() {
    let (workspace, id) = workspace();
    let root = workspace.path();
    let path = root.join("type.txt");
    fs::write(&path, "task").unwrap();
    let before = snapshot(root, &id);
    assert_success(&file_update(root, &id, &path, &[]));
    assert_eq!(snapshot(root, &id), before);
    fs::write(&path, "--preview\r\nסוג\n").unwrap();
    let output = file_update(root, &id, &path, &["--dry-run", "--title", "Preview title"]);
    assert_success(&output);
    let preview: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(preview["semantic_change"], true);
    assert_eq!(preview["before"]["issue_type"], "task");
    assert_eq!(preview["after"]["issue_type"], "--preview\r\nסוג\n");
    assert_eq!(preview["after"]["title"], "Preview title");
    assert_eq!(snapshot(root, &id), before);
}

#[test]
#[cfg(not(feature = "managed-secret-policy"))]
fn file_type_is_scanned_before_any_scalar_edit() {
    let (workspace, id) = workspace();
    let root = workspace.path();
    let synthetic = ["AK", "IA", "7Q9W2E4R6T8Y1U3I"].concat();
    let path = root.join("type.txt");
    fs::write(&path, &synthetic).unwrap();
    let before = snapshot(root, &id);
    let output = file_update(root, &id, &path, &["--title", "must not save"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("secret_detected"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(&synthetic));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&synthetic));
    assert_eq!(snapshot(root, &id), before);
}

#[test]
fn capability_schema_accepts_the_optional_type_transport() {
    let (workspace, _) = workspace();
    let root = workspace.path();
    let capabilities = json_output(root, &["capabilities"]);
    let editing = &capabilities["issue_editing"];
    assert_eq!(editing["version"], 1);
    assert_eq!(editing["issue_type_file"], true);
    let schema = json_output(
        root,
        &[
            "schema",
            "show",
            capabilities["schema_ref"].as_str().unwrap(),
            "--format",
            "json",
        ],
    );
    let editing_schema = &schema["properties"]["issue_editing"];
    assert_eq!(editing_schema["additionalProperties"], false);
    assert_eq!(
        editing_schema["properties"]["issue_type_file"]["type"],
        "boolean"
    );
    let required = editing_schema["required"].as_array().unwrap();
    assert_eq!(
        required,
        json!([
            "version",
            "fields",
            "clear_description",
            "revision_guard",
            "priority_reason_required"
        ])
        .as_array()
        .unwrap()
    );
    assert!(!required.contains(&json!("issue_type_file")));
    for field in editing.as_object().unwrap().keys() {
        assert!(editing_schema["properties"].get(field).is_some(), "{field}");
    }
    let mut previous = capabilities.clone();
    previous["issue_editing"]
        .as_object_mut()
        .unwrap()
        .remove("issue_type_file");
    for field in required {
        assert!(previous["issue_editing"]
            .get(field.as_str().unwrap())
            .is_some());
    }
    let decoded: Capabilities = serde_json::from_value(previous).unwrap();
    let decoded = serde_json::to_value(decoded).unwrap();
    assert_ne!(decoded["issue_editing"]["issue_type_file"], true);
    assert_eq!(decoded["issue_editing"]["fields"], editing["fields"]);
}

#[test]
fn help_and_generated_man_page_describe_type_file_input() {
    let (workspace, _) = workspace();
    let output = run(workspace.path(), &["update", "--help"]);
    assert_success(&output);
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--issue-type-file"));
    let pages = tempfile::tempdir().unwrap();
    bead_rs::docs::generate_man_pages(pages.path()).unwrap();
    let generated = fs::read_to_string(pages.path().join("bead-update.1")).unwrap();
    assert!(generated.replace("\\-", "-").contains("--issue-type-file"));
    let tracked = include_str!("../man/man1/bead-update.1");
    assert!(tracked.replace("\\-", "-").contains("--issue-type-file"));
}

#[test]
fn stored_issue_and_manifest_keep_literal_type_text() {
    let (workspace, id) = workspace();
    let root = workspace.path();
    let schema = json_output(
        root,
        &[
            "schema",
            "show",
            "urn:bead-rs:schema:issue:native-v1",
            "--format",
            "json",
        ],
    );
    assert!(schema["properties"].get("issue_type_file").is_none());
    let manifest = root.join("manifest.json");
    fs::write(
        &manifest,
        json!({"manifest_version":1,"operations":[{"op":"update","id":id,"issue_type":"literal-type.txt"}]}).to_string(),
    )
    .unwrap();
    let output = command(root)
        .args(["manifest", "commit", "--input"])
        .arg(&manifest)
        .output()
        .unwrap();
    assert_success(&output);
    let before = snapshot(root, &id);
    assert_eq!(before.0["issue_type"], "literal-type.txt");
    assert!(before.0.get("issue_type_file").is_none());
    fs::write(
        &manifest,
        json!({"manifest_version":1,"operations":[{"op":"update","id":id,"issue_type_file":"literal-type.txt"}]}).to_string(),
    )
    .unwrap();
    let output = command(root)
        .args(["manifest", "commit", "--input"])
        .arg(&manifest)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown field"));
    assert_eq!(snapshot(root, &id), before);
}
