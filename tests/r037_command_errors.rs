//! R037: Command errors that name the remedy
//!
//! Conformance tests for command error messages that state the domain rule
//! and the remedy instead of surfacing a bare parser error.
//!
//! Acceptance criteria:
//! - invalid edits identify the field or required input
//! - labels retain their dedicated-command remedy
//! - close --body names --reason
//! - near-miss handling is bounded to flags that exist on sibling commands
//! - conformance scenarios assert the remedy text, not merely a nonzero exit

use assert_cmd::Command;

fn bead_cmd() -> Command {
    Command::cargo_bin("bead").unwrap()
}

/// Create a disposable issue so input validation reaches the actual edit path.
fn editable_issue() -> (tempfile::TempDir, String) {
    let workspace = tempfile::tempdir().unwrap();
    bead_cmd()
        .args(["--skip-foreign-workspace", "init", "--no-auto-flush"])
        .current_dir(workspace.path())
        .assert()
        .success();
    let created = bead_cmd()
        .args(["create", "--title", "Existing issue", "--no-auto-flush"])
        .current_dir(workspace.path())
        .assert()
        .success();
    let id = String::from_utf8(created.get_output().stdout.clone())
        .unwrap()
        .trim()
        .to_owned();
    (workspace, id)
}

#[test]
fn test_empty_title_names_the_field_requirement() {
    let (workspace, id) = editable_issue();
    let result = bead_cmd()
        .args(["update", &id, "--title", "", "--no-auto-flush"])
        .current_dir(workspace.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&result.get_output().stderr);
    assert!(stderr.contains("Title cannot be empty"), "{stderr}");
}

#[test]
fn test_conflicting_description_sources_names_the_inputs() {
    let (workspace, id) = editable_issue();
    let result = bead_cmd()
        .args([
            "update",
            &id,
            "--description",
            "text",
            "--clear-description",
            "--no-auto-flush",
        ])
        .current_dir(workspace.path())
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&result.get_output().stderr);
    assert!(stderr.contains("--description"), "{stderr}");
    assert!(stderr.contains("--clear-description"), "{stderr}");
}

#[test]
fn test_priority_without_reason_names_the_required_flag() {
    let (workspace, id) = editable_issue();
    let result = bead_cmd()
        .args(["update", &id, "--priority", "0", "--no-auto-flush"])
        .current_dir(workspace.path())
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&result.get_output().stderr);
    assert!(stderr.contains("--priority"), "{stderr}");
    assert!(stderr.contains("--reason"), "{stderr}");
}

#[test]
fn test_empty_issue_type_names_the_field_requirement() {
    let (workspace, id) = editable_issue();
    let result = bead_cmd()
        .args(["update", &id, "--issue-type", "", "--no-auto-flush"])
        .current_dir(workspace.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&result.get_output().stderr);
    assert!(stderr.contains("issue_type cannot be empty"), "{stderr}");
}

#[test]
fn test_update_with_labels_shows_label_command_remedy() {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path();

    bead_cmd()
        .arg("--skip-foreign-workspace")
        .args(["init", "--skip-foreign-workspace"])
        .current_dir(workspace)
        .assert()
        .success();

    bead_cmd()
        .arg("create")
        .arg("--title")
        .arg("Test issue")
        .current_dir(workspace)
        .assert()
        .success();

    let result = bead_cmd()
        .arg("update")
        .arg("bead-")
        .arg("--label")
        .arg("urgent")
        .current_dir(workspace)
        .assert()
        .failure();

    let output = result.get_output();
    assert_eq!(output.status.code(), Some(2));

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stderr.contains("Domain rule:"));
    assert!(stderr.contains("labels are managed via 'bead label add|remove'"));
    assert!(stderr.contains("Remedies:"));
    assert!(stderr.contains("bead label add"));
    assert!(stderr.contains("bead label remove"));
}

#[test]
fn test_close_with_body_flag_shows_reason_remedy() {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path();

    bead_cmd()
        .arg("--skip-foreign-workspace")
        .args(["init", "--skip-foreign-workspace"])
        .current_dir(workspace)
        .assert()
        .success();

    bead_cmd()
        .arg("create")
        .arg("--title")
        .arg("Test issue")
        .current_dir(workspace)
        .assert()
        .success();

    let result = bead_cmd()
        .arg("close")
        .arg("bead-")
        .arg("--body")
        .arg("Done")
        .current_dir(workspace)
        .assert()
        .failure();

    let output = result.get_output();
    assert_eq!(output.status.code(), Some(2));

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Must name the correct flag
    assert!(stderr.contains("close command requires '--reason', not '--body'"));

    // Must state the domain rule
    assert!(stderr.contains("Domain rule:"));
    assert!(stderr.contains("close requires a non-empty reason argument"));

    // Must provide the remedy
    assert!(stderr.contains("Remedy:"));
    assert!(stderr.contains("bead close"));
    assert!(stderr.contains("--reason"));

    // Should provide an example
    assert!(stderr.contains("Example:"));
}

#[test]
fn test_create_with_status_shows_remedy() {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path();

    bead_cmd()
        .arg("--skip-foreign-workspace")
        .args(["init", "--skip-foreign-workspace"])
        .current_dir(workspace)
        .assert()
        .success();

    let result = bead_cmd()
        .arg("create")
        .arg("--title")
        .arg("Test issue")
        .arg("--status")
        .arg("open")
        .current_dir(workspace)
        .assert()
        .failure();

    let output = result.get_output();
    assert_eq!(output.status.code(), Some(2));

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stderr.contains("Domain rule:"));
    assert!(stderr.contains("create initializes an issue with basic fields"));
    assert!(stderr.contains("Remedies:"));
    assert!(stderr.contains("new issues start as 'open'"));
    assert!(stderr.contains("bead update"));
}

#[test]
fn test_label_without_subcommand_shows_remedy() {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path();

    bead_cmd()
        .arg("--skip-foreign-workspace")
        .args(["init", "--skip-foreign-workspace"])
        .current_dir(workspace)
        .assert()
        .success();

    let result = bead_cmd()
        .arg("label")
        .arg("bead-")
        .arg("--label")
        .arg("urgent")
        .current_dir(workspace)
        .assert()
        .failure();

    let output = result.get_output();
    assert_eq!(output.status.code(), Some(2));

    let stderr = String::from_utf8_lossy(&output.stderr);

    // The label command requires add/remove subcommand, so clap handles this
    // This is not a near-miss case that R037 needs to handle specially
    assert!(stderr.contains("unrecognized subcommand") || stderr.contains("required"));
}

#[test]
fn test_invalid_priority_in_compound_edit_names_the_range() {
    let (workspace, id) = editable_issue();
    let result = bead_cmd()
        .args([
            "update",
            &id,
            "--title",
            "must not save",
            "--priority",
            "6",
            "--reason",
            "Invalid range",
            "--no-auto-flush",
        ])
        .current_dir(workspace.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&result.get_output().stderr);
    assert!(
        stderr.contains("Priority must be between 0 and 4"),
        "{stderr}"
    );
    let shown = bead_cmd()
        .args(["show", &id, "--json"])
        .current_dir(workspace.path())
        .assert()
        .success();
    let issue: serde_json::Value = serde_json::from_slice(&shown.get_output().stdout).unwrap();
    assert_eq!(issue["title"], "Existing issue");
}

#[test]
fn test_help_reference_in_error_message() {
    // Error messages should reference --help for complete usage
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path();

    bead_cmd()
        .arg("--skip-foreign-workspace")
        .args(["init", "--skip-foreign-workspace"])
        .current_dir(workspace)
        .assert()
        .success();

    let result = bead_cmd()
        .arg("update")
        .arg("bead-")
        .arg("--label")
        .arg("new-label")
        .current_dir(workspace)
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&result.get_output().stderr);

    // Should reference help for complete usage information
    assert!(stderr.contains("--help"));
}
