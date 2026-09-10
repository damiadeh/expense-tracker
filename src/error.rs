//! The one error type the whole application returns.
//!
//! # Why not `Box<dyn Error>`?
//!
//! `Box<dyn Error>` is easy to write and impossible to inspect. Once an error
//! is boxed, all a caller can do is print it — there is no way to ask "was
//! this a missing row, or a corrupt database?" without downcasting.
//!
//! An enum keeps that question answerable. `commands::delete` can match on
//! [`AppError::NotFound`] and print a friendly message, while a genuine
//! database failure propagates. The `match` is also exhaustive, so adding a
//! variant later makes the compiler point at every place that needs updating.
//!
//! The rule of thumb: **libraries return enums, applications box them.** This
//! crate is both, so the library half (everything in `lib.rs`) uses `AppError`,
//! and `main.rs` is free to be sloppier at the very edge.

use std::path::PathBuf;

use crate::money::ParseMoneyError;

/// Anything that can go wrong in the expense tracker.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// SQLite failed. Wraps the underlying `rusqlite` error unchanged.
    ///
    /// `#[from]` generates `impl From<rusqlite::Error> for AppError`, and that
    /// `From` impl is precisely what the `?` operator calls. It is the reason
    /// a function returning `Result<T>` can write `conn.execute(...)?` even
    /// though `execute` returns a *different* error type.
    #[error("database error")]
    Database(#[from] rusqlite::Error),

    /// The user typed an amount we could not parse.
    ///
    /// `#[error(transparent)]` means "delegate `Display` entirely to the inner
    /// error" — no prefix of our own. `ParseMoneyError` already says
    /// "`banana` is not a valid amount"; wrapping that in "invalid amount: …"
    /// would just stutter.
    #[error(transparent)]
    Amount(#[from] ParseMoneyError),

    /// The category was empty, or contained whitespace.
    #[error("invalid category `{0}`: {1}")]
    InvalidCategory(String, &'static str),

    /// The date was not in `YYYY-MM-DD` form.
    #[error("invalid date `{0}`: expected YYYY-MM-DD")]
    InvalidDate(String),

    /// The month filter was not in `YYYY-MM` form.
    #[error("invalid month `{0}`: expected YYYY-MM")]
    InvalidMonth(String),

    /// No expense exists with the requested id.
    #[error("no expense with id {0}")]
    NotFound(i64),

    /// Reading or writing a file failed.
    #[error("i/o error")]
    Io(#[from] std::io::Error),

    /// We could not work out where to put the database.
    #[error("could not determine a data directory; set ET_DB_PATH to choose one")]
    NoDataDirectory,

    /// The data directory exists but could not be created or opened.
    ///
    /// `#[source]` marks the underlying cause without generating a `From` impl.
    /// You want that here: two variants both wrap `std::io::Error`, and only
    /// one of them can own the automatic conversion.
    #[error("could not open database at {path}")]
    DatabasePath {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Shorthand so every signature in this crate can say `Result<T>`.
///
/// This shadows `std::result::Result` inside the crate, which is why the
/// definition has to spell out the full path on the right-hand side.
pub type Result<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn question_mark_converts_parse_errors() {
        // This function returns `AppError`, but `parse` returns
        // `ParseMoneyError`. The `?` finds the `From` impl that `#[from]`
        // generated and applies it silently.
        fn parse_amount(input: &str) -> Result<crate::Money> {
            let money: crate::Money = input.parse()?;
            Ok(money)
        }

        assert!(parse_amount("42.50").is_ok());
        assert!(matches!(
            parse_amount("banana"),
            Err(AppError::Amount(ParseMoneyError::InvalidCharacter(_)))
        ));
    }

    #[test]
    fn question_mark_converts_sqlite_errors() {
        fn always_fails() -> Result<()> {
            Err(rusqlite::Error::QueryReturnedNoRows)?
        }

        assert!(matches!(always_fails(), Err(AppError::Database(_))));
    }

    #[test]
    fn callers_can_match_on_the_variant() {
        // The whole point of an enum over `Box<dyn Error>`: a caller can ask
        // what kind of failure this was and react differently.
        let err = AppError::NotFound(42);
        let message = match err {
            AppError::NotFound(id) => format!("nothing to delete at {id}"),
            _ => "something else".to_string(),
        };
        assert_eq!(message, "nothing to delete at 42");
    }

    #[test]
    fn display_messages_are_user_facing() {
        assert_eq!(AppError::NotFound(7).to_string(), "no expense with id 7");
        assert_eq!(
            AppError::InvalidDate("2026-13-01".into()).to_string(),
            "invalid date `2026-13-01`: expected YYYY-MM-DD"
        );
        // `transparent` passes the inner message straight through.
        assert_eq!(
            AppError::Amount(ParseMoneyError::Empty).to_string(),
            "amount is empty"
        );
    }

    #[test]
    fn wrapped_errors_keep_their_cause() {
        // `source()` is how a caller walks down to the original failure.
        // `thiserror` wires it up from `#[from]` and `#[source]`.
        let err = AppError::from(rusqlite::Error::QueryReturnedNoRows);
        assert_eq!(err.to_string(), "database error");

        let cause = err.source().expect("should have a source");
        assert_eq!(cause.to_string(), "Query returned no rows");
    }

    #[test]
    fn source_chain_can_be_walked() {
        let err = AppError::DatabasePath {
            path: PathBuf::from("/nope/expenses.db"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        };

        let mut chain = Vec::new();
        let mut current: Option<&dyn Error> = Some(&err);
        while let Some(e) = current {
            chain.push(e.to_string());
            current = e.source();
        }

        assert_eq!(
            chain,
            vec!["could not open database at /nope/expenses.db", "denied"]
        );
    }
}
