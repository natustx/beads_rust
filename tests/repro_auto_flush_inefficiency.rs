use beads_rust::model::{Issue, IssueType, Priority, Status};
use beads_rust::storage::SqliteStorage;
use beads_rust::sync::{auto_flush, AutoFlushResult};
use chrono::Utc;
use std::fs;
use tempfile::TempDir;

fn make_issue(id: &str) -> Issue {
    Issue {
        id: id.to_string(),
        title: "Test Issue".to_string(),
        status: Status::Open,
        priority: Priority::MEDIUM,
        issue_type: IssueType::Task,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        ..Default::default()
    }
}

#[test]
fn test_auto_flush_optimizes_no_content_change() {
    let temp_dir = TempDir::new().unwrap();
    let beads_dir = temp_dir.path().join(".beads");
    fs::create_dir(&beads_dir).unwrap();
    let db_path = beads_dir.join("beads.db");
    
    let mut storage = SqliteStorage::open(&db_path).unwrap();
    
    // 1. Create an issue
    let issue = make_issue("bd-1");
    storage.create_issue(&issue, "tester").unwrap();
    
    // 2. First auto-flush (should export)
    let result = auto_flush(&mut storage, &beads_dir).unwrap();
    assert!(result.flushed, "First flush should happen");
    assert_eq!(result.exported_count, 1);
    
    // 3. Mark issue dirty effectively WITHOUT changing content
    // We do this by changing it and changing it back.
    // NOTE: This relies on the fact that we haven't exported the intermediate state.
    
    // Change title
    let update_change = beads_rust::storage::IssueUpdate {
        title: Some("Changed Title".to_string()),
        ..Default::default()
    };
    storage.update_issue("bd-1", &update_change, "tester").unwrap();
    
    // Revert title
    let update_revert = beads_rust::storage::IssueUpdate {
        title: Some("Test Issue".to_string()),
        ..Default::default()
    };
    storage.update_issue("bd-1", &update_revert, "tester").unwrap();
    
    // Verify it is dirty
    let dirty_ids = storage.get_dirty_issue_ids().unwrap();
    assert_eq!(dirty_ids.len(), 1, "Issue should be dirty after updates");
    
    // 4. Second auto-flush (should SKIP export because content hash hasn't changed)
    // CURRENTLY THIS FAILS (it flushes)
    let result = auto_flush(&mut storage, &beads_dir).unwrap();
    
    // If optimization is working, flushed should be false
    assert!(!result.flushed, "Should skip export if content hash is unchanged");
    
    // And dirty flags should be cleared
    let dirty_ids = storage.get_dirty_issue_ids().unwrap();
    assert!(dirty_ids.is_empty(), "Dirty flags should be cleared even if export skipped");
}
