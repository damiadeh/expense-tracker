//! `et add` — record a new expense.

use chrono::NaiveDate;
use rusqlite::Connection;

use crate::db;
use crate::error::Result;
use crate::models::{Expense, NewExpense, today};
use crate::money::Money;

/// Validates the arguments, stores the expense, and returns it with its new id.
///
/// The `Option<NaiveDate>` becomes "today" here rather than in the parser: a
/// default that depends on the clock belongs in the code that does the work,
/// not in the code that reads the command line.
pub fn run(
    conn: &Connection,
    amount: Money,
    category: &str,
    note: Option<&str>,
    date: Option<NaiveDate>,
) -> Result<Expense> {
    // Validation happens here, at the edge, where the error can still quote
    // what the user typed. By the time a NewExpense exists it is known good,
    // so `db::insert` does not re-check anything.
    let expense = NewExpense::new(amount, category, note, date.unwrap_or_else(today))?;

    let id = db::insert(conn, &expense)?;

    // Consumes the NewExpense: the unsaved version is meaningless now, and
    // ownership makes it unreachable rather than merely discouraged.
    Ok(expense.saved_as(id))
}

/// The line printed after a successful `add`.
///
/// Separate from [`run`] so it can be tested against an `Expense` built by
/// hand, with no database and no captured output.
pub fn confirmation(expense: &Expense) -> String {
    format!("Added {}", super::describe(expense))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("test date must be valid")
    }

    fn conn() -> Connection {
        db::open_in_memory().unwrap()
    }

    #[test]
    fn stores_the_expense_and_returns_it_with_an_id() {
        let conn = conn();
        let saved = run(
            &conn,
            Money::from_cents(4250),
            "groceries",
            Some("Trader Joe's"),
            Some(date(2026, 9, 10)),
        )
        .unwrap();

        assert_eq!(saved.id, 1);

        // Round-trip through the database, not just the returned value.
        let fetched = db::get(&conn, saved.id).unwrap();
        assert_eq!(fetched, saved);
    }

    #[test]
    fn normalizes_category_and_note_on_the_way_in() {
        let conn = conn();
        let saved = run(
            &conn,
            Money::from_cents(100),
            "  Groceries  ",
            Some("   "),
            Some(date(2026, 9, 10)),
        )
        .unwrap();

        assert_eq!(saved.category, "groceries");
        // Whitespace-only note is the same as no note.
        assert_eq!(saved.note, None);
    }

    #[test]
    fn defaults_to_today_when_no_date_is_given() {
        let conn = conn();
        let saved = run(&conn, Money::from_cents(100), "coffee", None, None).unwrap();
        assert_eq!(saved.date, today());
    }

    #[test]
    fn an_explicit_date_wins_over_today() {
        let conn = conn();
        let chosen = date(2020, 1, 1);
        let saved = run(&conn, Money::from_cents(100), "coffee", None, Some(chosen)).unwrap();
        assert_eq!(saved.date, chosen);
    }

    #[test]
    fn rejects_an_invalid_category_without_writing_anything() {
        let conn = conn();
        let result = run(&conn, Money::from_cents(100), "", None, None);

        assert!(matches!(result, Err(AppError::InvalidCategory(_, _))));
        // The `?` returned before `db::insert` ran, so the table is untouched.
        let count = db::list(&conn, &db::ListFilter::default()).unwrap().len();
        assert_eq!(count, 0);
    }

    #[test]
    fn ids_increase_across_calls() {
        let conn = conn();
        let first = run(&conn, Money::from_cents(100), "coffee", None, None).unwrap();
        let second = run(&conn, Money::from_cents(200), "coffee", None, None).unwrap();
        assert_eq!((first.id, second.id), (1, 2));
    }

    #[test]
    fn accepts_a_negative_amount_as_a_refund() {
        let conn = conn();
        let saved = run(&conn, Money::from_cents(-1500), "groceries", None, None).unwrap();
        assert_eq!(saved.amount, Money::from_cents(-1500));
    }

    #[test]
    fn confirmation_includes_the_note_when_there_is_one() {
        let expense = Expense {
            id: 17,
            amount: Money::from_cents(4250),
            category: "groceries".to_string(),
            note: Some("Trader Joe's".to_string()),
            date: date(2026, 9, 10),
        };
        assert_eq!(
            confirmation(&expense),
            "Added #17: $42.50 groceries — Trader Joe's (2026-09-10)"
        );
    }

    #[test]
    fn confirmation_omits_the_dash_when_there_is_no_note() {
        let expense = Expense {
            id: 3,
            amount: Money::from_cents(1200),
            category: "coffee".to_string(),
            note: None,
            date: date(2026, 9, 10),
        };
        assert_eq!(
            confirmation(&expense),
            "Added #3: $12.00 coffee (2026-09-10)"
        );
    }
}
