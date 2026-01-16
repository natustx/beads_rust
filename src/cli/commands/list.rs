//! List command implementation.
//!
//! Primary discovery interface with classic filter semantics and
//! `IssueWithCounts` JSON output.

use crate::cli::ListArgs;
use crate::config;
use crate::error::{BeadsError, Result};
use crate::format::{IssueWithCounts, format_issue_line};
use crate::model::{IssueType, Priority, Status};
use crate::storage::{ListFilters, SqliteStorage};
use chrono::Utc;
use std::collections::HashSet;
use std::path::Path;

/// Execute the list command.
///
/// # Errors
///
/// Returns an error if the database cannot be opened or the query fails.
pub fn execute(args: &ListArgs, json: bool) -> Result<()> {
    // Open storage
    let beads_dir = config::discover_beads_dir(Some(Path::new(".")))?;
    let (storage, _paths) = config::open_storage(&beads_dir, None)?;

    // Build filter from args
    let mut filters = build_filters(args);
    let client_filters = needs_client_filters(args);
    let limit = if client_filters {
        filters.limit.take()
    } else {
        None
    };

    // Query issues
    let issues = storage.list_issues(&filters)?;
    let issues = if client_filters {
        apply_client_filters(&storage, issues, args)?
    } else {
        issues
    };

    // Convert to IssueWithCounts
    let mut issues_with_counts: Vec<IssueWithCounts> = issues
        .into_iter()
        .map(|issue| {
            let dependency_count = storage.count_dependencies(&issue.id).unwrap_or(0);
            let dependent_count = storage.count_dependents(&issue.id).unwrap_or(0);
            IssueWithCounts {
                issue,
                dependency_count,
                dependent_count,
            }
        })
        .collect();

    apply_sort(&mut issues_with_counts, args.sort.as_deref())?;
    if args.reverse {
        issues_with_counts.reverse();
    }
    if let Some(limit) = limit {
        if limit > 0 && issues_with_counts.len() > limit {
            issues_with_counts.truncate(limit);
        }
    }

    // Output
    if json {
        let json_output = serde_json::to_string_pretty(&issues_with_counts)?;
        println!("{json_output}");
    } else if issues_with_counts.is_empty() {
        println!("No issues found.");
    } else {
        for iwc in &issues_with_counts {
            let line = format_issue_line(&iwc.issue);
            println!("{line}");
        }
        println!("\n{} issue(s)", issues_with_counts.len());
    }

    Ok(())
}

/// Convert CLI args to storage filter.
fn build_filters(args: &ListArgs) -> ListFilters {
    // Parse status strings to Status enums
    let statuses = if args.status.is_empty() {
        None
    } else {
        let parsed: Vec<Status> = args.status.iter().filter_map(|s| s.parse().ok()).collect();
        if parsed.is_empty() {
            None
        } else {
            Some(parsed)
        }
    };

    // Parse type strings to IssueType enums
    let types = if args.type_.is_empty() {
        None
    } else {
        let parsed: Vec<IssueType> = args.type_.iter().filter_map(|t| t.parse().ok()).collect();
        if parsed.is_empty() {
            None
        } else {
            Some(parsed)
        }
    };

    // Parse priority values
    let priorities = if args.priority.is_empty() {
        None
    } else {
        let parsed: Vec<Priority> = args
            .priority
            .iter()
            .map(|&p| Priority(i32::from(p)))
            .collect();
        Some(parsed)
    };

    ListFilters {
        statuses,
        types,
        priorities,
        assignee: args.assignee.clone(),
        unassigned: args.unassigned,
        include_closed: args.all,
        include_templates: false,
        title_contains: args.title_contains.clone(),
        limit: args.limit,
    }
}

fn needs_client_filters(args: &ListArgs) -> bool {
    !args.id.is_empty()
        || !args.label.is_empty()
        || !args.label_any.is_empty()
        || args.priority_min.is_some()
        || args.priority_max.is_some()
        || args.desc_contains.is_some()
        || args.notes_contains.is_some()
        || args.sort.is_some()
        || args.reverse
        || args.deferred
        || args.overdue
}

fn apply_client_filters(
    storage: &SqliteStorage,
    issues: Vec<crate::model::Issue>,
    args: &ListArgs,
) -> Result<Vec<crate::model::Issue>> {
    let id_filter: Option<HashSet<&str>> = if args.id.is_empty() {
        None
    } else {
        Some(args.id.iter().map(String::as_str).collect())
    };

    let label_filters = !args.label.is_empty() || !args.label_any.is_empty();
    let mut filtered = Vec::new();
    let now = Utc::now();
    let min_priority = args.priority_min.map(i32::from);
    let max_priority = args.priority_max.map(i32::from);
    let desc_needle = args.desc_contains.as_deref().map(|s| s.to_lowercase());
    let notes_needle = args.notes_contains.as_deref().map(|s| s.to_lowercase());
    let include_deferred = args.deferred
        || args
            .status
            .iter()
            .any(|status| status.eq_ignore_ascii_case("deferred"));

    if let Some(min) = min_priority {
        if !(0..=4).contains(&min) {
            return Err(BeadsError::InvalidPriority { priority: min });
        }
    }
    if let Some(max) = max_priority {
        if !(0..=4).contains(&max) {
            return Err(BeadsError::InvalidPriority { priority: max });
        }
    }

    for issue in issues {
        if let Some(ids) = &id_filter {
            if !ids.contains(issue.id.as_str()) {
                continue;
            }
        }

        if let Some(min) = min_priority {
            if issue.priority.0 < min {
                continue;
            }
        }
        if let Some(max) = max_priority {
            if issue.priority.0 > max {
                continue;
            }
        }

        if let Some(ref needle) = desc_needle {
            let haystack = issue.description.as_deref().unwrap_or("").to_lowercase();
            if !haystack.contains(needle) {
                continue;
            }
        }

        if let Some(ref needle) = notes_needle {
            let haystack = issue.notes.as_deref().unwrap_or("").to_lowercase();
            if !haystack.contains(needle) {
                continue;
            }
        }

        if !include_deferred && matches!(issue.status, Status::Deferred) {
            continue;
        }

        if args.overdue {
            let overdue = issue.due_at.is_some_and(|due| due < now) && !issue.status.is_terminal();
            if !overdue {
                continue;
            }
        }

        if label_filters {
            let labels = storage.get_labels(&issue.id)?;
            if !args.label.is_empty() && !args.label.iter().all(|label| labels.contains(label)) {
                continue;
            }
            if !args.label_any.is_empty()
                && !args.label_any.iter().any(|label| labels.contains(label))
            {
                continue;
            }
        }

        filtered.push(issue);
    }

    Ok(filtered)
}

fn apply_sort(issues: &mut [IssueWithCounts], sort: Option<&str>) -> Result<()> {
    let Some(sort_key) = sort else {
        return Ok(());
    };

    match sort_key {
        "priority" => issues.sort_by_key(|iwc| iwc.issue.priority),
        "created_at" => issues.sort_by_key(|iwc| iwc.issue.created_at),
        "updated_at" => issues.sort_by_key(|iwc| iwc.issue.updated_at),
        "title" => issues.sort_by(|a, b| a.issue.title.cmp(&b.issue.title)),
        _ => {
            return Err(BeadsError::Validation {
                field: "sort".to_string(),
                reason: format!("invalid sort field '{sort_key}'"),
            });
        }
    }

    Ok(())
}
