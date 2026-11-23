//! Archive logic: pure functions over the board document and the archive list.
//!
//! The board document is kept as a [`serde_json::Value`] so fields this server
//! does not know about round-trip untouched. Every function here is
//! deterministic and takes the current time as a parameter; the routes do the
//! file I/O around them.
//!
//! Tasks are matched by their string `id`. Tasks that lack a string `id`, or
//! whose `updatedAt` / `archivedAt` is missing or not RFC 3339, are never
//! swept, archived or deleted automatically: when unsure, keep the data.

use std::collections::HashSet;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::Value;

use crate::config::ArchiveConfig;

/// What a [`sweep`] changed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepReport {
    /// Board tasks dropped because their id is already archived.
    pub dropped: usize,
    /// Tasks moved from the board into the archive.
    pub archived: usize,
    /// Archived tasks deleted permanently.
    pub deleted: usize,
}

impl SweepReport {
    /// Whether anything changed, i.e. the files need to be written.
    pub fn changed(&self) -> bool {
        self.dropped + self.archived + self.deleted > 0
    }
}

/// Formats a timestamp like JavaScript's `Date.toISOString()`.
pub fn timestamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Returns a task's string `id`, if it has one.
pub fn task_id(task: &Value) -> Option<&str> {
    task.get("id")?.as_str()
}

/// Parses an RFC 3339 timestamp field of a task.
fn time_field(task: &Value, field: &str) -> Option<DateTime<Utc>> {
    let s = task.get(field)?.as_str()?;
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// Returns the board's `tasks` array, if the document has one.
fn tasks_mut(board: &mut Value) -> Option<&mut Vec<Value>> {
    board.get_mut("tasks")?.as_array_mut()
}

/// The ids of all archived tasks.
fn archived_ids(archive: &[Value]) -> HashSet<String> {
    archive
        .iter()
        .filter_map(task_id)
        .map(str::to_owned)
        .collect()
}

/// Removes board tasks whose id is in the archive (the "tombstone" rule) and
/// returns how many were removed.
///
/// Applied to every `PUT /api/data` so a stale client cannot resurrect tasks
/// that were archived after it last loaded the board.
pub fn drop_archived(board: &mut Value, archive: &[Value]) -> usize {
    let ids = archived_ids(archive);
    let Some(tasks) = tasks_mut(board) else {
        return 0;
    };
    let before = tasks.len();
    tasks.retain(|t| task_id(t).is_none_or(|id| !ids.contains(id)));
    before - tasks.len()
}

/// Moves the board tasks matching `pred` into the archive, stamping
/// `archivedAt`. Newly archived tasks go to the front (newest first).
fn move_to_archive(
    board: &mut Value,
    archive: &mut Vec<Value>,
    now: DateTime<Utc>,
    mut pred: impl FnMut(&Value) -> bool,
) -> usize {
    let Some(tasks) = tasks_mut(board) else {
        return 0;
    };
    let mut moved = Vec::new();
    tasks.retain(|t| {
        if task_id(t).is_some() && pred(t) {
            moved.push(t.clone());
            false
        } else {
            true
        }
    });
    let n = moved.len();
    let stamp = Value::String(timestamp(now));
    for t in &mut moved {
        if let Some(obj) = t.as_object_mut() {
            obj.insert("archivedAt".to_owned(), stamp.clone());
        }
    }
    archive.splice(0..0, moved);
    n
}

/// Runs the lazy archive sweep.
///
/// 1. Drops board tasks that are already archived ([`drop_archived`]); this
///    also heals the state after a crash between the two file writes.
/// 2. If enabled, moves tasks in `done_column` whose `updatedAt` is at least
///    `after_days` old into the archive.
/// 3. If enabled and `delete_after_days > 0`, deletes archived tasks whose
///    `archivedAt` is at least that old.
pub fn sweep(
    board: &mut Value,
    archive: &mut Vec<Value>,
    cfg: &ArchiveConfig,
    now: DateTime<Utc>,
) -> SweepReport {
    let mut report = SweepReport {
        dropped: drop_archived(board, archive),
        ..Default::default()
    };
    if !cfg.enabled {
        return report;
    }
    if let Some(cutoff) = days_before(now, cfg.after_days) {
        report.archived = move_to_archive(board, archive, now, |t| {
            t.get("status").and_then(Value::as_str) == Some(cfg.done_column.as_str())
                && time_field(t, "updatedAt").is_some_and(|u| u <= cutoff)
        });
    }
    if cfg.delete_after_days > 0
        && let Some(cutoff) = days_before(now, cfg.delete_after_days)
    {
        let before = archive.len();
        archive.retain(|t| time_field(t, "archivedAt").is_none_or(|a| a > cutoff));
        report.deleted = before - archive.len();
    }
    report
}

/// `now` minus `days`, or `None` if that is before the earliest representable
/// time, in which case nothing is old enough.
fn days_before(now: DateTime<Utc>, days: u32) -> Option<DateTime<Utc>> {
    now.checked_sub_signed(Duration::try_days(days.into())?)
}

/// Moves the board tasks with the given ids into the archive (`POST
/// /api/archive`). Unknown ids are ignored. Returns how many were archived.
pub fn archive_tasks(
    board: &mut Value,
    archive: &mut Vec<Value>,
    ids: &[String],
    now: DateTime<Utc>,
) -> usize {
    let ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
    move_to_archive(board, archive, now, |t| {
        task_id(t).is_some_and(|id| ids.contains(id))
    })
}

/// Moves archived tasks with the given ids back onto the board (`POST
/// /api/archive/restore`).
///
/// `archivedAt` is removed and `updatedAt` set to `now`, so the next sweep does
/// not archive the task straight away. Status and context are kept. Unknown ids
/// are ignored; nothing is restored if the board document has no place for
/// tasks (it is not an object). Returns how many were restored.
pub fn restore_tasks(
    board: &mut Value,
    archive: &mut Vec<Value>,
    ids: &[String],
    now: DateTime<Utc>,
) -> usize {
    let Some(obj) = board.as_object_mut() else {
        return 0;
    };
    let tasks = obj
        .entry("tasks")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(tasks) = tasks.as_array_mut() else {
        return 0;
    };
    let ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let mut restored = Vec::new();
    archive.retain(|t| {
        if task_id(t).is_some_and(|id| ids.contains(id)) {
            restored.push(t.clone());
            false
        } else {
            true
        }
    });
    let restored_ids: HashSet<String> = restored
        .iter()
        .filter_map(task_id)
        .map(str::to_owned)
        .collect();
    tasks.retain(|t| task_id(t).is_none_or(|id| !restored_ids.contains(id)));
    let n = restored.len();
    for mut t in restored {
        if let Some(o) = t.as_object_mut() {
            o.remove("archivedAt");
            o.insert("updatedAt".to_owned(), Value::String(timestamp(now)));
        }
        tasks.push(t);
    }
    n
}

/// Sorts archived tasks newest `archivedAt` first; entries without a valid
/// timestamp go last. The sort is stable.
pub fn sort_newest_first(archive: &mut [Value]) {
    archive.sort_by_key(|t| std::cmp::Reverse(time_field(t, "archivedAt")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-03-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn cfg() -> ArchiveConfig {
        ArchiveConfig::default()
    }

    fn ids(v: &[Value]) -> Vec<&str> {
        v.iter().filter_map(task_id).collect()
    }

    #[test]
    fn sweeps_only_old_done_tasks() {
        let mut board = json!({ "tasks": [
            { "id": "old-done", "status": "done", "updatedAt": "2026-02-01T00:00:00.000Z", "extra": [1] },
            { "id": "edge", "status": "done", "updatedAt": "2026-02-15T12:00:00Z" },
            { "id": "new-done", "status": "done", "updatedAt": "2026-02-28T00:00:00Z" },
            { "id": "old-todo", "status": "todo", "updatedAt": "2020-01-01T00:00:00Z" },
            { "id": "no-date", "status": "done" },
            { "id": "bad-date", "status": "done", "updatedAt": "last tuesday" },
            { "id": "num-date", "status": "done", "updatedAt": 0 },
            { "status": "done", "updatedAt": "2020-01-01T00:00:00Z" },
            "not even an object"
        ], "columns": [], "unknown": { "kept": true } });
        let mut archive = vec![];
        let r = sweep(&mut board, &mut archive, &cfg(), now());
        assert_eq!(
            r,
            SweepReport {
                dropped: 0,
                archived: 2,
                deleted: 0
            }
        );
        assert_eq!(ids(&archive), ["old-done", "edge"]);
        assert_eq!(archive[0]["archivedAt"], "2026-03-01T12:00:00.000Z");
        assert_eq!(archive[0]["extra"], json!([1]));
        assert_eq!(board["tasks"].as_array().unwrap().len(), 7);
        assert_eq!(board["unknown"], json!({ "kept": true }));
    }

    #[test]
    fn honours_done_column_and_disabled() {
        let task = json!({ "id": "a", "status": "finished", "updatedAt": "2020-01-01T00:00:00Z" });
        let mut board = json!({ "tasks": [task] });
        let mut archive = vec![];
        assert!(!sweep(&mut board, &mut archive, &cfg(), now()).changed());
        let mut c = cfg();
        c.done_column = "finished".into();
        c.enabled = false;
        assert!(!sweep(&mut board, &mut archive, &c, now()).changed());
        c.enabled = true;
        assert_eq!(sweep(&mut board, &mut archive, &c, now()).archived, 1);
    }

    #[test]
    fn deletion_is_opt_in() {
        let mut board = json!({ "tasks": [] });
        let mut archive = vec![
            json!({ "id": "ancient", "archivedAt": "2020-01-01T00:00:00Z" }),
            json!({ "id": "recent", "archivedAt": "2026-02-28T00:00:00Z" }),
            json!({ "id": "undated" }),
        ];
        assert!(!sweep(&mut board, &mut archive, &cfg(), now()).changed());
        let mut c = cfg();
        c.delete_after_days = 30;
        assert_eq!(sweep(&mut board, &mut archive, &c, now()).deleted, 1);
        assert_eq!(ids(&archive), ["recent", "undated"]);
    }

    #[test]
    fn huge_day_counts_archive_and_delete_nothing() {
        let mut board = json!({ "tasks": [
            { "id": "a", "status": "done", "updatedAt": "2020-01-01T00:00:00Z" }
        ] });
        let mut archive = vec![json!({ "id": "b", "archivedAt": "2020-01-01T00:00:00Z" })];
        let mut c = cfg();
        for days in [4_000_000_000, u32::MAX] {
            c.after_days = days;
            c.delete_after_days = days;
            assert!(!sweep(&mut board, &mut archive, &c, now()).changed());
        }
    }

    #[test]
    fn non_object_board_is_left_alone() {
        for mut board in [json!(null), json!([1, 2]), json!({ "tasks": "nope" })] {
            let copy = board.clone();
            let mut archive = vec![json!({ "id": "x", "archivedAt": "2026-01-01T00:00:00Z" })];
            sweep(&mut board, &mut archive, &cfg(), now());
            assert_eq!(
                archive_tasks(&mut board, &mut archive, &["x".into()], now()),
                0
            );
            assert_eq!(board, copy);
        }
    }

    #[test]
    fn tombstones_archived_ids() {
        let mut board = json!({ "tasks": [{ "id": "a" }, { "id": "b" }, { "title": "no id" }] });
        let archive = vec![json!({ "id": "a" })];
        assert_eq!(drop_archived(&mut board, &archive), 1);
        assert_eq!(board["tasks"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn archive_then_restore_round_trips() {
        let mut board = json!({ "tasks": [
            { "id": "a", "status": "done", "contextId": "work", "updatedAt": "2020-01-01T00:00:00Z" },
            { "id": "b", "status": "todo" }
        ] });
        let mut archive = vec![];
        let ids_ab: Vec<String> = vec!["a".into(), "zzz".into()];
        assert_eq!(archive_tasks(&mut board, &mut archive, &ids_ab, now()), 1);
        assert_eq!(ids(board["tasks"].as_array().unwrap()), ["b"]);

        let later = now() + Duration::hours(1);
        assert_eq!(restore_tasks(&mut board, &mut archive, &ids_ab, later), 1);
        assert!(archive.is_empty());
        let a = &board["tasks"][1];
        assert_eq!(a["status"], "done");
        assert_eq!(a["contextId"], "work");
        assert_eq!(a["updatedAt"], "2026-03-01T13:00:00.000Z");
        assert!(a.get("archivedAt").is_none());
        // Freshly restored, so the sweep leaves it on the board.
        assert!(!sweep(&mut board, &mut archive, &cfg(), later).changed());
    }

    #[test]
    fn restore_creates_tasks_array_and_replaces_duplicates() {
        let mut board = json!({});
        let mut archive = vec![json!({ "id": "a", "archivedAt": "2026-01-01T00:00:00Z" })];
        assert_eq!(
            restore_tasks(&mut board, &mut archive, &["a".into()], now()),
            1
        );
        assert_eq!(ids(board["tasks"].as_array().unwrap()), ["a"]);

        let mut board = json!({ "tasks": [{ "id": "a", "title": "stale" }] });
        let mut archive = vec![json!({ "id": "a", "title": "archived" })];
        restore_tasks(&mut board, &mut archive, &["a".into()], now());
        assert_eq!(board["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(board["tasks"][0]["title"], "archived");
    }

    #[test]
    fn sorts_newest_first_with_undated_last() {
        let mut archive = vec![
            json!({ "id": "undated" }),
            json!({ "id": "old", "archivedAt": "2026-01-01T00:00:00Z" }),
            json!({ "id": "new", "archivedAt": "2026-02-01T00:00:00+01:00" }),
        ];
        sort_newest_first(&mut archive);
        assert_eq!(ids(&archive), ["new", "old", "undated"]);
    }
}
