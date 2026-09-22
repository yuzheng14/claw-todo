use std::{ffi::OsString, path::PathBuf};

use clap::{Args, CommandFactory, Parser, Subcommand, error::ErrorKind};

use crate::{
    AddReminder, CreateTask, EditReminder, EditTask, ListFilter, Result, Store, Transition,
    db::default_db_path,
    output::{self, Response},
};

#[derive(Debug, Parser)]
#[command(
    name = "claw-todo",
    version,
    disable_help_subcommand = true,
    about = "Local todo state for humans and agents"
)]
pub struct Cli {
    /// Database file (defaults to the user data directory).
    #[arg(
        long,
        global = true,
        env = "CLAW_TODO_DB",
        value_name = "PATH",
        allow_hyphen_values = true
    )]
    pub db: Option<PathBuf>,
    /// Emit a stable JSON envelope, including on errors.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a pending task; repeated tokens compare the original request.
    Create(CreateArgs),
    /// Edit business fields without changing task status.
    Edit(EditArgs),
    /// Set or clear a parent without changing task state.
    Parent(ParentArgs),
    /// Start an open task.
    Start(TaskId),
    /// Block an open task with a reason.
    Block {
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        reason: String,
    },
    /// Complete a task, optionally completing all open descendants atomically.
    Done {
        id: String,
        #[arg(long)]
        cascade: bool,
    },
    /// Cancel a task, optionally cancelling all open descendants atomically.
    Cancel {
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        reason: Option<String>,
        #[arg(long)]
        cascade: bool,
    },
    /// Explicitly reopen a closed task as pending.
    Reopen(TaskId),
    /// Append a progress note to task history.
    Note { id: String, text: String },
    /// List all matching tasks, including ancestor context for a readable tree.
    List(ListArgs),
    /// Show every task field, direct-child progress, and reminder associations.
    Show(TaskId),
    /// Show the complete history for a task.
    History(TaskId),
    /// Manage local reminder associations; external reminders are never changed.
    Reminder {
        #[command(subcommand)]
        command: ReminderCommand,
    },
}

#[derive(Debug, Args)]
struct TaskId {
    id: String,
}

#[derive(Debug, Args)]
struct CreateArgs {
    title: String,
    #[arg(long, value_parser = ["personal", "work"])]
    category: String,
    #[arg(long, allow_hyphen_values = true)]
    description: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    project: Option<String>,
    #[arg(long, value_name = "TASK_ID", allow_hyphen_values = true)]
    parent: Option<String>,
    /// Source reference; repeat for multiple references.
    #[arg(long = "source", value_name = "REFERENCE", allow_hyphen_values = true)]
    sources: Vec<String>,
    #[arg(long, allow_hyphen_values = true)]
    creation_token: Option<String>,
}

#[derive(Debug, Args)]
struct EditArgs {
    id: String,
    #[arg(long, allow_hyphen_values = true)]
    title: Option<String>,
    #[arg(long, value_parser = ["personal", "work"])]
    category: Option<String>,
    #[arg(long, conflicts_with = "clear_description", allow_hyphen_values = true)]
    description: Option<String>,
    #[arg(long)]
    clear_description: bool,
    #[arg(long, conflicts_with = "clear_project", allow_hyphen_values = true)]
    project: Option<String>,
    #[arg(long)]
    clear_project: bool,
    /// Replace source references; repeat for multiple references.
    #[arg(
        long = "source",
        value_name = "REFERENCE",
        conflicts_with = "clear_sources",
        allow_hyphen_values = true
    )]
    sources: Vec<String>,
    #[arg(long)]
    clear_sources: bool,
}

#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
struct ParentChoice {
    #[arg(long, value_name = "TASK_ID", allow_hyphen_values = true)]
    parent: Option<String>,
    #[arg(long)]
    clear_parent: bool,
}

#[derive(Debug, Args)]
struct ParentArgs {
    id: String,
    #[command(flatten)]
    choice: ParentChoice,
}

#[derive(Debug, Default, Args)]
struct ListArgs {
    /// Include closed tasks when no explicit status filter is supplied.
    #[arg(long)]
    all: bool,
    #[arg(long, value_parser = ["personal", "work"])]
    category: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    project: Option<String>,
    /// Filter statuses; comma-separated or repeatable (default: open statuses).
    #[arg(long = "status", value_delimiter = ',', value_parser = ["pending", "in_progress", "blocked", "done", "cancelled"])]
    statuses: Vec<String>,
    /// Search title and description for a literal substring.
    #[arg(long, allow_hyphen_values = true)]
    search: Option<String>,
}

#[derive(Debug, Subcommand)]
enum ReminderCommand {
    /// Associate an external reminder after the caller creates it.
    Add {
        task: String,
        #[arg(long, allow_hyphen_values = true)]
        external_id: String,
        /// Scheduled instant in RFC 3339 format, including its UTC offset.
        #[arg(long, allow_hyphen_values = true)]
        at: String,
        #[arg(long, allow_hyphen_values = true)]
        timezone: String,
        #[arg(long, allow_hyphen_values = true)]
        channel: String,
        #[arg(long, default_value = "scheduled", value_parser = ["scheduled", "fired", "cancelled", "unknown"])]
        status: String,
    },
    /// List every reminder associated with a task.
    List { task: String },
    /// Update local association information after the external operation succeeds.
    Edit {
        reminder: String,
        #[arg(long, allow_hyphen_values = true)]
        external_id: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        at: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        timezone: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        channel: Option<String>,
        #[arg(long, value_parser = ["scheduled", "fired", "cancelled", "unknown"])]
        status: Option<String>,
    },
    /// Record that an external reminder has fired.
    Fired { reminder: String },
    /// Record that an external reminder was successfully cancelled.
    Cancel { reminder: String },
    /// Remove a local association after external deletion succeeds.
    Remove { reminder: String },
}

/// clap can exit before reaching a trailing global flag on help or bad input.
/// Inspect the same argument definitions, without confusing option values or
/// text after `--` with a request for JSON.
fn requests_json(args: &[OsString]) -> bool {
    let root = Cli::command();
    let mut command = &root;
    let mut args = args.iter().skip(1).peekable();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if arg == "--json" {
            return true;
        }
        let Some(arg) = arg.to_str() else { continue };
        if let Some(name) = arg.strip_prefix("--") {
            if name.contains('=') {
                continue;
            }
            let definition = command
                .get_arguments()
                .chain(root.get_arguments())
                .find(|item| item.get_long() == Some(name));
            if let Some(definition) = definition
                && definition.get_action().takes_values()
                && args.peek().is_some_and(|next| {
                    definition.is_allow_hyphen_values_set()
                        || !next.to_string_lossy().starts_with('-')
                })
            {
                args.next();
            }
        } else if let Some(next) = command.find_subcommand(arg) {
            command = next;
        }
    }
    false
}

fn optional_edit(value: Option<String>, clear: bool) -> Option<Option<String>> {
    if clear { Some(None) } else { value.map(Some) }
}

async fn dispatch(store: &Store, command: Command) -> Result<Response> {
    Ok(match command {
        Command::Create(args) => Response::Created(
            store
                .create(CreateTask {
                    title: args.title,
                    category: args.category,
                    description: args.description,
                    project: args.project,
                    parent_id: args.parent,
                    sources: args.sources,
                    creation_token: args.creation_token,
                })
                .await?,
        ),
        Command::Edit(args) => Response::Changed(
            store
                .edit(
                    &args.id,
                    EditTask {
                        title: args.title,
                        category: args.category,
                        description: optional_edit(args.description, args.clear_description),
                        project: optional_edit(args.project, args.clear_project),
                        sources: if args.clear_sources {
                            Some(Vec::new())
                        } else if args.sources.is_empty() {
                            None
                        } else {
                            Some(args.sources)
                        },
                    },
                )
                .await?,
        ),
        Command::Parent(args) => Response::Changed(
            store
                .set_parent(&args.id, args.choice.parent.as_deref())
                .await?,
        ),
        Command::Start(args) => {
            Response::Transitioned(store.transition(&args.id, Transition::Start).await?)
        }
        Command::Block { id, reason } => {
            Response::Transitioned(store.transition(&id, Transition::Block { reason }).await?)
        }
        Command::Done { id, cascade } => {
            Response::Transitioned(store.transition(&id, Transition::Done { cascade }).await?)
        }
        Command::Cancel {
            id,
            reason,
            cascade,
        } => Response::Transitioned(
            store
                .transition(&id, Transition::Cancel { reason, cascade })
                .await?,
        ),
        Command::Reopen(args) => {
            Response::Transitioned(store.transition(&args.id, Transition::Reopen).await?)
        }
        Command::Note { id, text } => Response::Changed(store.note(&id, &text).await?),
        Command::List(args) => Response::Listed(
            store
                .list(ListFilter {
                    all: args.all,
                    category: args.category,
                    project: args.project,
                    statuses: args.statuses,
                    search: args.search,
                })
                .await?,
        ),
        Command::Show(args) => Response::Detail(store.show(&args.id).await?),
        Command::History(args) => Response::History(store.history(&args.id).await?),
        Command::Reminder { command } => match command {
            ReminderCommand::Add {
                task,
                external_id,
                at,
                timezone,
                channel,
                status,
            } => Response::ReminderChanged(
                store
                    .add_reminder(
                        &task,
                        AddReminder {
                            external_id,
                            scheduled_at: at,
                            timezone,
                            channel,
                            status,
                        },
                    )
                    .await?,
            ),
            ReminderCommand::List { task } => {
                Response::Reminders(store.list_reminders(&task).await?)
            }
            ReminderCommand::Edit {
                reminder,
                external_id,
                at,
                timezone,
                channel,
                status,
            } => Response::ReminderChanged(
                store
                    .edit_reminder(
                        &reminder,
                        EditReminder {
                            external_id,
                            scheduled_at: at,
                            timezone,
                            channel,
                            status,
                        },
                    )
                    .await?,
            ),
            ReminderCommand::Fired { reminder } => Response::ReminderChanged(
                store
                    .edit_reminder(
                        &reminder,
                        EditReminder {
                            status: Some("fired".into()),
                            ..Default::default()
                        },
                    )
                    .await?,
            ),
            ReminderCommand::Cancel { reminder } => Response::ReminderChanged(
                store
                    .edit_reminder(
                        &reminder,
                        EditReminder {
                            status: Some("cancelled".into()),
                            ..Default::default()
                        },
                    )
                    .await?,
            ),
            ReminderCommand::Remove { reminder } => {
                Response::ReminderRemoved(store.remove_reminder(&reminder).await?)
            }
        },
    })
}

/// Run a single noninteractive invocation, returning its documented exit code.
pub async fn run() -> u8 {
    let args: Vec<_> = std::env::args_os().collect();
    let json_requested = requests_json(&args);
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let response = match error.kind() {
                ErrorKind::DisplayHelp => Some(Response::Help(error.to_string())),
                ErrorKind::DisplayVersion => Some(Response::Version(error.to_string())),
                _ => None,
            };
            return match response {
                Some(response) => finish(Ok(response), json_requested),
                None => {
                    if let Err(error) =
                        output::write_argument_error(&error.to_string(), json_requested)
                    {
                        tracing::error!(%error, "Could not write argument error");
                    }
                    2
                }
            };
        }
    };
    let Some(command) = cli.command else {
        return finish(
            Ok(Response::Help(
                Cli::command().render_long_help().to_string(),
            )),
            cli.json,
        );
    };
    let result = async {
        let path = match cli.db {
            Some(path) => path,
            None => default_db_path()?,
        };
        let store = Store::open(path).await?;
        let result = dispatch(&store, command).await;
        store.close().await;
        result
    }
    .await;
    finish(result, cli.json)
}

fn finish(result: Result<Response>, json: bool) -> u8 {
    match result {
        Ok(response) => match output::write_success(&response, json) {
            Ok(()) => 0,
            Err(error) => {
                tracing::error!(%error, "Could not write command output");
                output::write_failure_exit_code(&error)
            }
        },
        Err(error) => {
            let exit_code = output::exit_code(&error);
            if let Err(write_error) = output::write_error(&error, json) {
                tracing::error!(%write_error, "Could not write command error");
            }
            exit_code
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn json_detection_respects_literal_arguments() {
        let args = |args: &[&str]| args.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(requests_json(&args(&["claw-todo", "list", "--json"])));
        assert!(!requests_json(&args(&[
            "claw-todo",
            "note",
            "id",
            "--",
            "--json"
        ])));
        assert!(requests_json(&args(&[
            "claw-todo",
            "--json",
            "note",
            "id",
            "--",
            "--json"
        ])));
        for value in [vec!["--title=--json"], vec!["--title", "--json"]] {
            let mut input = vec!["claw-todo", "edit", "id"];
            input.extend(value);
            assert!(!requests_json(&args(&input)));
            let parsed = Cli::try_parse_from(&input).unwrap();
            assert!(!parsed.json);
            input.push("--json");
            assert!(requests_json(&args(&input)));
            assert!(Cli::try_parse_from(&input).unwrap().json);
        }
    }

    #[test]
    fn editing_and_clearing_the_same_field_is_rejected() {
        for (flag, clear) in [
            ("--description", "--clear-description"),
            ("--project", "--clear-project"),
            ("--source", "--clear-sources"),
        ] {
            let result = Cli::try_parse_from(["claw-todo", "edit", "id", flag, "value", clear]);
            assert_eq!(result.unwrap_err().kind(), ErrorKind::ArgumentConflict);
        }
    }

    #[test]
    fn status_filters_accept_commas_and_repetition() {
        let cli = Cli::try_parse_from([
            "claw-todo",
            "list",
            "--status",
            "done,cancelled",
            "--status",
            "blocked",
        ])
        .unwrap();
        let Some(Command::List(args)) = cli.command else {
            panic!("expected list");
        };
        assert_eq!(args.statuses, ["done", "cancelled", "blocked"]);
    }
}
