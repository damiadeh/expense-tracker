//! `et delete` — remove an expense, after checking that is what was meant.

use std::io::{BufRead, Write};

use rusqlite::Connection;

use crate::db;
use crate::error::Result;
use crate::models::Expense;

/// Looks up an expense without deleting it.
///
/// Called before prompting so that a wrong id fails immediately — asking
/// "Delete #999?" about a row that does not exist wastes the user's time and
/// teaches them to answer prompts without reading them.
pub fn find(conn: &Connection, id: i64) -> Result<Expense> {
    db::get(conn, id)
}

/// Deletes the expense.
pub fn run(conn: &Connection, id: i64) -> Result<()> {
    db::delete(conn, id)
}

/// The question asked before deleting.
pub fn prompt(expense: &Expense) -> String {
    // `[y/N]` with the capital on N: the convention is that the capitalised
    // option is what a bare Enter selects. Here that is "no", because the
    // safe answer should be the easy one.
    format!("Delete {}? [y/N] ", super::describe(expense))
}

/// What is printed after a successful delete.
pub fn confirmation(expense: &Expense) -> String {
    format!("Deleted {}", super::describe(expense))
}

/// Asks a yes/no question and reads the answer.
///
/// Generic over the reader and writer rather than reaching for `stdin` and
/// `stderr` directly, which is what makes it testable: a test passes a
/// `Cursor` over canned input and a `Vec<u8>` to capture the prompt, with no
/// process and no terminal involved.
///
/// The prompt goes to the *writer* the caller chooses. `main` passes stderr,
/// so that `et delete 1 > file` still shows the question on screen and leaves
/// stdout clean for the result.
pub fn ask(prompt: &str, input: &mut impl BufRead, output: &mut impl Write) -> Result<bool> {
    write!(output, "{prompt}")?;
    // Prompts have no trailing newline, and stderr is unbuffered but stdout is
    // not — flushing explicitly means the question appears before we block
    // waiting for an answer rather than after.
    output.flush()?;

    let mut answer = String::new();
    let bytes = input.read_line(&mut answer)?;

    // Zero bytes means end of input — `et delete 1 < /dev/null`, or a closed
    // pipe. Treat it as "no": an unanswered question is not consent.
    if bytes == 0 {
        // Move off the prompt line so the next output is not glued to it.
        writeln!(output)?;
        return Ok(false);
    }

    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::add;
    use crate::error::AppError;
    use crate::models::parse_date;
    use crate::money::Money;
    use std::io::Cursor;

    fn seeded() -> Connection {
        let conn = db::open_in_memory().unwrap();
        add::run(
            &conn,
            Money::from_cents(4250),
            "groceries",
            Some("Trader Joe's"),
            Some(parse_date("2026-09-10").unwrap()),
        )
        .unwrap();
        conn
    }

    /// Runs `ask` against canned input, returning the answer and the prompt
    /// that was written.
    fn ask_with(input: &str) -> (bool, String) {
        let mut reader = Cursor::new(input.as_bytes());
        let mut written: Vec<u8> = Vec::new();
        let answer = ask("Delete it? [y/N] ", &mut reader, &mut written).unwrap();
        (answer, String::from_utf8(written).unwrap())
    }

    // --- finding -----------------------------------------------------------

    #[test]
    fn find_returns_the_expense() {
        let conn = seeded();
        assert_eq!(find(&conn, 1).unwrap().category, "groceries");
    }

    #[test]
    fn find_reports_a_missing_id_before_anything_is_deleted() {
        let conn = seeded();
        assert!(matches!(find(&conn, 99), Err(AppError::NotFound(99))));
        // The real row is untouched.
        assert!(find(&conn, 1).is_ok());
    }

    // --- deleting ----------------------------------------------------------

    #[test]
    fn run_removes_the_row() {
        let conn = seeded();
        run(&conn, 1).unwrap();
        assert!(matches!(find(&conn, 1), Err(AppError::NotFound(1))));
    }

    #[test]
    fn deleting_a_missing_id_is_an_error() {
        let conn = seeded();
        assert!(matches!(run(&conn, 99), Err(AppError::NotFound(99))));
    }

    // --- prompting ---------------------------------------------------------

    #[test]
    fn yes_answers_are_accepted_in_any_casing() {
        for input in ["y\n", "Y\n", "yes\n", "YES\n", "  yes  \n"] {
            let (answer, _) = ask_with(input);
            assert!(answer, "`{}` should mean yes", input.trim());
        }
    }

    #[test]
    fn everything_else_means_no() {
        // Including a bare Enter: the safe answer is the default.
        for input in ["n\n", "no\n", "\n", "maybe\n", "yep\n", "ye\n"] {
            let (answer, _) = ask_with(input);
            assert!(!answer, "`{}` should mean no", input.trim());
        }
    }

    #[test]
    fn end_of_input_means_no() {
        // `et delete 1 < /dev/null`. An unanswered question is not consent.
        let (answer, _) = ask_with("");
        assert!(!answer);
    }

    #[test]
    fn the_prompt_is_written_before_reading() {
        let (_, written) = ask_with("y\n");
        assert!(written.starts_with("Delete it? [y/N] "));
    }

    #[test]
    fn the_prompt_has_no_trailing_newline() {
        // The answer should be typed on the same line as the question.
        let (_, written) = ask_with("y\n");
        assert!(!written.ends_with('\n'), "{written:?}");
    }

    #[test]
    fn end_of_input_leaves_the_cursor_on_a_fresh_line() {
        let (_, written) = ask_with("");
        assert!(written.ends_with('\n'));
    }

    // --- wording -----------------------------------------------------------

    #[test]
    fn the_prompt_describes_what_will_be_deleted() {
        let conn = seeded();
        let expense = find(&conn, 1).unwrap();
        assert_eq!(
            prompt(&expense),
            "Delete #1: $42.50 groceries — Trader Joe's (2026-09-10)? [y/N] "
        );
    }

    #[test]
    fn the_default_answer_is_no() {
        let conn = seeded();
        // Capital N is the convention for "this is what Enter does".
        assert!(prompt(&find(&conn, 1).unwrap()).contains("[y/N]"));
    }

    #[test]
    fn add_and_delete_describe_a_row_identically() {
        // The shared `describe` is what guarantees this; without it the two
        // messages drift apart the first time either is edited.
        let conn = seeded();
        let expense = find(&conn, 1).unwrap();
        let described = super::super::describe(&expense);

        assert_eq!(add::confirmation(&expense), format!("Added {described}"));
        assert_eq!(confirmation(&expense), format!("Deleted {described}"));
    }
}
