mod common;

use common::cli::{BrWorkspace, extract_json_payload, run_br};
use serde_json::Value;
use std::time::Instant;
use tracing::info;

#[test]
fn test_create_json_output_includes_labels_and_deps() {
    common::init_test_logging();
    let test_start = Instant::now();
    info!("Starting test_create_json_output_includes_labels_and_deps");
    let workspace = BrWorkspace::new();

    // Init
    info!("Running init");
    let init = run_br(&workspace, ["init"], "init");
    info!(
        "init status={} duration={:?} log={}",
        init.status,
        init.duration,
        init.log_path.display()
    );
    assert!(init.status.success(), "init failed: {}", init.stderr);

    // Create blocking issue first
    info!("Creating blocker issue");
    let blocker = run_br(
        &workspace,
        ["create", "Blocker", "--json"],
        "create_blocker",
    );
    info!(
        "create_blocker status={} duration={:?} log={}",
        blocker.status,
        blocker.duration,
        blocker.log_path.display()
    );
    assert!(
        blocker.status.success(),
        "Failed to create blocking issue: {}",
        blocker.stderr
    );

    let blocker_json: Value = serde_json::from_str(&extract_json_payload(&blocker.stdout)).unwrap();
    let blocker_id = blocker_json["id"].as_str().unwrap();
    info!("Parsed blocker_id={blocker_id}");

    // Create issue with label and dep
    info!("Creating issue with label+dependency");
    let issue = run_br(
        &workspace,
        [
            "create", "My Issue", "--labels", "bug", "--deps", blocker_id, "--json",
        ],
        "create_issue",
    );
    info!(
        "create_issue status={} duration={:?} log={}",
        issue.status,
        issue.duration,
        issue.log_path.display()
    );
    assert!(
        issue.status.success(),
        "Failed to create issue with label and dep: {}",
        issue.stderr
    );

    let issue_json: Value = serde_json::from_str(&extract_json_payload(&issue.stdout)).unwrap();
    // Verify fields
    let labels = issue_json["labels"]
        .as_array()
        .expect("labels should be an array");
    let deps = issue_json["dependencies"]
        .as_array()
        .expect("dependencies should be an array");

    info!("Asserting labels include 'bug'");
    assert!(
        labels.iter().any(|l| l.as_str() == Some("bug")),
        "Labels should contain 'bug'"
    );
    info!("Asserting dependencies include blocker_id");
    assert!(
        deps.iter()
            .any(|d| d["depends_on_id"].as_str() == Some(blocker_id)),
        "Dependencies should contain blocker ID"
    );
    info!(
        "test_create_json_output_includes_labels_and_deps passed in {:?}",
        test_start.elapsed()
    );
}
