//! Existing-bead editing through the public CLI, manifests, and checkpoints.
//!
//! Each test owns a disposable native store. The requested edits must preserve
//! issue identity and related records, and one failed field or stale editor
//! must leave the complete edit unapplied.

use bead_rs::service::get_issue_by_id;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Barrier};

/// Construct a command bound to its test store, without publishing checkpoints
/// unless a test explicitly selects the publishing form.
fn command(workspace: &Path) -> Command {
    let mut command = publishing_command(workspace);
    command.arg("--no-auto-flush");
    command
}

/// Keep inherited worker credentials and optional organization scanners from
/// selecting a different actor or store. Native content checks remain enabled.
fn publishing_command(workspace: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bead"));
    command
        .current_dir(workspace)
        .env("BEAD_WORKSPACE_ROOT", workspace)
        .env("BEAD_ORG_SECRET_SCANNER", "off")
        .env("BEAD_ACTOR", "editing-test")
        .env_remove("NEEDLE_BEAD_ID")
        .env_remove("NEEDLE_BEAD_FENCING_TOKEN")
        .arg("--skip-foreign-workspace");
    command
}

/// Run a real CLI process and retain both output streams for failed assertions.
fn run(workspace: &Path, arguments: &[&str]) -> Output {
    command(workspace).args(arguments).output().unwrap()
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

/// Execute a successful CLI command and decode its documented JSON output.
fn json_output(workspace: &Path, arguments: &[&str]) -> Value {
    let output = run(workspace, arguments);
    assert_success(&output);
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The temp directory owns all store and subprocess files until test teardown.
fn workspace() -> tempfile::TempDir {
    let workspace = tempfile::tempdir().unwrap();
    assert_success(&run(workspace.path(), &["init", "--prefix", "edit"]));
    workspace
}

/// Create a bead through the CLI and return its generated stable identifier.
fn create_issue(workspace: &Path, title: &str) -> String {
    let output = run(workspace, &["create", "--title", title]);
    assert_success(&output);
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Read the full native row, including fields the compact CLI projection omits.
fn stored_issue(workspace: &Path, issue_id: &str) -> Value {
    let connection = Connection::open(workspace.join(".beads/beads.db")).unwrap();
    serde_json::to_value(get_issue_by_id(&connection, issue_id).unwrap().unwrap()).unwrap()
}

/// Read arbitrary extension records from their preserved native storage.
fn stored_extensions(workspace: &Path, issue_id: &str) -> Vec<(String, Value)> {
    let connection = Connection::open(workspace.join(".beads/beads.db")).unwrap();
    let mut statement = connection
        .prepare("SELECT key, value FROM issue_extensions WHERE issue_id = ?1 ORDER BY key")
        .unwrap();
    statement
        .query_map([issue_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .map(|row| {
            let (key, value) = row.unwrap();
            (key, serde_json::from_str(&value).unwrap())
        })
        .collect()
}

/// Capture semantic state and the publication pointer independently of the
/// command under test, so partial edits and unintended publications are visible.
#[derive(Debug, PartialEq)]
struct Snapshot {
    issue: Value,
    event_count: i64,
    pointer_bytes: Option<Vec<u8>>,
}

/// Read the issue, audit length, and published pointer without changing the store.
fn snapshot(workspace: &Path, issue_id: &str) -> Snapshot {
    let connection = Connection::open(workspace.join(".beads/beads.db")).unwrap();
    let event_count = connection
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .unwrap();
    let pointer_bytes = match fs::read(workspace.join(".beads/checkpoint/current.json")) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("checkpoint pointer could not be read: {error}"),
    };
    Snapshot {
        issue: stored_issue(workspace, issue_id),
        event_count,
        pointer_bytes,
    }
}

/// Audit records are read in commit order from the native event store.
fn event_details(workspace: &Path, issue_id: &str, kind: &str) -> Vec<Value> {
    let connection = Connection::open(workspace.join(".beads/beads.db")).unwrap();
    let mut statement = connection
        .prepare("SELECT detail FROM events WHERE issue_id = ?1 AND kind = ?2 ORDER BY sequence")
        .unwrap();
    statement
        .query_map([issue_id, kind], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
        .collect()
}

/// Write a native manifest for the public file-input command.
fn manifest_file(workspace: &Path, operations: Value) -> PathBuf {
    let path = workspace.join("editing-manifest.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({"manifest_version": 1, "operations": operations})).unwrap(),
    )
    .unwrap();
    path
}

/// Execute one atomic manifest request, retaining failures for rollback checks.
fn manifest(workspace: &Path, subcommand: &str, path: &Path) -> Output {
    command(workspace)
        .args(["manifest", subcommand, "--input"])
        .arg(path)
        .args(["--format", "json"])
        .output()
        .unwrap()
}

/// Import an independently written native record with an unknown extension;
/// this uses the public interchange path rather than writing SQLite directly.
fn import_extended_issue(workspace: &Path) -> String {
    let issue_id = "edit-imported";
    let path = workspace.join("extended-issue.jsonl");
    let issue = json!({
        "id": issue_id,
        "title": "Original imported task",
        "description": "Original description",
        "notes": "Original notes",
        "priority": 2,
        "base_status": "open",
        "manual_blocked": false,
        "issue_type": "task",
        "created_at": "2026-11-01T09:00:00Z",
        "updated_at": "2026-11-01T09:00:00Z",
        "revision": 1,
        "schema_ref": "urn:bead-rs:schema:issue:native-v1",
        "profile": "native-v1",
        "x_editor_extension": {"kept": [1, "two", null]}
    });
    fs::write(&path, format!("{issue}\n")).unwrap();
    let output = command(workspace)
        .args(["sync", "import-only", "--input"])
        .arg(path)
        .args(["--merge", "--actor", "editing-test"])
        .output()
        .unwrap();
    assert_success(&output);
    issue_id.to_owned()
}

#[test]
fn compound_edit_preserves_identity_extensions_and_related_records() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = import_extended_issue(root);
    let blocker_id = create_issue(root, "Existing blocker");
    for arguments in [
        vec!["label", "add", &issue_id, "kept-label"],
        vec!["dep", "add", &issue_id, &blocker_id, "--kind", "blocks"],
        vec![
            "comment",
            "create",
            &issue_id,
            "--body",
            "Keep this comment",
        ],
        vec![
            "ref",
            "add",
            "--id",
            &issue_id,
            "--namespace",
            "editing",
            "--key",
            "source",
            "--value",
            "record-one",
        ],
        vec![
            "data",
            "set",
            "--id",
            &issue_id,
            "--namespace",
            "editing",
            "--schema-ref",
            "urn:editing:fixture:v1",
            "--value",
            "{\"keep\":[1,2]}",
        ],
    ] {
        assert_success(&run(root, &arguments));
    }
    let before = stored_issue(root, &issue_id);
    let extensions_before = stored_extensions(root, &issue_id);
    assert_eq!(
        extensions_before,
        vec![(
            "x_editor_extension".to_owned(),
            json!({"kept": [1, "two", null]})
        )]
    );
    let shown_before = json_output(root, &["show", &issue_id, "--comments", "all", "--json"]);
    let refs_before = json_output(root, &["ref", "list", "--id", &issue_id, "--json"]);
    let data_before = json_output(
        root,
        &[
            "data",
            "get",
            "--id",
            &issue_id,
            "--namespace",
            "editing",
            "--json",
        ],
    );
    let revision = before["revision"].as_i64().unwrap().to_string();

    assert_success(&run(
        root,
        &[
            "update",
            &issue_id,
            "--title",
            "Changed title",
            "--description",
            "First line\nשורה שנייה",
            "--priority",
            "1",
            "--reason",
            "Reviewed urgency",
            "--issue-type",
            "custom-review",
            "--notes",
            "Changed notes",
            "--if-revision",
            &revision,
        ],
    ));

    let after = stored_issue(root, &issue_id);
    for field in ["id", "created_at", "schema_ref", "profile"] {
        assert_eq!(
            after[field], before[field],
            "changed unrelated field {field}"
        );
    }
    assert_eq!(stored_extensions(root, &issue_id), extensions_before);
    assert_eq!(after["title"], "Changed title");
    assert_eq!(after["description"], "First line\nשורה שנייה");
    assert_eq!(after["priority"], 1);
    assert_eq!(after["issue_type"], "custom-review");
    assert_eq!(after["notes"], "Changed notes");
    assert_eq!(
        after["revision"].as_i64(),
        Some(before["revision"].as_i64().unwrap() + 1)
    );
    let shown_after = json_output(root, &["show", &issue_id, "--comments", "all", "--json"]);
    assert_eq!(shown_after["title"], "Changed title");
    for field in ["labels", "dependencies", "comments"] {
        assert_eq!(shown_after[field], shown_before[field], "changed {field}");
    }
    assert_eq!(
        json_output(root, &["ref", "list", "--id", &issue_id, "--json"]),
        refs_before
    );
    assert_eq!(
        json_output(
            root,
            &[
                "data",
                "get",
                "--id",
                &issue_id,
                "--namespace",
                "editing",
                "--json"
            ]
        ),
        data_before
    );
}

#[test]
fn description_file_stdin_and_clear_preserve_exact_text() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Text transports");
    let path = root.join("description.txt");
    let description = "- opening line\nתיאור עם סימנים ✨\n";
    fs::write(&path, description).unwrap();
    let output = command(root)
        .args([
            "update",
            &issue_id,
            "--title",
            "- renamed",
            "--description-file",
        ])
        .arg(&path)
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(stored_issue(root, &issue_id)["description"], description);
    assert_eq!(stored_issue(root, &issue_id)["title"], "- renamed");

    let mut child = command(root)
        .args(["update", &issue_id, "--description-file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all("stdin\nתיאור".as_bytes())
        .unwrap();
    assert_success(&child.wait_with_output().unwrap());
    assert_eq!(stored_issue(root, &issue_id)["description"], "stdin\nתיאור");

    assert_success(&run(root, &["update", &issue_id, "--clear-description"]));
    assert_eq!(
        json_output(root, &["show", &issue_id, "--json"])["description"],
        ""
    );
    assert_success(&run(
        root,
        &[
            "update",
            &issue_id,
            "--description",
            "restored",
            "--notes",
            "a note",
        ],
    ));
    assert_success(&run(
        root,
        &["update", &issue_id, "--description", "", "--notes", ""],
    ));
    let shown = json_output(root, &["show", &issue_id, "--json"]);
    assert_eq!(shown["description"], "");
    assert_eq!(shown["notes"], "");
}

#[test]
fn conflicting_sources_and_invalid_utf8_do_not_modify_the_issue() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Text source refusal");
    let path = root.join("description.txt");
    fs::write(&path, b"from file").unwrap();
    let path_text = path.to_str().unwrap();
    let before = snapshot(root, &issue_id);
    for arguments in [
        vec!["--description", "inline", "--description-file", path_text],
        vec!["--description", "inline", "--clear-description"],
        vec!["--description-file", path_text, "--clear-description"],
    ] {
        let output = command(root)
            .args(["update", &issue_id, "--title", "must not save"])
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert_eq!(snapshot(root, &issue_id), before);
    }
    fs::write(&path, [0xff, 0xfe]).unwrap();
    let output = run(
        root,
        &["update", &issue_id, "--description-file", path_text],
    );
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("UTF-8"));
    assert_eq!(snapshot(root, &issue_id), before);
}

#[test]
fn invalid_compound_edits_leave_fields_events_and_checkpoint_unchanged() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Validation remains atomic");
    let before = snapshot(root, &issue_id);
    for arguments in [
        vec!["--title", ""],
        vec![
            "--title",
            "must not save",
            "--priority",
            "5",
            "--reason",
            "invalid range",
        ],
        vec!["--title", "must not save", "--issue-type", ""],
        vec!["--title", "must not save", "--status", "closed"],
    ] {
        let output = command(root)
            .args(["update", &issue_id])
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success(), "invalid edit was accepted");
        assert_eq!(snapshot(root, &issue_id), before);
    }
    let path = root.join("oversized-description.txt");
    fs::write(&path, "a".repeat(4 * 1024 * 1024 + 1)).unwrap();
    let output = command(root)
        .args([
            "update",
            &issue_id,
            "--title",
            "must not save",
            "--description-file",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(snapshot(root, &issue_id), before);
}

#[test]
fn title_limits_count_utf8_bytes() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Title limit");
    let maximum_title = "é".repeat(2048);
    assert_success(&run(
        root,
        &["update", &issue_id, "--title", &maximum_title],
    ));
    assert_eq!(stored_issue(root, &issue_id)["title"], maximum_title);
    let before = snapshot(root, &issue_id);
    let too_long = "é".repeat(2049);
    let output = run(root, &["update", &issue_id, "--title", &too_long]);
    assert!(!output.status.success());
    assert_eq!(snapshot(root, &issue_id), before);
}

#[test]
fn stale_revision_rejects_the_whole_edit() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Revision check");
    let revision = stored_issue(root, &issue_id)["revision"]
        .as_i64()
        .unwrap()
        .to_string();
    assert_success(&run(
        root,
        &["update", &issue_id, "--notes", "another editor"],
    ));
    let before = snapshot(root, &issue_id);
    let output = run(
        root,
        &[
            "update",
            &issue_id,
            "--title",
            "stale title",
            "--description",
            "stale description",
            "--if-revision",
            &revision,
        ],
    );
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Revision mismatch"));
    assert_eq!(snapshot(root, &issue_id), before);
}

#[test]
fn claimed_edits_require_the_issued_credential() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Claim credential check");
    let claim = json_output(root, &["claim", "--assignee", "editing-worker", "--json"]);
    assert_eq!(claim["bead_id"], issue_id);
    let epoch = claim["claim_epoch"].as_i64().unwrap();
    let before = snapshot(root, &issue_id);
    for arguments in [
        vec![],
        vec!["--fencing-token".to_owned(), (epoch + 1).to_string()],
    ] {
        let output = command(root)
            .args(["update", &issue_id, "--title", "held edit"])
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(4), "{output:?}");
        assert_eq!(snapshot(root, &issue_id), before);
    }
    assert_success(&run(
        root,
        &[
            "update",
            &issue_id,
            "--title",
            "held edit",
            "--fencing-token",
            &epoch.to_string(),
        ],
    ));
    let after = stored_issue(root, &issue_id);
    assert_eq!(after["title"], "held edit");
    assert_eq!(after["assignee"], before.issue["assignee"]);
    assert_eq!(after["claim_epoch"], before.issue["claim_epoch"]);
}

#[test]
fn concurrent_editors_with_one_revision_have_one_complete_winner() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Concurrent edits");
    let before = stored_issue(root, &issue_id);
    let revision = before["revision"].as_i64().unwrap().to_string();
    let barrier = Arc::new(Barrier::new(3));
    let workers: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|editor| {
            let root = root.to_path_buf();
            let issue_id = issue_id.clone();
            let revision = revision.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                let output = run(
                    &root,
                    &[
                        "update",
                        &issue_id,
                        "--title",
                        editor,
                        "--notes",
                        editor,
                        "--if-revision",
                        &revision,
                    ],
                );
                (editor, output)
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|(_, output)| output.status.success())
            .count(),
        1,
        "{results:?}"
    );
    assert_eq!(
        results
            .iter()
            .filter(|(_, output)| output.status.code() == Some(4))
            .count(),
        1,
        "{results:?}"
    );
    let winner = results
        .iter()
        .find(|(_, output)| output.status.success())
        .unwrap()
        .0;
    let after = stored_issue(root, &issue_id);
    assert_eq!(after["title"], winner);
    assert_eq!(after["notes"], winner);
    assert_eq!(
        after["revision"].as_i64(),
        Some(before["revision"].as_i64().unwrap() + 1)
    );
}

#[test]
fn priority_change_records_reason_actor_and_changes_ready_order() {
    let workspace = workspace();
    let root = workspace.path();
    create_issue(root, "Earlier task");
    let promoted = create_issue(root, "Promoted task");
    assert_success(&run(
        root,
        &[
            "update",
            &promoted,
            "--priority",
            "0",
            "--reason",
            "Urgent customer repair",
        ],
    ));
    let ready = json_output(root, &["list", "--ready", "--json"]);
    assert_eq!(ready[0]["id"], promoted);
    let events = event_details(root, &promoted, "priority_changed");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["old_priority"], 2);
    assert_eq!(events[0]["new_priority"], 0);
    assert_eq!(events[0]["actor"], "editing-test");
    assert_eq!(events[0]["reason"], "Urgent customer repair");
    let history = json_output(root, &["history", &promoted, "--json"]);
    assert!(history
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["kind"] == "priority_changed"));
}

#[test]
fn priority_requires_a_reason_and_same_value_has_no_priority_event() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Priority no-op");
    let before = snapshot(root, &issue_id);
    for reason_arguments in [vec![], vec!["--reason", "   "]] {
        let output = command(root)
            .args([
                "update",
                &issue_id,
                "--title",
                "must not save",
                "--priority",
                "1",
            ])
            .args(reason_arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("--reason"));
        assert_eq!(snapshot(root, &issue_id), before);
    }
    assert_success(&run(
        root,
        &[
            "update",
            &issue_id,
            "--priority",
            "2",
            "--reason",
            "Keep current urgency",
        ],
    ));
    assert_eq!(snapshot(root, &issue_id), before);
    assert_success(&run(
        root,
        &[
            "update",
            &issue_id,
            "--title",
            "new title only",
            "--priority",
            "2",
            "--reason",
            "Keep current urgency",
        ],
    ));
    assert!(event_details(root, &issue_id, "priority_changed").is_empty());
    assert_eq!(stored_issue(root, &issue_id)["title"], "new title only");
}

#[test]
fn dry_run_projects_the_committed_edit_without_writing() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Preview the edit");
    let before = snapshot(root, &issue_id);
    let arguments = [
        "update",
        &issue_id,
        "--title",
        "Previewed title",
        "--description",
        "Previewed description",
        "--priority",
        "1",
        "--reason",
        "Reviewed",
        "--issue-type",
        "custom",
        "--notes",
        "Previewed notes",
        "--status",
        "blocked",
    ];
    let output = command(root)
        .args(arguments)
        .arg("--dry-run")
        .output()
        .unwrap();
    assert_success(&output);
    let preview: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(preview["semantic_change"], true);
    assert_eq!(preview["before"]["title"], before.issue["title"]);
    assert_eq!(snapshot(root, &issue_id), before);
    assert_success(&run(root, &arguments));
    let after = stored_issue(root, &issue_id);
    for field in [
        "title",
        "description",
        "priority",
        "issue_type",
        "notes",
        "base_status",
        "manual_blocked",
    ] {
        assert_eq!(
            preview["after"][field], after[field],
            "preview differs for {field}"
        );
    }
}

#[test]
fn dry_run_enforces_revision_claim_and_transition_checks() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Preview authorization");
    let claim = json_output(root, &["claim", "--assignee", "editing-worker", "--json"]);
    let epoch = claim["claim_epoch"].as_i64().unwrap().to_string();
    let before = snapshot(root, &issue_id);
    for arguments in [
        vec!["--title", "missing credential"],
        vec![
            "--title",
            "stale edit",
            "--fencing-token",
            &epoch,
            "--if-revision",
            "0",
        ],
        vec![
            "--title",
            "invalid transition",
            "--fencing-token",
            &epoch,
            "--status",
            "closed",
        ],
    ] {
        let output = command(root)
            .args(["update", &issue_id, "--dry-run"])
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(4), "{output:?}");
        assert_eq!(snapshot(root, &issue_id), before);
    }
}

#[test]
fn manifest_edits_and_dependency_changes_commit_together() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Manifest edit");
    let old_blocker = create_issue(root, "Old blocker");
    let new_blocker = create_issue(root, "New blocker");
    assert_success(&run(root, &["dep", "add", &issue_id, &old_blocker]));
    let before = snapshot(root, &issue_id);
    let path = manifest_file(
        root,
        json!([
            {"op":"update", "id":issue_id, "title":"Manifest title", "description":"Manifest description", "priority":1, "reason":"Plan changed", "issue_type":"investigation", "notes":"Manifest notes", "if_revision":before.issue["revision"]},
            {"op":"dep_remove", "blocked":issue_id, "blocker":old_blocker, "kind":"blocks"},
            {"op":"dep_add", "blocked":issue_id, "blocker":new_blocker, "kind":"blocks"}
        ]),
    );
    let dry_run = manifest(root, "dry-run", &path);
    assert_success(&dry_run);
    let report: Value = serde_json::from_slice(&dry_run.stdout).unwrap();
    assert_eq!(report["committed"], false);
    assert_eq!(report["semantic_changes"], 3);
    assert_eq!(snapshot(root, &issue_id), before);
    assert_success(&manifest(root, "commit", &path));
    let after = stored_issue(root, &issue_id);
    assert_eq!(after["title"], "Manifest title");
    assert_eq!(after["description"], "Manifest description");
    assert_eq!(after["priority"], 1);
    assert_eq!(after["issue_type"], "investigation");
    assert_eq!(after["notes"], "Manifest notes");
    assert_eq!(
        json_output(root, &["show", &issue_id, "--json"])["dependencies"],
        json!([{"blocker":new_blocker,"kind":"blocks"}])
    );
    assert_eq!(event_details(root, &issue_id, "priority_changed").len(), 1);
}

#[test]
fn manifest_later_failure_rolls_back_edits_and_priority_audit() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Manifest rollback");
    let before = snapshot(root, &issue_id);
    let path = manifest_file(
        root,
        json!([
            {"op":"update", "id":issue_id, "title":"must not save", "priority":0, "reason":"Not committed"},
            {"op":"dep_add", "blocked":issue_id, "blocker":issue_id, "kind":"blocks"}
        ]),
    );
    for subcommand in ["dry-run", "commit"] {
        let output = manifest(root, subcommand, &path);
        assert_eq!(output.status.code(), Some(4), "{output:?}");
        assert_eq!(snapshot(root, &issue_id), before);
    }
}

#[test]
fn manifest_edits_cannot_take_over_an_existing_claim() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Manifest claim protection");
    assert_success(&run(
        root,
        &["claim", "--assignee", "editing-worker", "--json"],
    ));
    let before = snapshot(root, &issue_id);
    let path = manifest_file(
        root,
        json!([{"op":"update", "id":issue_id, "title":"must not save"}]),
    );
    let output = manifest(root, "commit", &path);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert_eq!(snapshot(root, &issue_id), before);
}

#[test]
fn edited_content_and_audit_survive_published_checkpoint_restore() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = import_extended_issue(root);
    assert_success(&run(root, &["sync", "configure", "--mode", "monolithic"]));
    assert_success(&run(root, &["sync", "flush-only"]));
    let before = snapshot(root, &issue_id);
    let output = publishing_command(root)
        .args([
            "update",
            &issue_id,
            "--title",
            "Restorable edit",
            "--description",
            "Preserve this description",
            "--priority",
            "0",
            "--reason",
            "Restore test",
            "--issue-type",
            "custom",
            "--notes",
            "Preserve these notes",
        ])
        .output()
        .unwrap();
    assert_success(&output);
    let edited = snapshot(root, &issue_id);
    assert_ne!(edited.pointer_bytes, before.pointer_bytes);
    let checkpoint = root.join(".beads/checkpoint");
    let restored = tempfile::tempdir().unwrap();
    let output = command(restored.path())
        .args(["sync", "import-only", "--input"])
        .arg(checkpoint)
        .args(["--restore-into-empty", "--actor", "editing-restore"])
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(stored_issue(restored.path(), &issue_id), edited.issue);
    assert_eq!(
        stored_extensions(restored.path(), &issue_id),
        stored_extensions(root, &issue_id)
    );
    assert_eq!(
        event_details(restored.path(), &issue_id, "priority_changed"),
        event_details(root, &issue_id, "priority_changed")
    );
}

#[test]
fn editing_closed_metadata_preserves_the_close_record() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Closed task");
    assert_success(&run(
        root,
        &["close", &issue_id, "--reason", "Completed work"],
    ));
    let before = stored_issue(root, &issue_id);
    assert_success(&run(
        root,
        &[
            "update",
            &issue_id,
            "--title",
            "Corrected closed title",
            "--description",
            "Corrected description",
            "--issue-type",
            "review",
        ],
    ));
    let after = stored_issue(root, &issue_id);
    assert_eq!(after["title"], "Corrected closed title");
    for field in ["base_status", "closed_at", "close_reason", "created_at"] {
        assert_eq!(after[field], before[field]);
    }
}

#[test]
#[cfg(not(feature = "managed-secret-policy"))]
fn new_edit_fields_keep_native_content_checks_and_redacted_errors() {
    let workspace = workspace();
    let root = workspace.path();
    let issue_id = create_issue(root, "Content checks");
    let synthetic_key = ["AK", "IA", "7Q9W2E4R6T8Y1U3I"].concat();
    let description_file = root.join("blocked-description.txt");
    fs::write(&description_file, &synthetic_key).unwrap();
    let before = snapshot(root, &issue_id);
    for arguments in [
        vec!["--title", synthetic_key.as_str()],
        vec!["--description", synthetic_key.as_str()],
        vec!["--issue-type", synthetic_key.as_str()],
        vec!["--description-file", description_file.to_str().unwrap()],
        vec!["--priority", "0", "--reason", synthetic_key.as_str()],
    ] {
        let output = command(root)
            .args(["update", &issue_id])
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "content rejection should be a usage failure"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("secret_detected"), "{stderr}");
        assert!(!stderr.contains(&synthetic_key));
        assert!(!String::from_utf8_lossy(&output.stdout).contains(&synthetic_key));
        assert_eq!(snapshot(root, &issue_id), before);
    }
}

#[test]
fn capabilities_schema_and_help_describe_the_same_editable_fields() {
    let workspace = workspace();
    let root = workspace.path();
    let capabilities = json_output(root, &["capabilities"]);
    assert_eq!(capabilities["priority_change"], true);
    let editing = &capabilities["issue_editing"];
    assert_eq!(editing["version"], 1);
    assert_eq!(editing["clear_description"], true);
    assert_eq!(editing["revision_guard"], true);
    assert_eq!(editing["priority_reason_required"], true);
    for field in [
        "title",
        "description",
        "priority",
        "issue_type",
        "notes",
        "status",
        "assignee",
    ] {
        assert!(
            editing["fields"]
                .as_array()
                .unwrap()
                .contains(&json!(field)),
            "missing {field}"
        );
    }
    let help = run(root, &["update", "--help"]);
    assert_success(&help);
    let help = String::from_utf8(help.stdout).unwrap();
    for flag in [
        "--title",
        "--description",
        "--description-file",
        "--clear-description",
        "--priority",
        "--reason",
        "--issue-type",
    ] {
        assert!(help.contains(flag), "missing help for {flag}");
    }
    assert!(!help.contains("Title, description, priority, issue type, and labels are not editable"));
    let guide = json_output(
        root,
        &[
            "schema",
            "explain",
            "urn:bead-rs:schema:issue:native-v1",
            "--format",
            "json",
        ],
    );
    for field_name in ["title", "description", "priority", "issue_type"] {
        let field = guide["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["document"] == "checkpoint_issue" && field["name"] == field_name)
            .unwrap();
        assert!(
            field["operations"]
                .as_array()
                .unwrap()
                .contains(&json!("update")),
            "{field_name} is still described as creation-only"
        );
    }
}
