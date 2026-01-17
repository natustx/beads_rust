use crate::cli::CreateArgs;
use crate::config;
use crate::error::{BeadsError, Result};
use crate::model::{Dependency, DependencyType, Issue, IssueType, Priority, Status};
use crate::storage::SqliteStorage;
use crate::util::id::IdGenerator;
use crate::util::markdown_import::parse_markdown_file;
use crate::util::time::parse_flexible_timestamp;
use crate::validation::{IssueValidator, LabelValidator};
use chrono::{DateTime, Utc};
use std::path::Path;
use std::str::FromStr;

/// Configuration for creating an issue.
pub struct CreateConfig {
    pub id_config: crate::util::id::IdConfig,
    pub default_priority: Priority,
    pub default_issue_type: IssueType,
    pub actor: String,
}

/// Execute the create command.
///
/// # Errors
///
/// Returns an error if validation fails, the database cannot be opened, or the issue cannot be created.
#[allow(clippy::too_many_lines)]
pub fn execute(args: CreateArgs, cli: &config::CliOverrides) -> Result<()> {
    if let Some(ref file_path) = args.file {
        return execute_import(file_path, &args, cli);
    }

    // 1. Open storage (unless dry run without DB)
    let beads_dir = config::discover_beads_dir(Some(Path::new(".")))?;

    // We open storage even for dry-run to check ID collisions.
    let mut storage_ctx = config::open_storage_with_cli(&beads_dir, cli)?;
    let layer = config::load_config(&beads_dir, Some(&storage_ctx.storage), cli)?;
    
    let config = CreateConfig {
        id_config: config::id_config_from_layer(&layer),
        default_priority: config::default_priority_from_layer(&layer)?,
        default_issue_type: config::default_issue_type_from_layer(&layer)?,
        actor: config::resolve_actor(&layer),
    };

    let issue = create_issue_impl(&mut storage_ctx.storage, &args, &config)?;

    // Output
    if args.silent {
        println!("{}", issue.id);
    } else if cli.json.unwrap_or(false) {
        // For JSON output, we want the full issue as retrieved from DB if it was created,
        // or the constructed issue if dry-run.
        // If created, fetching from DB ensures all fields are canonical.
        if !args.dry_run {
            let full_issue = storage_ctx.storage
                .get_issue_for_export(&issue.id)?
                .ok_or_else(|| BeadsError::IssueNotFound { id: issue.id.clone() })?;
            println!("{}", serde_json::to_string_pretty(&full_issue)?);
        } else {
            println!("{}", serde_json::to_string_pretty(&issue)?);
        }
    } else if args.dry_run {
        println!("Dry run: would create issue {}", issue.id);
        println!("Title: {}", issue.title);
        println!("Type: {}", issue.issue_type);
        println!("Priority: {}", issue.priority);
        if !args.labels.is_empty() {
            println!("Labels: {}", args.labels.join(", "));
        }
        if let Some(parent) = &args.parent {
            println!("Parent: {parent}");
        }
        if !args.deps.is_empty() {
            println!("Dependencies: {}", args.deps.join(", "));
        }
    } else {
        println!("Created {}: {}", issue.id, issue.title);
    }

    storage_ctx.flush_no_db_if_dirty()?;
    Ok(())
}

/// Core logic for creating an issue.
/// 
/// Handles ID generation, validation, and storage insertion.
/// Returns the constructed Issue.
pub fn create_issue_impl(
    storage: &mut SqliteStorage,
    args: &CreateArgs,
    config: &CreateConfig,
) -> Result<Issue> {
    // 1. Resolve title
    let title = args
        .title
        .as_ref()
        .or(args.title_flag.as_ref())
        .ok_or_else(|| BeadsError::validation("title", "cannot be empty"))?;

    if title.is_empty() {
        return Err(BeadsError::validation("title", "cannot be empty"));
    }

    // 2. Generate ID
    let id_gen = IdGenerator::new(config.id_config.clone());
    let now = Utc::now();
    let count = storage.count_issues()?;

    let id = id_gen.generate(
        title,
        None, // description
        None, // creator
        now,
        count,
        |id| storage.id_exists(id).unwrap_or(false),
    );

    // 3. Parse fields
    let priority = if let Some(p) = &args.priority {
        Priority::from_str(p)?
    } else {
        config.default_priority
    };

    let issue_type = if let Some(t) = &args.type_ {
        IssueType::from_str(t)?
    } else {
        config.default_issue_type.clone()
    };

    let due_at = parse_optional_date(args.due.as_deref())?;
    let defer_until = parse_optional_date(args.defer.as_deref())?;

    // 4. Construct Issue
    let mut issue = Issue {
        id: id.clone(),
        title: title.clone(),
        description: args.description.clone(),
        status: Status::Open,
        priority,
        issue_type,
        created_at: now,
        updated_at: now,
        assignee: args.assignee.clone(),
        owner: args.owner.clone(),
        estimated_minutes: args.estimate,
        due_at,
        defer_until,
        external_ref: args.external_ref.clone(),
        ephemeral: args.ephemeral,
        // Defaults
        content_hash: None,
        design: None,
        acceptance_criteria: None,
        notes: None,
        created_by: None,
        closed_at: None,
        close_reason: None,
        closed_by_session: None,
        source_system: None,
        deleted_at: None,
        deleted_by: None,
        delete_reason: None,
        original_type: None,
        compaction_level: None,
        compacted_at: None,
        compacted_at_commit: None,
        original_size: None,
        sender: None,
        pinned: false,
        is_template: false,
        labels: vec![],
        dependencies: vec![],
        comments: vec![],
    };

    // Compute content hash
    issue.content_hash = Some(issue.compute_content_hash());

    // 5. Validate Issue
    IssueValidator::validate(&issue).map_err(BeadsError::from_validation_errors)?;

    // 6. Dry Run check - return early
    if args.dry_run {
        return Ok(issue);
    }

    // 7. Create
    storage.create_issue(&issue, &config.actor)?;

    // 8. Add auxiliary data
    // Labels
    for label in &args.labels {
        let label = label.trim();
        if !label.is_empty() {
            LabelValidator::validate(label)
                .map_err(|e| BeadsError::validation("label", e.message))?;
            storage.add_label(&id, label, &config.actor)?;
            issue.labels.push(label.to_string());
        }
    }

    // Parent
    if let Some(parent_id) = &args.parent {
        if parent_id == &id {
            return Err(BeadsError::validation(
                "parent",
                "cannot be parent of itself",
            ));
        }
        storage.add_dependency(&id, parent_id, "parent-child", &config.actor)?;

        issue.dependencies.push(Dependency {
            issue_id: id.clone(),
            depends_on_id: parent_id.clone(),
            dep_type: DependencyType::ParentChild,
            created_at: now,
            created_by: Some(config.actor.clone()),
            metadata: None,
            thread_id: None,
        });
    }

    // Dependencies
    for dep_str in &args.deps {
        let (type_str, dep_id) = if let Some((t, i)) = dep_str.split_once(':') {
            (t, i)
        } else {
            ("blocks", dep_str.as_str())
        };

        if dep_id == id {
            return Err(BeadsError::validation("deps", "cannot depend on itself"));
        }
        storage.add_dependency(&id, dep_id, type_str, &config.actor)?;

        let dep_type = type_str
            .parse()
            .unwrap_or_else(|_| DependencyType::Custom(type_str.to_string()));
        issue.dependencies.push(Dependency {
            issue_id: id.clone(),
            depends_on_id: dep_id.to_string(),
            dep_type,
            created_at: now,
            created_by: Some(config.actor.clone()),
            metadata: None,
            thread_id: None,
        });
    }

    Ok(issue)
}

#[allow(clippy::too_many_lines)]
fn execute_import(path: &Path, args: &CreateArgs, cli: &config::CliOverrides) -> Result<()> {
    let parsed_issues = parse_markdown_file(path)?;
    if parsed_issues.is_empty() {
        return Ok(());
    }

    let beads_dir = config::discover_beads_dir(Some(Path::new(".")))?;
    let mut storage_ctx = config::open_storage_with_cli(&beads_dir, cli)?;
    let layer = config::load_config(&beads_dir, Some(&storage_ctx.storage), cli)?;

    let id_config = config::id_config_from_layer(&layer);
    let default_priority = config::default_priority_from_layer(&layer)?;
    let default_issue_type = config::default_issue_type_from_layer(&layer)?;
    let actor = config::resolve_actor(&layer);
    let now = Utc::now();

    let storage = &mut storage_ctx.storage;
    let id_gen = IdGenerator::new(id_config);

    // Track created IDs for output
    let mut created_ids = Vec::new();

    for parsed in parsed_issues {
        let count = storage.count_issues()?;
        let id = id_gen.generate(
            &parsed.title,
            parsed.description.as_deref(),
            None,
            now,
            count,
            |id| storage.id_exists(id).unwrap_or(false),
        );

        let priority = if let Some(ref p) = parsed.priority {
            Priority::from_str(p)?
        } else {
            default_priority
        };

        let issue_type = if let Some(ref t) = parsed.issue_type {
            IssueType::from_str(t)?
        } else {
            default_issue_type.clone()
        };

        let mut issue = Issue {
            id: id.clone(),
            title: parsed.title,
            description: parsed.description,
            status: Status::Open,
            priority,
            issue_type,
            created_at: now,
            updated_at: now,
            assignee: parsed.assignee,
            owner: args.owner.clone(),
            estimated_minutes: args.estimate, 
            due_at: parse_optional_date(args.due.as_deref())?,
            defer_until: parse_optional_date(args.defer.as_deref())?,
            external_ref: args.external_ref.clone(),
            ephemeral: args.ephemeral,
            design: parsed.design,
            acceptance_criteria: parsed.acceptance_criteria,
            content_hash: None,
            notes: None,
            created_by: None,
            closed_at: None,
            close_reason: None,
            closed_by_session: None,
            source_system: None,
            deleted_at: None,
            deleted_by: None,
            delete_reason: None,
            original_type: None,
            compaction_level: None,
            compacted_at: None,
            compacted_at_commit: None,
            original_size: None,
            sender: None,
            pinned: false,
            is_template: false,
            labels: vec![],
            dependencies: vec![],
            comments: vec![],
        };

        issue.content_hash = Some(issue.compute_content_hash());
        IssueValidator::validate(&issue).map_err(BeadsError::from_validation_errors)?;

        if args.dry_run {
            println!("Dry run: would create issue {}", issue.id);
            continue;
        }

        storage.create_issue(&issue, &actor)?;

        let mut labels = parsed.labels;
        labels.extend(args.labels.clone());
        for label in labels {
            if !label.trim().is_empty() {
                LabelValidator::validate(&label)
                    .map_err(|e| BeadsError::validation("label", e.message))?;
                storage.add_label(&id, &label, &actor)?;
            }
        }

        let mut deps = parsed.dependencies;
        deps.extend(args.deps.clone());
        for dep_str in deps {
            let (type_str, dep_id) = if let Some((t, i)) = dep_str.split_once(':') {
                (t, i)
            } else {
                ("blocks", dep_str.as_str())
            };
            if dep_id == id {
                return Err(BeadsError::validation(
                    "deps",
                    format!("issue {id} cannot depend on itself"),
                ));
            }
            storage.add_dependency(&id, dep_id, type_str, &actor)?;
        }

        created_ids.push(id);
    }

    if !created_ids.is_empty() && !args.dry_run {
        println!("Created {} issues:", created_ids.len());
        for id in created_ids {
            println!("  {id}");
        }
    }

    storage_ctx.flush_no_db_if_dirty()?;
    Ok(())
}

fn parse_optional_date(s: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    match s {
        Some(s) if !s.is_empty() => parse_flexible_timestamp(s, "date").map(Some),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;
    use crate::util::id::IdConfig;

    // Helper to create basic args
    fn default_args() -> CreateArgs {
        CreateArgs {
            title: Some("Test Issue".to_string()),
            title_flag: None,
            type_: None,
            priority: None,
            description: None,
            assignee: None,
            owner: None,
            labels: vec![],
            parent: None,
            deps: vec![],
            estimate: None,
            due: None,
            defer: None,
            external_ref: None,
            ephemeral: false,
            dry_run: false,
            silent: false,
            file: None,
        }
    }

    fn default_config() -> CreateConfig {
        CreateConfig {
            id_config: IdConfig {
                prefix: "bd".to_string(),
                min_hash_length: 3,
                max_hash_length: 8,
                max_collision_prob: 0.25,
            },
            default_priority: Priority::MEDIUM,
            default_issue_type: IssueType::Task,
            actor: "test_user".to_string(),
        }
    }

    fn setup_memory_storage() -> SqliteStorage {
        SqliteStorage::open_memory().expect("failed to open memory db")
    }

    #[test]
    fn test_create_issue_basic_success() {
        let mut storage = setup_memory_storage();
        let args = default_args();
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");

        assert_eq!(issue.title, "Test Issue");
        assert_eq!(issue.priority, Priority::MEDIUM);
        assert_eq!(issue.issue_type, IssueType::Task);
        assert!(issue.id.starts_with("bd-"));
        
        // Verify persisted
        let loaded = storage.get_issue(&issue.id).expect("get issue");
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().title, "Test Issue");
    }

    #[test]
    fn test_create_issue_validation_empty_title() {
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.title = None;
        let config = default_config();

        let err = create_issue_impl(&mut storage, &args, &config).unwrap_err();
        assert!(matches!(err, BeadsError::Validation { field, .. } if field == "title"));
    }

    #[test]
    fn test_create_issue_dry_run_no_writes() {
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.dry_run = true;
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");

        // Should return issue but not verify existence in DB
        assert_eq!(issue.title, "Test Issue");
        let loaded = storage.get_issue(&issue.id).expect("get issue");
        assert!(loaded.is_none(), "dry run should not persist issue");
    }

    #[test]
    fn test_create_issue_with_overrides() {
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.priority = Some("0".to_string());
        args.type_ = Some("bug".to_string());
        args.description = Some("Desc".to_string());
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");

        assert_eq!(issue.priority, Priority::CRITICAL);
        assert_eq!(issue.issue_type, IssueType::Bug);
        assert_eq!(issue.description, Some("Desc".to_string()));
    }

    #[test]
    fn test_create_issue_with_labels_and_deps() {
        let mut storage = setup_memory_storage();
        let config = default_config();
        
        // Create dependency target first
        let target_args = CreateArgs {
             title: Some("Target".to_string()),
             ..default_args()
        };
        let target = create_issue_impl(&mut storage, &target_args, &config).expect("create target");
        
        // Create issue with label and dep
        let mut args = default_args();
        args.labels = vec!["backend".to_string()];
        args.deps = vec![target.id.clone()];
        
        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");
        
        // Verify labels
        let labels = storage.get_labels(&issue.id).expect("get labels");
        assert!(labels.contains(&"backend".to_string()));
        
        // Verify deps
        let deps = storage.get_dependencies(&issue.id).expect("get deps");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0], target.id);
    }
    
    #[test]
    fn test_create_parent_dependency() {
        let mut storage = setup_memory_storage();
        let config = default_config();
        
        // Parent
        let parent = create_issue_impl(&mut storage, &default_args(), &config).expect("parent");
        
        // Child
        let mut args = default_args();
        args.parent = Some(parent.id.clone());
        let child = create_issue_impl(&mut storage, &args, &config).expect("child");
        
        let deps = storage.get_dependencies(&child.id).expect("get deps");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0], parent.id);
    }

    // =========================================================================
    // parse_optional_date tests (preserved)
    // =========================================================================

    #[test]
    fn test_parse_optional_date_none() {
        let result = parse_optional_date(None);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_parse_optional_date_empty_string() {
        let result = parse_optional_date(Some(""));
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_parse_optional_date_iso8601() {
        let result = parse_optional_date(Some("2026-01-17T10:00:00Z"));
        assert!(result.is_ok());
        let date = result.unwrap();
        assert!(date.is_some());
        let dt = date.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 17);
    }

    #[test]
    fn test_parse_optional_date_simple_date() {
        let result = parse_optional_date(Some("2026-12-31"));
        assert!(result.is_ok());
        let date = result.unwrap();
        assert!(date.is_some());
        let dt = date.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 12);
        assert_eq!(dt.day(), 31);
    }

    #[test]
    fn test_parse_optional_date_with_timezone() {
        let result = parse_optional_date(Some("2026-06-15T14:30:00+05:30"));
        assert!(result.is_ok());
        let date = result.unwrap();
        assert!(date.is_some());
    }

    #[test]
    fn test_parse_optional_date_invalid_format() {
        let result = parse_optional_date(Some("not-a-date"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_optional_date_partial_date() {
        // Flexible parser may accept various formats
        let result = parse_optional_date(Some("2026-01"));
        // This might be valid or invalid depending on the flexible parser
        // We just verify it doesn't panic
        let _ = result;
    }

    // =========================================================================
    // Date boundary tests
    // =========================================================================

    #[test]
    fn test_parse_optional_date_year_boundaries() {
        // Far future date
        let result = parse_optional_date(Some("2099-12-31"));
        assert!(result.is_ok());

        // Past date
        let result = parse_optional_date(Some("2000-01-01"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_optional_date_leap_year() {
        // Feb 29 on leap year
        let result = parse_optional_date(Some("2024-02-29"));
        assert!(result.is_ok());
        let date = result.unwrap();
        assert!(date.is_some());
        let dt = date.unwrap();
        assert_eq!(dt.month(), 2);
        assert_eq!(dt.day(), 29);
    }

    #[test]
    fn test_parse_optional_date_end_of_month() {
        // 31-day month
        let result = parse_optional_date(Some("2026-03-31"));
        assert!(result.is_ok());

        // 30-day month
        let result = parse_optional_date(Some("2026-04-30"));
        assert!(result.is_ok());
    }

    // =========================================================================
    // Whitespace handling tests
    // =========================================================================

    #[test]
    fn test_parse_optional_date_whitespace_only() {
        // Should be treated as non-empty by the string check, but may fail parsing
        let result = parse_optional_date(Some("   "));
        // Behavior depends on implementation - just ensure no panic
        let _ = result;
    }

    // =========================================================================
    // Additional create issue tests for comprehensive coverage
    // =========================================================================

    #[test]
    fn test_create_issue_with_title_flag() {
        // Test using title_flag instead of positional title
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.title = None;
        args.title_flag = Some("Title via Flag".to_string());
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");
        assert_eq!(issue.title, "Title via Flag");
    }

    #[test]
    fn test_create_issue_empty_string_title() {
        // Empty string should fail validation
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.title = Some(String::new());
        let config = default_config();

        let err = create_issue_impl(&mut storage, &args, &config).unwrap_err();
        assert!(matches!(err, BeadsError::Validation { field, .. } if field == "title"));
    }

    #[test]
    fn test_create_issue_invalid_priority() {
        // Invalid priority string should fail
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.priority = Some("invalid_priority".to_string());
        let config = default_config();

        let result = create_issue_impl(&mut storage, &args, &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_issue_custom_type() {
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.type_ = Some("invalid_type".to_string());
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create should succeed with custom type");
        assert_eq!(issue.issue_type, IssueType::Custom("invalid_type".to_string()));
    }

    #[test]
    fn test_create_issue_content_hash_computed() {
        // Content hash should be computed for the issue
        let mut storage = setup_memory_storage();
        let args = default_args();
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");

        assert!(issue.content_hash.is_some(), "content hash should be set");
        // Hash should be non-empty
        assert!(!issue.content_hash.as_ref().unwrap().is_empty());
    }

    #[test]
    fn test_create_issue_with_estimate() {
        // Test estimated_minutes field
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.estimate = Some(120); // 2 hours
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");
        assert_eq!(issue.estimated_minutes, Some(120));
    }

    #[test]
    fn test_create_issue_with_due_date() {
        // Test due date handling
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.due = Some("2026-12-31".to_string());
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");
        assert!(issue.due_at.is_some());
        let due = issue.due_at.unwrap();
        assert_eq!(due.year(), 2026);
        assert_eq!(due.month(), 12);
        assert_eq!(due.day(), 31);
    }

    #[test]
    fn test_create_issue_with_defer_date() {
        // Test defer_until handling
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.defer = Some("2026-06-15".to_string());
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");
        assert!(issue.defer_until.is_some());
        let defer = issue.defer_until.unwrap();
        assert_eq!(defer.year(), 2026);
        assert_eq!(defer.month(), 6);
        assert_eq!(defer.day(), 15);
    }

    #[test]
    fn test_create_issue_with_external_ref() {
        // Test external_ref field
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.external_ref = Some("JIRA-123".to_string());
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");
        assert_eq!(issue.external_ref, Some("JIRA-123".to_string()));
    }

    #[test]
    fn test_create_issue_ephemeral() {
        // Test ephemeral flag
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.ephemeral = true;
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");
        assert!(issue.ephemeral);
    }

    #[test]
    fn test_create_issue_with_assignee_and_owner() {
        // Test assignee and owner fields
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.assignee = Some("alice".to_string());
        args.owner = Some("bob".to_string());
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");
        assert_eq!(issue.assignee, Some("alice".to_string()));
        assert_eq!(issue.owner, Some("bob".to_string()));
    }

    #[test]
    fn test_create_issue_typed_dependency() {
        // Test dependency with type prefix (e.g., "related:bd-123")
        let mut storage = setup_memory_storage();
        let config = default_config();

        // Create target issue first
        let target = create_issue_impl(&mut storage, &default_args(), &config).expect("create target");

        // Create issue with typed dependency
        let mut args = default_args();
        args.deps = vec![format!("related:{}", target.id)];

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");

        // Verify the dependency was created (type is "related")
        let deps = storage.get_dependencies(&issue.id).expect("get deps");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0], target.id);
    }

    #[test]
    fn test_create_issue_multiple_labels() {
        // Test multiple labels
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.labels = vec![
            "backend".to_string(),
            "urgent".to_string(),
            "security".to_string(),
        ];
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");

        let labels = storage.get_labels(&issue.id).expect("get labels");
        assert_eq!(labels.len(), 3);
        assert!(labels.contains(&"backend".to_string()));
        assert!(labels.contains(&"urgent".to_string()));
        assert!(labels.contains(&"security".to_string()));
    }

    #[test]
    fn test_create_issue_label_trimmed() {
        // Labels with whitespace should be trimmed
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.labels = vec!["  spaced  ".to_string()];
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");

        let labels = storage.get_labels(&issue.id).expect("get labels");
        assert_eq!(labels.len(), 1);
        assert!(labels.contains(&"spaced".to_string()));
    }

    #[test]
    fn test_create_issue_empty_label_skipped() {
        // Empty labels should be skipped
        let mut storage = setup_memory_storage();
        let mut args = default_args();
        args.labels = vec!["valid".to_string(), "".to_string(), "  ".to_string()];
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");

        let labels = storage.get_labels(&issue.id).expect("get labels");
        // Only non-empty labels should be added
        assert!(labels.contains(&"valid".to_string()));
    }

    #[test]
    fn test_create_issue_all_priority_values() {
        // Test all valid priority values
        let config = default_config();

        for (p_str, expected) in [
            ("0", Priority::CRITICAL),
            ("1", Priority::HIGH),
            ("2", Priority::MEDIUM),
            ("3", Priority::LOW),
            ("4", Priority::BACKLOG),
            ("P0", Priority::CRITICAL),
            ("P1", Priority::HIGH),
            ("P2", Priority::MEDIUM),
            ("P3", Priority::LOW),
            ("P4", Priority::BACKLOG),
        ] {
            let mut storage = setup_memory_storage();
            let mut args = default_args();
            args.priority = Some(p_str.to_string());

            let issue = create_issue_impl(&mut storage, &args, &config)
                .unwrap_or_else(|e| panic!("Failed for priority {p_str}: {e}"));
            assert_eq!(issue.priority, expected, "Priority mismatch for {p_str}");
        }
    }

    #[test]
    fn test_create_issue_all_type_values() {
        // Test all valid issue types
        let config = default_config();

        for (t_str, expected) in [
            ("task", IssueType::Task),
            ("bug", IssueType::Bug),
            ("feature", IssueType::Feature),
            ("epic", IssueType::Epic),
            ("chore", IssueType::Chore),
            ("docs", IssueType::Docs),
            ("question", IssueType::Question),
        ] {
            let mut storage = setup_memory_storage();
            let mut args = default_args();
            args.type_ = Some(t_str.to_string());

            let issue = create_issue_impl(&mut storage, &args, &config)
                .unwrap_or_else(|e| panic!("Failed for type {t_str}: {e}"));
            assert_eq!(issue.issue_type, expected, "Type mismatch for {t_str}");
        }
    }

    #[test]
    fn test_create_issue_id_has_prefix() {
        // Issue ID should start with configured prefix
        let mut storage = setup_memory_storage();
        let args = default_args();
        let mut config = default_config();
        config.id_config.prefix = "test".to_string();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");
        assert!(issue.id.starts_with("test-"), "ID should start with configured prefix");
    }

    #[test]
    fn test_create_issue_timestamps_set() {
        // created_at and updated_at should be set
        let mut storage = setup_memory_storage();
        let args = default_args();
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");

        // Both timestamps should be set and roughly equal for new issue
        let diff = (issue.updated_at - issue.created_at).num_seconds().abs();
        assert!(diff < 2, "created_at and updated_at should be nearly equal for new issue");
    }

    #[test]
    fn test_create_issue_status_is_open() {
        // New issues should have Open status
        let mut storage = setup_memory_storage();
        let args = default_args();
        let config = default_config();

        let issue = create_issue_impl(&mut storage, &args, &config).expect("create failed");
        assert_eq!(issue.status, Status::Open);
    }
}
