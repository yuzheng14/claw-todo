use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_claw-todo"));
    command.env_remove("CLAW_TODO_DB").env_remove("RUST_LOG");
    command
}

fn json(output: &Output, code: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("one complete JSON envelope");
    assert_eq!(value["ok"], code == 0);
    value
}

fn run(dir: &TempDir, args: &[&str], code: i32) -> Value {
    let output = command()
        .arg("--db")
        .arg(dir.path().join("todos.db"))
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    json(&output, code)
}

fn create(dir: &TempDir, title: &str) -> String {
    run(dir, &["create", title, "--category", "personal"], 0)["data"]["task"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn creation_edit_parent_and_complete_history() {
    let dir = tempfile::tempdir().unwrap();
    let args = [
        "create",
        "Child",
        "--category",
        "personal",
        "--creation-token",
        "request-1",
        "--description",
        "detail",
        "--project",
        "demo",
        "--source",
        "one",
        "--source",
        "two",
    ];
    let first = run(&dir, &args, 0);
    let id = first["data"]["task"]["id"].as_str().unwrap();
    assert_eq!(first["data"]["deduplicated"], false);
    let again = run(&dir, &args, 0);
    assert_eq!(again["data"]["deduplicated"], true);
    assert_eq!(again["data"]["task"]["id"], id);
    let conflict = run(
        &dir,
        &[
            "create",
            "Different",
            "--category",
            "personal",
            "--creation-token",
            "request-1",
        ],
        4,
    );
    assert_eq!(conflict["error"]["code"], "CREATION_TOKEN_CONFLICT");
    assert_eq!(conflict["error"]["context"]["task_id"], id);
    let edited = run(
        &dir,
        &[
            "edit",
            id,
            "--title",
            "Edited",
            "--clear-description",
            "--clear-project",
            "--clear-sources",
        ],
        0,
    );
    assert_eq!(edited["data"]["task"]["description"], Value::Null);
    assert_eq!(edited["data"]["task"]["project"], Value::Null);
    assert_eq!(edited["data"]["task"]["sources"], serde_json::json!([]));
    let parent = create(&dir, "Parent");
    assert_eq!(
        run(&dir, &["parent", id, "--parent", &parent], 0)["data"]["task"]["parent_id"],
        parent
    );
    let cycle = run(&dir, &["parent", &parent, "--parent", id], 4);
    assert_eq!(cycle["error"]["code"], "CYCLE_DETECTED");
    assert_eq!(
        run(&dir, &["parent", id, "--clear-parent"], 0)["data"]["task"]["parent_id"],
        Value::Null
    );
    run(&dir, &["note", id, "Progress update"], 0);
    let history = run(&dir, &["history", id], 0);
    let entries = history["data"].as_array().unwrap();
    assert_eq!(entries.len(), 5);
    assert_eq!(entries.last().unwrap()["kind"], "note");
    assert_eq!(
        entries.last().unwrap()["changes"]["body"],
        "Progress update"
    );
}

#[test]
fn statuses_cascade_and_typed_constraint_errors() {
    let dir = tempfile::tempdir().unwrap();
    let parent = create(&dir, "Parent");
    let child = create(&dir, "Child");
    run(&dir, &["parent", &child, "--parent", &parent], 0);
    run(&dir, &["start", &child], 0);
    assert_eq!(
        run(&dir, &["block", &child, "--reason", "Waiting"], 0)["data"]["task"]["status"],
        "blocked"
    );
    let refused = run(&dir, &["done", &parent], 4);
    assert_eq!(refused["error"]["code"], "OPEN_DESCENDANTS");
    assert_eq!(
        refused["error"]["context"]["blocking_task_ids"],
        serde_json::json!([child])
    );
    let done = run(&dir, &["done", &parent, "--cascade"], 0);
    assert_eq!(
        done["data"]["affected_task_ids"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        run(&dir, &["start", &child], 4)["error"]["code"],
        "INVALID_STATE"
    );
    assert_eq!(
        run(&dir, &["reopen", &child], 4)["error"]["code"],
        "CLOSED_ANCESTOR"
    );
    run(&dir, &["reopen", &parent], 0);
    run(&dir, &["reopen", &child], 0);
    let cancelled = run(
        &dir,
        &[
            "cancel",
            &parent,
            "--reason",
            "No longer needed",
            "--cascade",
        ],
        0,
    );
    assert_eq!(cancelled["data"]["task"]["status"], "cancelled");
    assert_eq!(
        cancelled["data"]["task"]["cancel_reason"],
        "No longer needed"
    );
    assert_eq!(
        run(
            &dir,
            &["cancel", &parent, "--reason", "No longer needed"],
            0
        )["data"]["changed"],
        false
    );
}

#[test]
fn lists_filters_context_progress_and_detail() {
    let dir = tempfile::tempdir().unwrap();
    let parent = create(&dir, "Parent");
    let child = run(
        &dir,
        &[
            "create",
            "Child",
            "--category",
            "work",
            "--parent",
            &parent,
            "--description",
            "Find me",
            "--project",
            "demo",
        ],
        0,
    );
    let child = child["data"]["task"]["id"].as_str().unwrap();
    run(&dir, &["done", child], 0);
    let open = run(&dir, &["list"], 0);
    assert_eq!(open["data"]["summary"]["matched"], 1);
    assert_eq!(open["data"]["summary"]["total_open"], 1);
    let filtered = run(
        &dir,
        &[
            "list",
            "--category",
            "work",
            "--project",
            "demo",
            "--search",
            "Find",
            "--status",
            "done,cancelled",
            "--status",
            "blocked",
        ],
        0,
    );
    assert_eq!(filtered["data"]["summary"]["matched"], 1);
    let views = filtered["data"]["tasks"].as_array().unwrap();
    assert_eq!(views.len(), 2);
    assert!(
        views
            .iter()
            .any(|v| v["id"] == parent && v["context_only"] == true)
    );
    assert_eq!(
        run(&dir, &["list", "--all"], 0)["data"]["summary"]["matched"],
        2
    );
    let detail = run(&dir, &["show", &parent], 0);
    assert_eq!(detail["data"]["progress"]["done"], 1);
    assert_eq!(detail["data"]["progress"]["cancelled"], 0);
    assert_eq!(detail["data"]["progress"]["total"], 1);
    assert_eq!(detail["data"]["reminders"], serde_json::json!([]));
}

#[test]
fn local_reminder_lifecycle_and_channel_rules() {
    let dir = tempfile::tempdir().unwrap();
    let id = create(&dir, "Reminder task");
    let added = run(
        &dir,
        &[
            "reminder",
            "add",
            &id,
            "--external-id",
            "external-1",
            "--at",
            "2026-12-01T10:00:00+08:00",
            "--timezone",
            "Asia/Shanghai",
            "--channel",
            "wechat",
        ],
        0,
    );
    let reminder = added["data"]["reminder"]["id"].as_str().unwrap();
    assert_eq!(
        run(&dir, &["reminder", "list", &id], 0)["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        run(&dir, &["edit", &id, "--category", "work"], 4)["error"]["code"],
        "CHANNEL_FORBIDDEN"
    );
    run(
        &dir,
        &[
            "reminder",
            "edit",
            reminder,
            "--external-id",
            "external-2",
            "--channel",
            "email",
            "--status",
            "unknown",
        ],
        0,
    );
    run(&dir, &["edit", &id, "--category", "work"], 0);
    assert_eq!(
        run(
            &dir,
            &["reminder", "edit", reminder, "--channel", "wechat"],
            4
        )["error"]["code"],
        "CHANNEL_FORBIDDEN"
    );
    let denied = run(
        &dir,
        &[
            "reminder",
            "add",
            &id,
            "--external-id",
            "other",
            "--at",
            "2026-12-01T10:00:00Z",
            "--timezone",
            "UTC",
            "--channel",
            "wechat",
        ],
        4,
    );
    assert_eq!(denied["error"]["code"], "CHANNEL_FORBIDDEN");
    assert_eq!(
        run(&dir, &["reminder", "fired", reminder], 0)["data"]["reminder"]["status"],
        "fired"
    );
    assert_eq!(
        run(&dir, &["reminder", "cancel", reminder], 0)["data"]["reminder"]["status"],
        "cancelled"
    );
    run(&dir, &["done", &id], 0);
    assert_eq!(
        run(&dir, &["show", &id], 0)["data"]["reminders"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    run(&dir, &["reminder", "remove", reminder], 0);
    assert_eq!(
        run(&dir, &["reminder", "list", &id], 0)["data"],
        serde_json::json!([])
    );
    assert_eq!(
        run(&dir, &["reminder", "remove", reminder], 3)["error"]["context"]["entity"],
        "reminder"
    );
}

#[test]
fn help_version_parser_errors_and_literal_json_values() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec![],
        vec!["--help"],
        vec!["--version"],
        vec!["reminder", "--help"],
    ] {
        let output = command()
            .arg("--db")
            .arg(dir.path().join("unused.db"))
            .args(args)
            .arg("--json")
            .output()
            .unwrap();
        json(&output, 0);
        assert!(!dir.path().join("unused.db").exists());
    }
    for args in [
        vec!["create", "Title"],
        vec!["list", "--bogus"],
        vec!["parent", "id"],
        vec!["parent", "id", "--parent", "other", "--clear-parent"],
        vec!["edit", "id", "--description", "new", "--clear-description"],
        vec!["unknown"],
    ] {
        let output = command().args(args).arg("--json").output().unwrap();
        assert_eq!(json(&output, 2)["error"]["code"], "INVALID_ARGUMENT");
    }
    let missing = run(&dir, &["show", "missing"], 3);
    assert_eq!(
        missing["error"]["context"],
        serde_json::json!({"entity": "task", "id": "missing"})
    );
    assert_eq!(
        run(&dir, &["create", " ", "--category", "personal"], 2)["error"]["code"],
        "INVALID_INPUT"
    );
    let id = create(&dir, "Literal flag");
    run(&dir, &["note", &id, "--", "--json"], 0);
    assert_eq!(
        run(&dir, &["edit", &id, "--title=--json"], 0)["data"]["task"]["title"],
        "--json"
    );
    let raw = command()
        .arg("--db")
        .arg(dir.path().join("todos.db"))
        .args(["note", "missing", "--", "--json"])
        .output()
        .unwrap();
    assert_eq!(raw.status.code(), Some(3));
    assert!(raw.stdout.is_empty());
    let raw = command()
        .args(["edit", "missing", "--title", "--json", "--bogus"])
        .output()
        .unwrap();
    assert_eq!(raw.status.code(), Some(2));
    assert!(raw.stdout.is_empty());
}

#[test]
fn explicit_path_overrides_environment() {
    let dir = tempfile::tempdir().unwrap();
    let env_db = dir.path().join("env.db");
    let explicit = dir.path().join("explicit.db");
    let output = command()
        .env("CLAW_TODO_DB", &env_db)
        .args(["list", "--db"])
        .arg(&explicit)
        .arg("--json")
        .output()
        .unwrap();
    json(&output, 0);
    assert!(explicit.exists());
    assert!(!env_db.exists());
    let output = command()
        .env("CLAW_TODO_DB", &env_db)
        .args(["list", "--json"])
        .output()
        .unwrap();
    json(&output, 0);
    assert!(env_db.exists());
}

#[test]
fn independent_cli_processes_deduplicate_creation_and_keep_json_stdout_clean() {
    let dir = tempfile::tempdir().unwrap();
    let mut children: Vec<_> = (0..6)
        .map(|_| {
            command()
                .env("RUST_LOG", "debug")
                .arg("--db")
                .arg(dir.path().join("concurrent.db"))
                .args([
                    "create",
                    "Concurrent",
                    "--category",
                    "personal",
                    "--creation-token",
                    "shared-request",
                    "--json",
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let responses: Vec<_> = children
        .drain(..)
        .map(|child| json(&child.wait_with_output().unwrap(), 0))
        .collect();
    assert_eq!(
        responses
            .iter()
            .filter(|value| value["data"]["deduplicated"] == false)
            .count(),
        1
    );
    let id = &responses[0]["data"]["task"]["id"];
    assert!(
        responses
            .iter()
            .all(|value| &value["data"]["task"]["id"] == id)
    );
    let output = command()
        .arg("--db")
        .arg(dir.path().join("concurrent.db"))
        .args(["history", id.as_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(json(&output, 0)["data"].as_array().unwrap().len(), 1);
}

// Unix directory providers honor the isolated HOME/XDG_DATA_HOME below.
// Windows resolves known folders through OS APIs, so do not touch its real user database.
#[cfg(unix)]
#[test]
fn default_database_is_shared_across_working_directories() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    for path in [&home, &first, &second] {
        std::fs::create_dir_all(path).unwrap();
    }
    let user_command = |cwd: &std::path::Path, args: &[&str]| {
        command()
            .env("HOME", &home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("USERPROFILE", &home)
            .env("LOCALAPPDATA", home.join("local"))
            .current_dir(cwd)
            .args(args)
            .arg("--json")
            .output()
            .unwrap()
    };
    let created = json(
        &user_command(&first, &["create", "Shared", "--category", "personal"]),
        0,
    );
    let listed = json(&user_command(&second, &["list"]), 0);
    assert_eq!(listed["data"]["summary"]["matched"], 1);
    assert_eq!(
        listed["data"]["tasks"][0]["id"],
        created["data"]["task"]["id"]
    );
    assert!(!first.join("todos.db").exists());
    assert!(!second.join("todos.db").exists());
}
