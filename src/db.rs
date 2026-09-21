//! SQLite storage: opening a connection, and keeping the schema current.
//!
//! Queries live in the next stage. This module is only concerned with getting
//! a usable, correctly-shaped database in front of the rest of the program.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use rusqlite::{Connection, Row, ToSql, params, params_from_iter};

use crate::error::{AppError, Result};
use crate::models::{Expense, Month, NewExpense};
use crate::money::Money;

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
        --
        -- One consequence: ids are *reused* after a delete. Adding
        -- AUTOINCREMENT would stop that, at the cost of ever-growing numbers
        -- in a tool whose ids exist to be typed by hand. The `delete` command
        -- prints what it is about to remove and asks for confirmation, which
        -- is the real protection against acting on a stale id.
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

    // `month()` is a `Datelike` trait method, not an inherent one, so the trait
    // has to be in scope to call it. This is why `use`-ing a trait you never
    // name directly is a normal thing to do in Rust.
    use chrono::Datelike;

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

    // --- query fixtures ----------------------------------------------------

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("test date must be valid")
    }

    fn new_expense(cents: i64, category: &str, note: Option<&str>, d: NaiveDate) -> NewExpense {
        NewExpense::new(Money::from_cents(cents), category, note, d).unwrap()
    }

    /// A database with a known set of expenses across two months.
    fn seeded() -> Connection {
        let conn = open_in_memory().unwrap();
        let rows = [
            (4250, "groceries", Some("Trader Joe's"), date(2026, 9, 10)),
            (8814, "groceries", Some("Costco"), date(2026, 9, 3)),
            (5200, "transport", None, date(2026, 9, 7)),
            (3070, "coffee", None, date(2026, 9, 1)),
            (1000, "coffee", None, date(2026, 8, 28)),
        ];
        for (cents, category, note, d) in rows {
            insert(&conn, &new_expense(cents, category, note, d)).unwrap();
        }
        conn
    }

    // --- insert ------------------------------------------------------------

    #[test]
    fn insert_returns_increasing_ids() {
        let conn = open_in_memory().unwrap();
        let first = insert(&conn, &new_expense(100, "coffee", None, date(2026, 9, 1))).unwrap();
        let second = insert(&conn, &new_expense(200, "coffee", None, date(2026, 9, 2))).unwrap();
        assert_eq!(first, 1);
        assert_eq!(second, 2);
    }

    #[test]
    fn insert_then_get_round_trips_every_field() {
        let conn = open_in_memory().unwrap();
        let original = new_expense(4250, "groceries", Some("Trader Joe's"), date(2026, 9, 10));
        let id = insert(&conn, &original).unwrap();

        let fetched = get(&conn, id).unwrap();
        assert_eq!(fetched, original.clone().saved_as(id));
        // Spelled out, so a failure says which field drifted.
        assert_eq!(fetched.amount, Money::from_cents(4250));
        assert_eq!(fetched.category, "groceries");
        assert_eq!(fetched.note.as_deref(), Some("Trader Joe's"));
        assert_eq!(fetched.date, date(2026, 9, 10));
    }

    #[test]
    fn a_missing_note_round_trips_as_none() {
        // SQL NULL out, `None` back in, with no special handling anywhere.
        let conn = open_in_memory().unwrap();
        let id = insert(&conn, &new_expense(100, "coffee", None, date(2026, 9, 1))).unwrap();
        assert_eq!(get(&conn, id).unwrap().note, None);
    }

    #[test]
    fn negative_amounts_survive_the_round_trip() {
        // Refunds. The column is a signed INTEGER, so this just works.
        let conn = open_in_memory().unwrap();
        let id = insert(
            &conn,
            &new_expense(-1500, "groceries", None, date(2026, 9, 1)),
        )
        .unwrap();
        assert_eq!(get(&conn, id).unwrap().amount, Money::from_cents(-1500));
    }

    #[test]
    fn quotes_in_a_note_are_stored_literally() {
        // The apostrophe would end the string early if values were being
        // interpolated into the SQL. Bound parameters make it a non-event.
        let conn = open_in_memory().unwrap();
        let note = "Bob's \"cafe\"; DROP TABLE expenses; --";
        let id = insert(
            &conn,
            &new_expense(100, "coffee", Some(note), date(2026, 9, 1)),
        )
        .unwrap();

        assert_eq!(get(&conn, id).unwrap().note.as_deref(), Some(note));
        // The table is still there.
        assert_eq!(list(&conn, &ListFilter::default()).unwrap().len(), 1);
    }

    // --- get ---------------------------------------------------------------

    #[test]
    fn get_reports_not_found_rather_than_a_driver_error() {
        let conn = open_in_memory().unwrap();
        assert!(matches!(get(&conn, 99), Err(AppError::NotFound(99))));
    }

    // --- list --------------------------------------------------------------

    #[test]
    fn list_returns_everything_by_default() {
        let conn = seeded();
        assert_eq!(list(&conn, &ListFilter::default()).unwrap().len(), 5);
    }

    #[test]
    fn list_is_newest_first() {
        let conn = seeded();
        let dates: Vec<NaiveDate> = list(&conn, &ListFilter::default())
            .unwrap()
            .iter()
            .map(|e| e.date)
            .collect();

        let mut sorted = dates.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(dates, sorted);
    }

    #[test]
    fn list_filters_by_month() {
        let conn = seeded();
        let filter = ListFilter {
            month: Some("2026-09".parse().unwrap()),
            ..Default::default()
        };
        let found = list(&conn, &filter).unwrap();

        assert_eq!(found.len(), 4);
        assert!(found.iter().all(|e| e.date.month() == 9));
    }

    #[test]
    fn list_filters_by_category() {
        let conn = seeded();
        let filter = ListFilter {
            category: Some("groceries".to_string()),
            ..Default::default()
        };
        let found = list(&conn, &filter).unwrap();

        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|e| e.category == "groceries"));
    }

    #[test]
    fn list_combines_filters_with_and() {
        let conn = seeded();
        let filter = ListFilter {
            month: Some("2026-09".parse().unwrap()),
            category: Some("coffee".to_string()),
            limit: None,
        };
        let found = list(&conn, &filter).unwrap();

        // The August coffee is excluded by the month, the September groceries
        // by the category.
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].amount, Money::from_cents(3070));
    }

    #[test]
    fn list_respects_the_limit() {
        let conn = seeded();
        let filter = ListFilter {
            limit: Some(2),
            ..Default::default()
        };
        assert_eq!(list(&conn, &filter).unwrap().len(), 2);
    }

    #[test]
    fn list_returns_an_empty_vec_not_an_error() {
        // "Nothing matched" is an answer, not a failure. The command layer
        // decides how to phrase it.
        let conn = seeded();
        let filter = ListFilter {
            month: Some("1999-01".parse().unwrap()),
            ..Default::default()
        };
        assert_eq!(list(&conn, &filter).unwrap(), vec![]);
    }

    #[test]
    fn month_filter_does_not_leak_into_neighbouring_months() {
        // The LIKE prefix is the only thing keeping 2026-09 from matching
        // 2026-09-xx *and* nothing else. Worth pinning explicitly.
        let conn = open_in_memory().unwrap();
        for d in [
            date(2026, 8, 31),
            date(2026, 9, 1),
            date(2026, 9, 30),
            date(2026, 10, 1),
        ] {
            insert(&conn, &new_expense(100, "misc", None, d)).unwrap();
        }

        let filter = ListFilter {
            month: Some("2026-09".parse().unwrap()),
            ..Default::default()
        };
        let found = list(&conn, &filter).unwrap();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn list_ordering_is_stable_for_same_day_expenses() {
        let conn = open_in_memory().unwrap();
        let same_day = date(2026, 9, 10);
        for cents in [100, 200, 300] {
            insert(&conn, &new_expense(cents, "coffee", None, same_day)).unwrap();
        }

        let ids: Vec<i64> = list(&conn, &ListFilter::default())
            .unwrap()
            .iter()
            .map(|e| e.id)
            .collect();
        // `id DESC` is the tiebreaker, so this is deterministic run to run.
        assert_eq!(ids, [3, 2, 1]);
    }

    // --- summary -----------------------------------------------------------

    #[test]
    fn summary_totals_each_category() {
        let conn = seeded();
        let totals = summary(&conn, Some("2026-09".parse().unwrap())).unwrap();

        assert_eq!(
            totals,
            vec![
                CategoryTotal {
                    category: "groceries".to_string(),
                    total: Money::from_cents(13_064),
                },
                CategoryTotal {
                    category: "transport".to_string(),
                    total: Money::from_cents(5200),
                },
                CategoryTotal {
                    category: "coffee".to_string(),
                    total: Money::from_cents(3070),
                },
            ]
        );
    }

    #[test]
    fn summary_is_largest_first() {
        let conn = seeded();
        let totals = summary(&conn, None).unwrap();
        let amounts: Vec<Money> = totals.iter().map(|t| t.total).collect();

        let mut sorted = amounts.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(amounts, sorted);
    }

    #[test]
    fn summary_without_a_month_covers_everything() {
        let conn = seeded();
        let totals = summary(&conn, None).unwrap();

        let coffee = totals.iter().find(|t| t.category == "coffee").unwrap();
        // September's $30.70 plus August's $10.00.
        assert_eq!(coffee.total, Money::from_cents(4070));
    }

    #[test]
    fn summary_total_matches_the_sum_of_the_listed_rows() {
        // Cross-check: the aggregate SQL and adding the rows up in Rust must
        // agree. If they ever diverge, one of them is wrong.
        let conn = seeded();
        let month: Month = "2026-09".parse().unwrap();

        let from_summary: Money = summary(&conn, Some(month))
            .unwrap()
            .iter()
            .map(|t| t.total)
            .sum();

        let filter = ListFilter {
            month: Some(month),
            ..Default::default()
        };
        let from_rows: Money = list(&conn, &filter).unwrap().iter().map(|e| e.amount).sum();

        assert_eq!(from_summary, from_rows);
        assert_eq!(from_summary, Money::from_cents(21_334));
    }

    #[test]
    fn summary_of_an_empty_database_is_empty() {
        let conn = open_in_memory().unwrap();
        assert_eq!(summary(&conn, None).unwrap(), vec![]);
    }

    // --- delete ------------------------------------------------------------

    #[test]
    fn delete_removes_the_row() {
        let conn = seeded();
        delete(&conn, 1).unwrap();

        assert!(matches!(get(&conn, 1), Err(AppError::NotFound(1))));
        assert_eq!(list(&conn, &ListFilter::default()).unwrap().len(), 4);
    }

    #[test]
    fn deleting_a_missing_id_is_an_error_not_a_silent_no_op() {
        // SQLite is perfectly happy to delete zero rows and call it success.
        // The affected-row count is the only signal that anything went wrong.
        let conn = seeded();
        assert!(matches!(delete(&conn, 99), Err(AppError::NotFound(99))));
    }

    #[test]
    fn ids_are_reused_after_a_delete() {
        // Deliberate, and a consequence of INTEGER PRIMARY KEY without
        // AUTOINCREMENT. Pinned here so that changing it is a decision rather
        // than an accident: an id noted from an earlier `list` can refer to a
        // different expense later, which is why `delete` confirms by showing
        // the row rather than trusting the number.
        let conn = open_in_memory().unwrap();
        let first = insert(&conn, &new_expense(100, "tea", None, date(2026, 9, 1))).unwrap();
        delete(&conn, first).unwrap();

        let second = insert(&conn, &new_expense(200, "tea", None, date(2026, 9, 2))).unwrap();
        assert_eq!(second, first, "the id should be handed out again");
        assert_eq!(get(&conn, second).unwrap().amount, Money::from_cents(200));
    }

    #[test]
    fn deleting_twice_fails_the_second_time() {
        let conn = seeded();
        assert!(delete(&conn, 1).is_ok());
        assert!(matches!(delete(&conn, 1), Err(AppError::NotFound(1))));
    }

    // --- the mapping seam --------------------------------------------------

    #[test]
    fn a_corrupt_date_surfaces_as_an_error_not_a_panic() {
        // Someone edited the file with the sqlite3 CLI. `row_to_expense` has
        // to cope, and the `?` in `collect()` is what propagates it.
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO expenses (amount_cents, category, note, date)
             VALUES (100, 'coffee', NULL, 'not-a-date')",
            [],
        )
        .unwrap();

        let result = list(&conn, &ListFilter::default());
        assert!(matches!(result, Err(AppError::Database(_))));
    }
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// The column list every `SELECT` in this module uses, in this exact order.
///
/// [`row_to_expense`] reads by positional index, so the order here and the
/// indices there are a single unit — change one and you must change the other.
/// Naming the list once is what keeps them from drifting apart.
const EXPENSE_COLUMNS: &str = "id, amount_cents, category, note, date";

/// Which expenses to return. Every field is optional; all-`None` means "all".
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub month: Option<Month>,
    pub category: Option<String>,
    pub limit: Option<u32>,
}

/// One row of a summary: a category and what was spent on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryTotal {
    pub category: String,
    pub total: Money,
}

/// Turns a database row into an [`Expense`].
///
/// This is the seam where storage types become domain types: an `i64` of cents
/// becomes a `Money`, a `TEXT` date becomes a `NaiveDate`.
///
/// It returns `rusqlite::Result`, not our `Result`, because `query_map` demands
/// that type — so a date that will not parse has to be reported as a *SQLite*
/// error. `FromSqlConversionFailure` is the right one: it means "the value in
/// this column is not what this column is supposed to hold", which is exactly
/// what a corrupt date is.
fn row_to_expense(row: &Row<'_>) -> rusqlite::Result<Expense> {
    // These indices match EXPENSE_COLUMNS above.
    const DATE_COL: usize = 4;

    let date_text: String = row.get(DATE_COL)?;
    let date = NaiveDate::parse_from_str(&date_text, "%Y-%m-%d").map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            DATE_COL,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })?;

    Ok(Expense {
        id: row.get(0)?,
        amount: Money::from_cents(row.get(1)?),
        category: row.get(2)?,
        // `Option<String>` maps straight onto a nullable column — rusqlite
        // turns SQL NULL into None with no work from us.
        note: row.get(3)?,
        date,
    })
}

/// Stores an expense and returns the id the database assigned it.
pub fn insert(conn: &Connection, expense: &NewExpense) -> Result<i64> {
    conn.execute(
        "INSERT INTO expenses (amount_cents, category, note, date)
         VALUES (?1, ?2, ?3, ?4)",
        // `params!` bundles values of different types into something the driver
        // can bind. The values never touch the SQL string, which is what makes
        // SQL injection structurally impossible here.
        params![
            expense.amount.cents(),
            expense.category,
            expense.note,
            expense.date.to_string(),
        ],
    )?;

    Ok(conn.last_insert_rowid())
}

/// Fetches one expense by id.
pub fn get(conn: &Connection, id: i64) -> Result<Expense> {
    let sql = format!("SELECT {EXPENSE_COLUMNS} FROM expenses WHERE id = ?1");

    conn.query_row(&sql, [id], row_to_expense)
        // "No rows" is not really an error at this layer — it is an answer.
        // Translating it into our own NotFound lets `main` print something
        // useful instead of leaking a driver error at the user.
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => AppError::NotFound(id),
            other => AppError::Database(other),
        })
}

/// Fetches expenses matching a filter, newest first.
pub fn list(conn: &Connection, filter: &ListFilter) -> Result<Vec<Expense>> {
    let mut sql = format!("SELECT {EXPENSE_COLUMNS} FROM expenses");
    let mut conditions: Vec<String> = Vec::new();
    // A trait object, because the values bound here are of different types —
    // a String for the date prefix, an i64 for the limit.
    let mut values: Vec<Box<dyn ToSql>> = Vec::new();

    // Note what is being built dynamically and what is not: the *shape* of the
    // query grows, but every user-supplied value goes in as a bound parameter.
    // Interpolating a value into this string would be the injection bug.
    if let Some(month) = filter.month {
        values.push(Box::new(month.like_prefix()));
        conditions.push(format!("date LIKE ?{}", values.len()));
    }

    if let Some(category) = &filter.category {
        values.push(Box::new(category.clone()));
        conditions.push(format!("category = ?{}", values.len()));
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }

    // Newest first, and `id DESC` breaks ties so that two expenses added on the
    // same day come back in a stable order rather than whatever SQLite feels
    // like. Without it, output could change between runs.
    sql.push_str(" ORDER BY date DESC, id DESC");

    if let Some(limit) = filter.limit {
        values.push(Box::new(i64::from(limit)));
        sql.push_str(&format!(" LIMIT ?{}", values.len()));
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values.iter()), row_to_expense)?;

    // `rows` is an iterator of `Result<Expense>`. Collecting into
    // `Result<Vec<Expense>>` turns it inside out: the first error short-circuits
    // and is returned, otherwise you get every row. This works because `Result`
    // implements `FromIterator` — one of the most useful impls in the standard
    // library, and easy to miss.
    let expenses: rusqlite::Result<Vec<Expense>> = rows.collect();
    Ok(expenses?)
}

/// Totals spending per category, largest first.
pub fn summary(conn: &Connection, month: Option<Month>) -> Result<Vec<CategoryTotal>> {
    let mut sql = String::from("SELECT category, SUM(amount_cents) FROM expenses");
    let mut values: Vec<Box<dyn ToSql>> = Vec::new();

    if let Some(month) = month {
        values.push(Box::new(month.like_prefix()));
        sql.push_str(" WHERE date LIKE ?1");
    }

    // Summing in SQL rather than in Rust: the database can do it while reading
    // the rows, and only one row per category crosses the boundary.
    sql.push_str(" GROUP BY category ORDER BY SUM(amount_cents) DESC, category ASC");

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values.iter()), |row| {
        Ok(CategoryTotal {
            category: row.get(0)?,
            total: Money::from_cents(row.get(1)?),
        })
    })?;

    let totals: rusqlite::Result<Vec<CategoryTotal>> = rows.collect();
    Ok(totals?)
}

/// Deletes an expense, reporting [`AppError::NotFound`] if there was none.
pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    // `execute` returns how many rows it changed. Zero means the id was wrong —
    // SQLite considers deleting nothing a perfectly successful DELETE, so this
    // check is the only thing standing between the user and a silent no-op.
    let affected = conn.execute("DELETE FROM expenses WHERE id = ?1", [id])?;

    if affected == 0 {
        return Err(AppError::NotFound(id));
    }

    Ok(())
}
