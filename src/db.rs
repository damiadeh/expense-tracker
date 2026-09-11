//! SQLite storage: opening a connection, and keeping the schema current.
//!
//! Queries live in the next stage. This module is only concerned with getting
//! a usable, correctly-shaped database in front of the rest of the program.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::error::{AppError, Result};

/// Environment variable that overrides where the database lives.
///
/// Mostly for tests — each one points at its own temporary file so they cannot
/// interfere with each other, or with your real expenses.
pub const DB_PATH_ENV: &str = "ET_DB_PATH";

/// Filename used inside the platform data directory.
const DB_FILE_NAME: &str = "expenses.db";

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Schema migrations, in order. Index + 1 is the schema version each produces.
///
/// The list is append-only: once a migration has shipped it must never be
/// edited, because databases in the wild have already run it. To change the
/// schema you add a new entry.
const MIGRATIONS: &[&str] = &[
    // v1 — the initial schema.
    r"
    CREATE TABLE expenses (
        -- `INTEGER PRIMARY KEY` is special in SQLite: it aliases the internal
        -- rowid, so it auto-assigns and needs no AUTOINCREMENT keyword.
        id           INTEGER PRIMARY KEY,

        -- Cents, never a float. `REAL` here would reintroduce exactly the
        -- rounding problem the Money newtype exists to prevent.
        amount_cents INTEGER NOT NULL,

        category     TEXT    NOT NULL,

        -- Nullable, and the only nullable column: it maps to Option<String>.
        note         TEXT,

        -- Zero-padded ISO text, so string comparison is date comparison.
        date         TEXT    NOT NULL
    );

    -- `list` and `summary` both filter on date, and `list` orders by it.
    CREATE INDEX idx_expenses_date ON expenses (date);

    -- `summary` groups by category.
    CREATE INDEX idx_expenses_category ON expenses (category);
    ",
];

/// The schema version this build of the program expects.
fn target_version() -> i64 {
    MIGRATIONS.len() as i64
}

/// Reads the version stamped into the database file itself.
///
/// `user_version` is a 4-byte slot SQLite sets aside for exactly this purpose.
/// A brand-new database reports 0.
fn schema_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

/// Brings the database up to the current schema version.
///
/// Idempotent: running it against an already-current database does nothing.
/// That matters because this runs on *every* invocation of the program — there
/// is no separate "migrate" command to remember.
pub fn migrate(conn: &Connection) -> Result<()> {
    let current = schema_version(conn)?;

    // The overwhelmingly common case: already current, so do no work at all.
    // Without this, every run of the program would loop over every migration
    // just to skip them all.
    if current >= target_version() {
        return Ok(());
    }

    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= current {
            continue;
        }

        // Each migration and its version stamp go in one transaction, so an
        // interrupted upgrade cannot leave the file half-migrated.
        //
        // `PRAGMA user_version` is the rare case where building SQL with
        // `format!` is correct: pragmas cannot take bound parameters, and this
        // value is an array index, never user input.
        conn.execute_batch(&format!(
            "BEGIN;
             {sql}
             PRAGMA user_version = {version};
             COMMIT;"
        ))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------

/// Opens the database at `path`, creating and migrating it if needed.
pub fn open(path: &Path) -> Result<Connection> {
    // The data directory may not exist on a first run.
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| AppError::DatabasePath {
            path: path.to_path_buf(),
            source,
        })?;
    }

    let conn = Connection::open(path)?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Opens a throwaway database that exists only in RAM.
///
/// Used by tests: it is fast, needs no cleanup, and is invisible to every other
/// test running at the same time.
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Connection settings applied before anything else touches the database.
fn configure(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        -- Off by default in SQLite, for backwards compatibility. Turn it on
        -- now so that foreign keys added later are actually enforced.
        PRAGMA foreign_keys = ON;

        -- If another process holds the write lock, wait up to 5s rather than
        -- failing instantly.
        PRAGMA busy_timeout = 5000;
        ",
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Locating the database file
// ---------------------------------------------------------------------------

/// Where the database lives when the user has not said otherwise.
///
/// On macOS that is `~/Library/Application Support/expense-tracker/expenses.db`;
/// on Linux it follows the XDG spec. The `directories` crate knows the
/// conventions so we do not hardcode any of them.
pub fn default_db_path() -> Result<PathBuf> {
    resolve_db_path(std::env::var_os(DB_PATH_ENV))
}

/// The decision logic behind [`default_db_path`], with the environment passed
/// in rather than read.
///
/// Splitting it this way is what makes it testable. Environment variables are
/// process-global, and `cargo test` runs tests in parallel threads — a test
/// that sets `ET_DB_PATH` would be visible to every other test running at that
/// moment. A pure function takes the environment as an argument instead.
fn resolve_db_path(override_path: Option<OsString>) -> Result<PathBuf> {
    if let Some(path) = override_path
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }

    let dirs = directories::ProjectDirs::from("", "", "expense-tracker")
        .ok_or(AppError::NoDataDirectory)?;

    Ok(dirs.data_dir().join(DB_FILE_NAME))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Every column in `expenses`, as SQLite reports it.
    fn columns(conn: &Connection) -> Vec<(String, String, bool)> {
        let mut stmt = conn
            .prepare("SELECT name, type, \"notnull\" FROM pragma_table_info('expenses')")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? == 1,
                ))
            })
            .unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    #[test]
    fn migration_creates_the_expenses_table() {
        let conn = open_in_memory().unwrap();
        let names: Vec<String> = columns(&conn).into_iter().map(|(n, _, _)| n).collect();
        assert_eq!(names, ["id", "amount_cents", "category", "note", "date"]);
    }

    #[test]
    fn amount_is_an_integer_column() {
        // If this ever says REAL, the float bug is back.
        let conn = open_in_memory().unwrap();
        let amount = columns(&conn)
            .into_iter()
            .find(|(name, _, _)| name == "amount_cents")
            .expect("amount_cents column");
        assert_eq!(amount.1, "INTEGER");
    }

    #[test]
    fn only_note_is_nullable() {
        let conn = open_in_memory().unwrap();
        let nullable: Vec<String> = columns(&conn)
            .into_iter()
            // `id` is the rowid alias and is never null despite what
            // pragma_table_info reports, so it is excluded here.
            .filter(|(name, _, notnull)| !notnull && name != "id")
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(nullable, ["note"]);
    }

    #[test]
    fn indexes_exist_for_the_columns_we_filter_on() {
        let conn = open_in_memory().unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL")
            .unwrap();
        let mut names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["idx_expenses_category", "idx_expenses_date"]);
    }

    #[test]
    fn migration_stamps_the_schema_version() {
        let conn = open_in_memory().unwrap();
        assert_eq!(schema_version(&conn).unwrap(), target_version());
    }

    #[test]
    fn migration_is_idempotent() {
        // This runs on every invocation of the program, so running it twice
        // must be indistinguishable from running it once.
        let conn = open_in_memory().unwrap();
        let before = schema_version(&conn).unwrap();

        migrate(&conn).unwrap();
        migrate(&conn).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), before);
        assert_eq!(columns(&conn).len(), 5);
    }

    #[test]
    fn a_fresh_database_reports_version_zero() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(schema_version(&conn).unwrap(), 0);
    }

    #[test]
    fn open_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        // Two levels that do not exist yet — a first run on a new machine.
        let path = dir.path().join("nested").join("deeper").join("expenses.db");
        assert!(!path.exists());

        let conn = open(&path).unwrap();

        assert!(path.exists());
        assert_eq!(schema_version(&conn).unwrap(), target_version());
    }

    #[test]
    fn reopening_a_database_preserves_its_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("expenses.db");

        {
            let conn = open(&path).unwrap();
            conn.execute(
                "INSERT INTO expenses (amount_cents, category, note, date)
                 VALUES (4250, 'groceries', NULL, '2026-09-10')",
                [],
            )
            .unwrap();
        } // connection dropped here, closing the file

        let conn = open(&path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM expenses", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn foreign_keys_are_enabled() {
        let conn = open_in_memory().unwrap();
        let enabled: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(enabled, 1);
    }

    #[test]
    fn env_override_wins_over_the_platform_directory() {
        let chosen = resolve_db_path(Some(OsString::from("/tmp/somewhere/et.db"))).unwrap();
        assert_eq!(chosen, PathBuf::from("/tmp/somewhere/et.db"));
    }

    #[test]
    fn empty_env_override_is_ignored() {
        // An unset variable and one set to "" should behave the same way.
        let chosen = resolve_db_path(Some(OsString::new())).unwrap();
        assert!(chosen.ends_with(DB_FILE_NAME));
        assert!(chosen.to_string_lossy().contains("expense-tracker"));
    }

    #[test]
    fn default_path_is_inside_the_platform_data_directory() {
        let path = resolve_db_path(None).unwrap();
        assert!(path.ends_with("expense-tracker/expenses.db"), "{path:?}");
        assert!(path.is_absolute());
    }
}
