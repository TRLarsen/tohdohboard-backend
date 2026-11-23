//! On-disk storage of boards: `data.json` and `archive.json` in one directory.
//!
//! Writes are atomic (temporary file, `fsync`, rename), so a crash never leaves
//! a half-written file behind. All access to one board directory is serialized
//! by a per-board lock ([`Board::with_files`]), so read-modify-write sequences
//! such as the archive sweep cannot interleave.

use std::collections::HashMap;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde_json::{Value, json};

/// File name of the board document.
pub const DATA_FILE: &str = "data.json";
/// File name of the archive.
pub const ARCHIVE_FILE: &str = "archive.json";

/// The board served when no `data.json` exists yet: no tasks, the contexts
/// `work`, `school` and `projects`, and the columns `todo`, `doing` and `done`.
pub fn default_board() -> Value {
    json!({
        "tasks": [],
        "contexts": [
            { "id": "work", "name": "Work", "position": 0 },
            { "id": "school", "name": "School", "position": 1 },
            { "id": "projects", "name": "Projects", "position": 2 }
        ],
        "columns": [
            { "id": "todo", "name": "To Do", "position": 0 },
            { "id": "doing", "name": "Doing", "position": 1 },
            { "id": "done", "name": "Done", "position": 2 }
        ]
    })
}

/// Hands out [`Board`] handles, one lock per board directory.
#[derive(Debug, Default)]
pub struct Boards {
    locks: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
}

impl Boards {
    /// Returns the handle for the board stored in `dir`.
    pub fn board(&self, dir: PathBuf) -> Board {
        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        let lock = locks.entry(dir.clone()).or_default().clone();
        Board { dir, lock }
    }
}

/// A handle to one board directory.
#[derive(Debug, Clone)]
pub struct Board {
    dir: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl Board {
    /// Runs `f` with exclusive access to the board's files.
    ///
    /// The closure runs on the blocking thread pool, so it may do plain
    /// synchronous file I/O.
    ///
    /// # Errors
    ///
    /// Returns whatever `f` returns, or an error if the task panicked.
    pub async fn with_files<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&BoardFiles) -> Result<T> + Send + 'static,
    {
        let board = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = board.lock.lock().unwrap_or_else(|e| e.into_inner());
            f(&BoardFiles { dir: &board.dir })
        })
        .await
        .context("storage task failed")?
    }
}

/// The files of one board, available while its lock is held.
#[derive(Debug)]
pub struct BoardFiles<'a> {
    dir: &'a Path,
}

impl BoardFiles<'_> {
    /// Reads `data.json`, or the [`default_board`] if it does not exist (or is
    /// empty).
    ///
    /// # Errors
    ///
    /// Fails if the file exists but cannot be read or is not valid JSON. This
    /// is deliberate: answering with the default board would invite the client
    /// to overwrite the real (if damaged) data with it.
    pub fn read_board(&self) -> Result<Value> {
        Ok(read_json(&self.dir.join(DATA_FILE))?.unwrap_or_else(default_board))
    }

    /// Atomically replaces `data.json`.
    ///
    /// # Errors
    ///
    /// Fails if the directory cannot be created or the file cannot be written.
    pub fn write_board(&self, board: &Value) -> Result<()> {
        write_json(self.dir, DATA_FILE, board)
    }

    /// Reads the archived tasks from `archive.json` (`{ "tasks": [...] }`);
    /// no file means an empty archive.
    ///
    /// # Errors
    ///
    /// Fails if the file exists but cannot be read or does not have that shape.
    pub fn read_archive(&self) -> Result<Vec<Value>> {
        let path = self.dir.join(ARCHIVE_FILE);
        match read_json(&path)? {
            None => Ok(Vec::new()),
            Some(Value::Object(mut obj)) => match obj.remove("tasks") {
                Some(Value::Array(tasks)) => Ok(tasks),
                _ => anyhow::bail!("{}: expected a \"tasks\" array", path.display()),
            },
            Some(_) => anyhow::bail!("{}: expected an object", path.display()),
        }
    }

    /// Atomically replaces `archive.json`.
    ///
    /// # Errors
    ///
    /// Fails if the directory cannot be created or the file cannot be written.
    pub fn write_archive(&self, tasks: &[Value]) -> Result<()> {
        write_json(self.dir, ARCHIVE_FILE, &json!({ "tasks": tasks }))
    }
}

/// Reads and parses a JSON file. `None` if it is missing or blank.
fn read_json(path: &Path) -> Result<Option<Value>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&text)
        .map(Some)
        .with_context(|| format!("{} is not valid JSON", path.display()))
}

/// Writes pretty-printed JSON to `dir/name` via a temporary file and rename.
fn write_json(dir: &Path, name: &str, value: &Value) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join(name);
    let tmp = dir.join(format!(".{name}.tmp"));
    let result = (|| -> std::io::Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&tmp, &path)?;
        // Persist the rename itself; best effort, not supported everywhere.
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_blank_file_is_default_board() {
        let dir = tempfile::tempdir().unwrap();
        let files = BoardFiles { dir: dir.path() };
        assert_eq!(files.read_board().unwrap(), default_board());
        fs::write(dir.path().join(DATA_FILE), "  \n").unwrap();
        assert_eq!(files.read_board().unwrap(), default_board());
        assert!(files.read_archive().unwrap().is_empty());
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let files = BoardFiles { dir: dir.path() };
        fs::write(dir.path().join(DATA_FILE), "{ \"tasks\": [").unwrap();
        assert!(files.read_board().is_err());
        fs::write(dir.path().join(ARCHIVE_FILE), "[]").unwrap();
        assert!(files.read_archive().is_err());
    }

    #[test]
    fn writes_round_trip_and_leave_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        let files = BoardFiles { dir: &nested };
        let board = json!({ "tasks": [{ "id": "x", "custom": { "deep": [1, 2] } }], "future": 1 });
        files.write_board(&board).unwrap();
        files.write_archive(&[json!({ "id": "y" })]).unwrap();
        assert_eq!(files.read_board().unwrap(), board);
        assert_eq!(files.read_archive().unwrap(), vec![json!({ "id": "y" })]);
        let mut names: Vec<_> = fs::read_dir(&nested)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, [ARCHIVE_FILE, DATA_FILE]);
    }

    #[test]
    fn same_dir_shares_a_lock() {
        let boards = Boards::default();
        let a = boards.board("/x".into());
        let b = boards.board("/x".into());
        let c = boards.board("/y".into());
        assert!(Arc::ptr_eq(&a.lock, &b.lock));
        assert!(!Arc::ptr_eq(&a.lock, &c.lock));
    }
}
