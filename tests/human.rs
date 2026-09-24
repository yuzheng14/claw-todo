use std::{path::Path, process::Command};

use serde_json::Value;

fn command(db: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_claw-todo"));
    command.env_remove("CLAW_TODO_DB").env_remove("RUST_LOG");
    command.arg("--db").arg(db).args(args);
    command
}

fn json(db: &Path, args: &[&str]) -> Value {
    let output = command(db, args).arg("--json").output().unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{value}");
    assert_eq!(value["ok"], true);
    value["data"].clone()
}

fn human(db: &Path, args: &[&str]) -> String {
    let output = command(db, args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn filtered_tree_distinguishes_context_counts_and_closed_children() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("todos.db");
    let parent = json(&db, &["create", "root", "--category", "personal"]);
    let parent_id = parent["task"]["id"].as_str().unwrap();
    let child = json(
        &db,
        &[
            "create",
            "matching child",
            "--category",
            "work",
            "--parent",
            parent_id,
        ],
    );
    let child_id = child["task"]["id"].as_str().unwrap();
    let done = json(
        &db,
        &[
            "create",
            "finished sibling",
            "--category",
            "personal",
            "--parent",
            parent_id,
        ],
    );
    let done_id = done["task"]["id"].as_str().unwrap();
    json(&db, &["done", done_id]);
    let cancelled = json(
        &db,
        &[
            "create",
            "cancelled sibling",
            "--category",
            "personal",
            "--parent",
            parent_id,
        ],
    );
    let cancelled_id = cancelled["task"]["id"].as_str().unwrap();
    json(&db, &["cancel", cancelled_id]);

    let filtered = human(&db, &["list", "--category", "work"]);
    assert!(filtered.contains("Matched: 1 (open: 1)"));
    assert!(filtered.contains("top-level=1, total=2"));
    assert!(filtered.contains("[context only]"));
    assert!(filtered.contains("direct children: done=1, cancelled=1, total=3"));
    assert!(filtered.contains("└── [pending] matching child"));
    assert_eq!(filtered.matches(parent_id).count(), 1);
    assert_eq!(filtered.matches(child_id).count(), 1);
    assert!(!filtered.contains(done_id));
    assert!(!filtered.contains(cancelled_id));
    assert_eq!(filtered, human(&db, &["list", "--category", "work"]));

    let default = human(&db, &["list"]);
    assert!(default.contains("Matched: 2 (open: 2)"));
    assert!(!default.contains(done_id));
    assert!(!default.contains(cancelled_id));
    let all = human(&db, &["list", "--all"]);
    assert!(all.contains("Matched: 4 (open: 2)"));
    assert!(all.contains("[done] finished sibling"));
    assert!(all.contains("[cancelled] cancelled sibling"));
}

#[test]
fn detail_history_and_reminders_preserve_all_fields_without_terminal_controls() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("todos.db");
    let title = "标题\n伪造行\u{1b}[31m\u{202e}";
    let created = json(
        &db,
        &[
            "create",
            title,
            "--category",
            "personal",
            "--description",
            "first\nsecond",
            "--project",
            "project",
            "--source",
            "SPEC.md",
            "--source",
            "source\t2",
            "--creation-token",
            "human-test",
        ],
    );
    let id = created["task"]["id"].as_str().unwrap();
    let reminder = json(
        &db,
        &[
            "reminder",
            "add",
            id,
            "--external-id",
            "external\nreminder",
            "--at",
            "2026-10-01T08:00:00+08:00",
            "--timezone",
            "Asia/Shanghai",
            "--channel",
            "email",
        ],
    );
    let reminder_id = reminder["reminder"]["id"].as_str().unwrap();
    json(&db, &["block", id, "--reason", "waiting\nfor input"]);
    json(&db, &["note", id, "complete note\nline two\u{202e}"]);
    let detail = human(&db, &["show", id]);
    for expected in [
        "Title: 标题\\n伪造行\\u{1b}[31m\\u{202e}",
        "Description: first\\nsecond",
        "Status: blocked",
        "Category: personal",
        "Project: project",
        "Parent: (none)",
        "Blocked reason: waiting\\nfor input",
        "Cancel reason: (none)",
        "Created at:",
        "Updated at:",
        "Closed at: (none)",
        "Creation token: human-test",
        "Sources: 2",
        "  SPEC.md",
        "  source\\t2",
        "Direct children: done=0, cancelled=0, total=0",
        "Reminders: 1",
        "External ID: external\\nreminder",
        "Timezone: Asia/Shanghai",
        "Channel: email",
        "Status: scheduled",
        reminder_id,
    ] {
        assert!(detail.contains(expected), "missing {expected}: {detail}");
    }
    assert!(!detail.contains('\u{1b}'));
    assert!(!detail.contains('\u{202e}'));
    let history = human(&db, &["history", id]);
    assert!(history.contains("complete note\\nline two\\u{202e}"));
    assert!(history.contains("\"before\":"));
    assert!(history.contains("\"after\":"));
    assert!(history.contains("\"creation_token\": \"human-test\""));
    assert!(!history.contains('\u{202e}'));
    assert_eq!(json(&db, &["show", id])["task"]["title"], title);
    assert!(human(&db, &["reminder", "list", id]).contains("Reminder associations: 1"));
    assert!(
        human(&db, &["reminder", "remove", reminder_id])
            .contains("Removed local reminder association:")
    );
}

#[test]
fn operation_summaries_report_deduplication_noop_and_cascade_affected_ids() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("todos.db");
    let args = [
        "create",
        "parent",
        "--category",
        "work",
        "--creation-token",
        "parent-token",
    ];
    let parent = json(&db, &args);
    let parent_id = parent["task"]["id"].as_str().unwrap();
    assert!(human(&db, &args).contains("Existing task (creation deduplicated):"));
    assert!(human(&db, &["edit", parent_id, "--title", "parent"]).starts_with("Unchanged:"));
    let child = json(
        &db,
        &[
            "create",
            "child",
            "--category",
            "work",
            "--parent",
            parent_id,
        ],
    );
    let child_id = child["task"]["id"].as_str().unwrap();
    let closed = human(&db, &["done", parent_id, "--cascade"]);
    assert!(closed.starts_with("Updated:"));
    assert!(closed.contains("Affected tasks (2):"));
    assert!(closed.contains(&format!("  {parent_id}\n")));
    assert!(closed.contains(&format!("  {child_id}\n")));
    assert!(human(&db, &["list"]).contains("No matching tasks."));
    let repeated = human(&db, &["done", parent_id, "--cascade"]);
    assert!(repeated.starts_with("Unchanged:"));
    assert!(repeated.contains("Affected tasks (0):"));
}

#[test]
fn errors_escape_untrusted_messages_and_context_without_changing_json_values() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("todos.db");
    let missing = "missing\n\u{1b}[31m\u{202e}";
    let output = command(&db, &["show", missing]).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(!error.contains('\u{1b}'));
    assert!(!error.contains('\u{202e}'));
    assert!(error.contains("Error [NOT_FOUND]"));
    assert!(error.contains("\\u{202e}"));
    assert_eq!(error.lines().count(), 2);
    let output = command(&db, &["show", missing, "--json"]).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["context"]["id"], missing);
}
