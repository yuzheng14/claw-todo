CREATE TABLE tasks (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL CHECK (length(trim(title)) > 0),
    description TEXT,
    status TEXT NOT NULL CHECK (status IN ('pending', 'in_progress', 'blocked', 'done', 'cancelled')),
    category TEXT NOT NULL CHECK (category IN ('personal', 'work')),
    project TEXT,
    parent_id TEXT REFERENCES tasks(id) ON DELETE RESTRICT,
    blocked_reason TEXT,
    cancel_reason TEXT,
    sources TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(sources) AND json_type(sources) = 'array'),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    closed_at TEXT,
    token TEXT UNIQUE,
    creation_request TEXT NOT NULL CHECK (json_valid(creation_request)),
    CHECK (parent_id IS NULL OR parent_id != id),
    CHECK ((status = 'blocked' AND blocked_reason IS NOT NULL AND length(trim(blocked_reason)) > 0)
        OR (status != 'blocked' AND blocked_reason IS NULL)),
    CHECK (status = 'cancelled' OR cancel_reason IS NULL),
    CHECK ((status IN ('done', 'cancelled') AND closed_at IS NOT NULL)
        OR (status IN ('pending', 'in_progress', 'blocked') AND closed_at IS NULL))
);
CREATE INDEX tasks_parent ON tasks(parent_id);
CREATE INDEX tasks_status ON tasks(status);
CREATE INDEX tasks_category_project ON tasks(category, project);

CREATE TABLE history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('created', 'edited', 'parent_changed', 'note', 'status_changed', 'reminder_added', 'reminder_updated', 'reminder_removed')),
    at TEXT NOT NULL,
    changes TEXT NOT NULL CHECK (json_valid(changes))
);
CREATE INDEX history_task ON history(task_id, id);

CREATE TABLE reminders (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    external_id TEXT NOT NULL CHECK (length(trim(external_id)) > 0),
    scheduled_at TEXT NOT NULL,
    timezone TEXT NOT NULL CHECK (length(trim(timezone)) > 0),
    channel TEXT NOT NULL CHECK (length(trim(channel)) > 0),
    status TEXT NOT NULL CHECK (status IN ('scheduled', 'fired', 'cancelled', 'unknown')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX reminders_task ON reminders(task_id);

-- In addition to service validation, enforce the channel rule on every write path.
CREATE TRIGGER reminders_work_insert BEFORE INSERT ON reminders
WHEN NEW.channel = 'wechat' AND (SELECT category FROM tasks WHERE id = NEW.task_id) = 'work'
BEGIN SELECT RAISE(ABORT, 'work_wechat_forbidden'); END;
CREATE TRIGGER reminders_work_update BEFORE UPDATE ON reminders
WHEN NEW.channel = 'wechat' AND (SELECT category FROM tasks WHERE id = NEW.task_id) = 'work'
BEGIN SELECT RAISE(ABORT, 'work_wechat_forbidden'); END;
CREATE TRIGGER tasks_work_update BEFORE UPDATE OF category ON tasks
WHEN NEW.category = 'work' AND EXISTS (SELECT 1 FROM reminders WHERE task_id = NEW.id AND channel = 'wechat')
BEGIN SELECT RAISE(ABORT, 'work_wechat_forbidden'); END;

CREATE TRIGGER tasks_creation_immutable BEFORE UPDATE OF token, creation_request ON tasks
WHEN NEW.token IS NOT OLD.token OR NEW.creation_request != OLD.creation_request
BEGIN SELECT RAISE(ABORT, 'creation_request_immutable'); END;
