use std::{collections::HashMap, fmt::Write as _};

use crate::{ListResult, Progress, Reminder, Task};

use crate::output::Response;

/// Human output is deliberately separate from the machine-readable contract.
pub(crate) fn render(response: &Response) -> String {
    let mut text = String::new();
    match response {
        Response::Created(result) => render_change(
            &mut text,
            if result.deduplicated {
                "Existing task (creation deduplicated)"
            } else {
                "Created"
            },
            &result.task,
        ),
        Response::Changed(result) => render_change(
            &mut text,
            if result.changed {
                "Updated"
            } else {
                "Unchanged"
            },
            &result.task,
        ),
        Response::Transitioned(result) => {
            render_change(
                &mut text,
                if result.changed {
                    "Updated"
                } else {
                    "Unchanged"
                },
                &result.task,
            );
            let _ = writeln!(text, "Affected tasks ({}):", result.affected_task_ids.len());
            for id in &result.affected_task_ids {
                let _ = writeln!(text, "  {}", human_text(id));
            }
        }
        Response::Listed(result) => render_tree(&mut text, result),
        Response::Detail(detail) => {
            render_task(&mut text, &detail.task);
            render_progress(&mut text, &detail.progress);
            let _ = writeln!(text, "Reminders: {}", detail.reminders.len());
            for reminder in &detail.reminders {
                render_reminder(&mut text, reminder, "  ");
            }
        }
        Response::History(entries) => {
            if entries.is_empty() {
                text.push_str("No history.\n");
            }
            for entry in entries {
                let _ = writeln!(
                    text,
                    "{}  {}  #{}",
                    human_text(&entry.at),
                    human_text(&entry.kind),
                    entry.id
                );
                let _ = writeln!(text, "  Task: {}", human_text(&entry.task_id));
                let changes = serde_json::to_string_pretty(&entry.changes.0)
                    .expect("a serde_json::Value is always serializable");
                for line in changes.lines() {
                    // JSON escapes control characters, but not bidi controls.
                    let _ = writeln!(text, "  {}", human_text(line));
                }
            }
        }
        Response::ReminderChanged(result) => {
            let _ = writeln!(
                text,
                "{} reminder association:",
                if result.changed {
                    "Updated"
                } else {
                    "Unchanged"
                }
            );
            render_reminder(&mut text, &result.reminder, "");
        }
        Response::Reminders(reminders) => {
            let _ = writeln!(text, "Reminder associations: {}", reminders.len());
            for reminder in reminders {
                render_reminder(&mut text, reminder, "");
            }
        }
        Response::ReminderRemoved(reminder) => {
            text.push_str("Removed local reminder association:\n");
            render_reminder(&mut text, reminder, "");
        }
        Response::Help(value) | Response::Version(value) => {
            text.push_str(value);
            if !text.ends_with('\n') {
                text.push('\n');
            }
        }
    }
    text
}

/// Keep untrusted stored fields on one display line; never emit terminal escapes.
pub(crate) fn human_text(value: &str) -> String {
    let mut text = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_control()
            || matches!(ch, '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            text.extend(ch.escape_default());
        } else {
            text.push(ch);
        }
    }
    text
}

fn optional(value: Option<&str>) -> String {
    value.map(human_text).unwrap_or_else(|| "(none)".into())
}

fn render_change(text: &mut String, action: &str, task: &Task) {
    let _ = writeln!(
        text,
        "{action}: {} [{}] {}",
        human_text(&task.id),
        human_text(&task.status),
        human_text(&task.title)
    );
}

fn render_task(text: &mut String, task: &Task) {
    let _ = writeln!(text, "ID: {}", human_text(&task.id));
    let _ = writeln!(text, "Title: {}", human_text(&task.title));
    let _ = writeln!(
        text,
        "Description: {}",
        optional(task.description.as_deref())
    );
    let _ = writeln!(text, "Status: {}", human_text(&task.status));
    let _ = writeln!(text, "Category: {}", human_text(&task.category));
    let _ = writeln!(text, "Project: {}", optional(task.project.as_deref()));
    let _ = writeln!(text, "Parent: {}", optional(task.parent_id.as_deref()));
    let _ = writeln!(
        text,
        "Blocked reason: {}",
        optional(task.blocked_reason.as_deref())
    );
    let _ = writeln!(
        text,
        "Cancel reason: {}",
        optional(task.cancel_reason.as_deref())
    );
    let _ = writeln!(text, "Created at: {}", human_text(&task.created_at));
    let _ = writeln!(text, "Updated at: {}", human_text(&task.updated_at));
    let _ = writeln!(text, "Closed at: {}", optional(task.closed_at.as_deref()));
    let _ = writeln!(
        text,
        "Creation token: {}",
        optional(task.creation_token.as_deref())
    );
    let _ = writeln!(text, "Sources: {}", task.sources.0.len());
    for source in &task.sources.0 {
        let _ = writeln!(text, "  {}", human_text(source));
    }
}

fn render_progress(text: &mut String, progress: &Progress) {
    let _ = writeln!(
        text,
        "Direct children: done={}, cancelled={}, total={}",
        progress.done, progress.cancelled, progress.total
    );
}

fn render_reminder(text: &mut String, reminder: &Reminder, indent: &str) {
    for (name, value) in [
        ("ID", &reminder.id),
        ("Task", &reminder.task_id),
        ("External ID", &reminder.external_id),
        ("Scheduled at", &reminder.scheduled_at),
        ("Timezone", &reminder.timezone),
        ("Channel", &reminder.channel),
        ("Status", &reminder.status),
        ("Created at", &reminder.created_at),
        ("Updated at", &reminder.updated_at),
    ] {
        let _ = writeln!(text, "{indent}{name}: {}", human_text(value));
    }
}

fn render_tree(text: &mut String, result: &ListResult) {
    let _ = writeln!(
        text,
        "Matched: {} (open: {})",
        result.summary.matched, result.summary.matched_open
    );
    let _ = writeln!(
        text,
        "Open across database: top-level={}, total={}",
        result.summary.top_level_open, result.summary.total_open
    );
    if result.tasks.is_empty() {
        text.push_str("No matching tasks.\n");
        return;
    }
    let indices: HashMap<&str, usize> = result
        .tasks
        .iter()
        .enumerate()
        .map(|(index, view)| (view.task.id.as_str(), index))
        .collect();
    let mut children = vec![Vec::new(); result.tasks.len()];
    let mut roots = Vec::new();
    for (index, view) in result.tasks.iter().enumerate() {
        match view
            .task
            .parent_id
            .as_deref()
            .and_then(|parent| indices.get(parent))
        {
            Some(&parent) => children[parent].push(index),
            None => roots.push(index),
        }
    }
    // Keep store order for roots/siblings. Explicit DFS needs no recursive calls
    // and stores the current ancestry once, even for deeply nested trees.
    let mut stack: Vec<_> = roots
        .into_iter()
        .rev()
        .map(|index| (index, 0, true))
        .collect();
    let mut last_ancestors = Vec::new();
    while let Some((index, depth, is_last)) = stack.pop() {
        if depth == 0 {
            last_ancestors.clear();
        } else {
            last_ancestors.truncate(depth - 1);
            last_ancestors.push(is_last);
        }
        for (position, is_last) in last_ancestors.iter().enumerate() {
            text.push_str(if position + 1 == depth {
                if *is_last { "└── " } else { "├── " }
            } else if *is_last {
                "    "
            } else {
                "│   "
            });
        }
        let view = &result.tasks[index];
        let _ = write!(
            text,
            "[{}] {} ({}) category={}",
            human_text(&view.task.status),
            human_text(&view.task.title),
            human_text(&view.task.id),
            human_text(&view.task.category)
        );
        if let Some(project) = &view.task.project {
            let _ = write!(text, " project={}", human_text(project));
        }
        if view.context_only {
            text.push_str(" [context only]");
        }
        if view.progress.total > 0 {
            let _ = write!(
                text,
                " [direct children: done={}, cancelled={}, total={}]",
                view.progress.done, view.progress.cancelled, view.progress.total
            );
        }
        text.push('\n');
        for (position, &child) in children[index].iter().enumerate().rev() {
            stack.push((child, depth + 1, position + 1 == children[index].len()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HistoryEntry, ListSummary, TaskView};
    use serde_json::json;
    use sqlx::types::Json;

    fn task(id: &str, title: &str, parent: Option<&str>, context_only: bool) -> TaskView {
        TaskView {
            task: Task {
                id: id.into(),
                title: title.into(),
                parent_id: parent.map(str::to_string),
                description: None,
                status: "pending".into(),
                category: "personal".into(),
                project: None,
                blocked_reason: None,
                cancel_reason: None,
                sources: Json(vec![]),
                created_at: "now".into(),
                updated_at: "now".into(),
                closed_at: None,
                creation_token: None,
            },
            context_only,
            progress: Progress::default(),
        }
    }

    #[test]
    fn tree_preserves_context_branching_and_direct_child_counts() {
        let mut parent = task("parent", "Parent", None, true);
        parent.progress = Progress {
            done: 1,
            cancelled: 2,
            total: 5,
        };
        let response = Response::Listed(ListResult {
            tasks: vec![
                task("child", "Child", Some("parent"), false),
                parent,
                task("grandchild", "Grandchild", Some("child"), false),
                task("sibling", "Sibling", Some("parent"), false),
                task("other", "Other", None, false),
            ],
            summary: ListSummary {
                matched: 4,
                matched_open: 4,
                top_level_open: 2,
                total_open: 5,
            },
        });
        let text = render(&response);
        assert!(text.contains("Matched: 4 (open: 4)"));
        assert!(text.contains("top-level=2, total=5"));
        assert!(text.contains("[context only] [direct children: done=1, cancelled=2, total=5]"));
        assert!(text.contains("├── [pending] Child (child)"));
        assert!(text.contains("│   └── [pending] Grandchild (grandchild)"));
        assert!(text.contains("└── [pending] Sibling (sibling)"));
        assert!(text.contains("\n[pending] Other (other)"));
        assert_eq!(text.matches("Child (child)").count(), 1);
        assert_eq!(text, render(&response));
    }

    #[test]
    fn moved_older_child_renders_below_its_newer_parent_once() {
        let mut child = task("child", "Older child", Some("parent"), false);
        child.task.created_at = "2026-09-20T00:00:00.000000Z".into();
        let mut parent = task("parent", "Newer parent", None, false);
        parent.task.created_at = "2026-09-21T00:00:00.000000Z".into();
        let text = render(&Response::Listed(ListResult {
            // Store order is created_at/id, not tree order. A reparented child
            // may precede its new parent even though both match the query.
            tasks: vec![child, parent],
            summary: ListSummary {
                matched: 2,
                matched_open: 2,
                top_level_open: 1,
                total_open: 2,
            },
        }));
        let rows: Vec<_> = text.lines().skip(2).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], "[pending] Newer parent (parent) category=personal");
        assert_eq!(
            rows[1],
            "└── [pending] Older child (child) category=personal"
        );
        assert_eq!(text.matches("(parent)").count(), 1);
        assert_eq!(text.matches("(child)").count(), 1);
    }

    #[test]
    fn matching_parent_and_child_share_one_context_only_grandparent() {
        let text = render(&Response::Listed(ListResult {
            tasks: vec![
                task("child", "Child", Some("parent"), false),
                task("parent", "Parent", Some("grandparent"), false),
                task("grandparent", "Grandparent", None, true),
            ],
            summary: ListSummary {
                matched: 2,
                matched_open: 2,
                top_level_open: 1,
                total_open: 3,
            },
        }));
        assert!(text.contains("Matched: 2 (open: 2)"));
        assert!(
            text.contains("[pending] Grandparent (grandparent) category=personal [context only]")
        );
        assert!(text.contains("\n└── [pending] Parent (parent) category=personal\n"));
        assert!(text.contains("\n    └── [pending] Child (child) category=personal\n"));
        assert_eq!(text.matches("[context only]").count(), 1);
        for id in ["(grandparent)", "(parent)", "(child)"] {
            assert_eq!(text.matches(id).count(), 1);
        }
    }

    #[test]
    fn controls_cannot_insert_tree_lines_or_terminal_escapes() {
        let malicious = "line\nnext\t\u{1b}[31m\u{202e}end\u{2066}";
        let safe = "line\\nnext\\t\\u{1b}[31m\\u{202e}end\\u{2066}";
        assert_eq!(human_text(malicious), safe);
        assert_eq!(human_text("中文标题"), "中文标题");
        let text = render(&Response::History(vec![HistoryEntry {
            id: 1,
            task_id: "task".into(),
            at: "now".into(),
            kind: "note".into(),
            changes: Json(
                json!({ "body": malicious, "before": null, "after": { "title": "保留完整快照" } }),
            ),
        }]));
        assert!(!text.contains('\u{202e}'));
        assert!(!text.contains('\u{1b}'));
        assert!(text.contains("保留完整快照"));
        assert!(text.contains("\\u{202e}"));
    }

    #[test]
    fn deeply_nested_trees_are_rendered_in_full_without_recursion() {
        let depth = 1200;
        let tasks = (0..depth)
            .map(|index| {
                let parent = (index > 0).then(|| format!("id-{}", index - 1));
                task(
                    &format!("id-{index}"),
                    &format!("Task {index}"),
                    parent.as_deref(),
                    false,
                )
            })
            .collect();
        let text = render(&Response::Listed(ListResult {
            tasks,
            summary: ListSummary {
                matched: depth,
                matched_open: depth,
                top_level_open: 1,
                total_open: depth,
            },
        }));
        assert_eq!(text.lines().count(), depth + 2);
        assert!(text.contains("Task 1199 (id-1199)"));
    }
}
