//! `et list` — show expenses, newest first.

use std::fmt::Write as _;

use rusqlite::Connection;

use crate::db::{self, ListFilter};
use crate::error::Result;
use crate::models::{Expense, Month, normalize_category};

/// Column headings, also the minimum width of each column.
const HEADERS: [&str; 5] = ["ID", "DATE", "CATEGORY", "AMOUNT", "NOTE"];

/// Two spaces between columns — enough to read, not enough to waste a narrow
/// terminal.
const GUTTER: &str = "  ";

/// Fetches the expenses matching the filters.
pub fn run(
    conn: &Connection,
    month: Option<Month>,
    category: Option<&str>,
    limit: Option<u32>,
) -> Result<Vec<Expense>> {
    // The filter has to go through the same normalization as stored
    // categories, or `--category Groceries` silently matches nothing: the
    // database holds "groceries", and `=` is exact.
    let category = category.map(normalize_category).transpose()?;

    db::list(
        conn,
        &ListFilter {
            month,
            category,
            limit,
        },
    )
}

/// Renders expenses as an aligned table.
///
/// Widths are measured from the data rather than hardcoded, so a long category
/// or a large amount widens its column instead of breaking the alignment.
pub fn render(expenses: &[Expense]) -> String {
    // Everything is stringified once, up front. Measuring and printing from
    // the same strings is what guarantees the widths are actually right.
    let rows: Vec<[String; 5]> = expenses
        .iter()
        .map(|e| {
            [
                e.id.to_string(),
                e.date.to_string(),
                e.category.clone(),
                e.amount.to_string(),
                e.note.clone().unwrap_or_default(),
            ]
        })
        .collect();

    let widths: Vec<usize> = (0..HEADERS.len())
        .map(|column| {
            rows.iter()
                .map(|row| display_width(&row[column]))
                .chain(std::iter::once(display_width(HEADERS[column])))
                .max()
                .unwrap_or(0)
        })
        .collect();

    // The heading row goes through exactly the same formatter as the data
    // rows. That is the only way the two are guaranteed to line up — a
    // separately hand-padded header is how tables drift out of alignment.
    let header = HEADERS.map(str::to_string);

    let mut out = format_row(&header, &widths);
    for row in &rows {
        let _ = write!(out, "\n{}", format_row(row, &widths));
    }

    out
}

/// Lays out one row: id and amount right-aligned, the rest left-aligned.
fn format_row(row: &[String; 5], widths: &[usize]) -> String {
    let line = format!(
        "{:>id$}{GUTTER}{:<date$}{GUTTER}{:<category$}{GUTTER}{:>amount$}{GUTTER}{}",
        row[0],
        row[1],
        row[2],
        row[3],
        row[4],
        id = widths[0],
        date = widths[1],
        category = widths[2],
        amount = widths[3],
    );

    // The note is last and often empty, which would leave the padding from the
    // amount column dangling at the end of the line. Trailing whitespace shows
    // up in diffs and in `cat -A`, so it gets trimmed.
    line.trim_end().to_string()
}

/// What to say when nothing matched.
///
/// "No expenses found" alone leaves the user wondering whether the tool is
/// broken or the filters were just too narrow, so the filters are echoed back.
pub fn empty_message(month: Option<Month>, category: Option<&str>) -> String {
    let mut message = String::from("No expenses found");

    if let Some(category) = category {
        let _ = write!(message, " in `{}`", category.trim().to_lowercase());
    }

    if let Some(month) = month {
        let _ = write!(message, " for {month}");
    }

    message.push('.');
    message
}

/// How wide a string is on screen, in characters rather than bytes.
///
/// `str::len()` counts bytes, so an accented category would be padded several
/// columns short. Rust's own `{:<width$}` pads by character count, so counting
/// the same way is what keeps them consistent.
fn display_width(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::add;
    use crate::models::parse_date;
    use crate::money::Money;
    use chrono::NaiveDate;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("test date must be valid")
    }

    fn expense(id: i64, cents: i64, category: &str, note: Option<&str>, d: NaiveDate) -> Expense {
        Expense {
            id,
            amount: Money::from_cents(cents),
            category: category.to_string(),
            note: note.map(str::to_string),
            date: d,
        }
    }

    fn sample() -> Vec<Expense> {
        vec![
            expense(
                17,
                4250,
                "groceries",
                Some("Trader Joe's"),
                date(2026, 9, 10),
            ),
            expense(12, 8814, "groceries", Some("Costco"), date(2026, 9, 3)),
        ]
    }

    fn seeded() -> Connection {
        let conn = db::open_in_memory().unwrap();
        let rows = [
            (4250, "groceries", Some("Trader Joe's"), "2026-09-10"),
            (8814, "groceries", Some("Costco"), "2026-09-03"),
            (5200, "transport", None, "2026-09-07"),
            (1000, "coffee", None, "2026-08-28"),
        ];
        for (cents, category, note, d) in rows {
            add::run(
                &conn,
                Money::from_cents(cents),
                category,
                note,
                Some(parse_date(d).unwrap()),
            )
            .unwrap();
        }
        conn
    }

    // --- rendering ---------------------------------------------------------

    #[test]
    fn renders_the_expected_table() {
        let table = render(&sample());
        assert_eq!(
            table,
            "\
ID  DATE        CATEGORY   AMOUNT  NOTE
17  2026-09-10  groceries  $42.50  Trader Joe's
12  2026-09-03  groceries  $88.14  Costco"
        );
    }

    #[test]
    fn columns_line_up() {
        // Every line must have its columns at the same offsets — that is what
        // "aligned" actually means, and eyeballing it is not a test.
        let table = render(&sample());
        let lines: Vec<&str> = table.lines().collect();

        let amount_column = lines[0].find("AMOUNT").unwrap();
        // Amounts are right-aligned, so they end where the heading ends.
        let heading_end = amount_column + "AMOUNT".len();
        for line in &lines[1..] {
            assert!(
                line[..heading_end].ends_with("$42.50") || line[..heading_end].ends_with("$88.14"),
                "amount not right-aligned in: {line}"
            );
        }
    }

    #[test]
    fn widths_grow_to_fit_the_data() {
        let wide = vec![expense(
            1234567,
            123_456_789,
            "home-improvement",
            None,
            date(2026, 9, 10),
        )];
        let table = render(&wide);
        let lines: Vec<&str> = table.lines().collect();

        // The CATEGORY heading is padded out to the width of the widest value
        // beneath it, so the AMOUNT column that follows still lines up.
        assert!(lines[0].contains("CATEGORY        "));
        assert!(lines[1].contains("home-improvement"));
        assert_eq!(
            lines[0].find("AMOUNT").map(|i| i + "AMOUNT".len()),
            lines[1]
                .find("$1234567.89")
                .map(|i| i + "$1234567.89".len()),
            "the amount column should end at the same offset on both lines"
        );
        // The heading row keeps its NOTE column even though no row has a note;
        // the data row, having nothing to put there, ends early.
        assert!(lines[0].ends_with("NOTE"));
        assert!(lines[1].ends_with("$1234567.89"));
    }

    #[test]
    fn no_line_has_trailing_whitespace() {
        // Rows without a note would otherwise end in the amount column's
        // padding, which shows up in diffs and when piping to other tools.
        let mixed = vec![
            expense(1, 100, "coffee", None, date(2026, 9, 1)),
            expense(2, 200, "groceries", Some("Costco"), date(2026, 9, 2)),
        ];
        for line in render(&mixed).lines() {
            assert_eq!(line, line.trim_end(), "trailing whitespace in: {line:?}");
        }
    }

    #[test]
    fn headers_are_printed_even_with_one_row() {
        let table = render(&sample()[..1]);
        assert!(table.starts_with("ID  DATE"));
        assert_eq!(table.lines().count(), 2);
    }

    #[test]
    fn an_empty_slice_renders_just_the_headers() {
        // `run` never hands this to `render` — `main` prints the empty message
        // instead — but a renderer that panics on empty input is a trap.
        let table = render(&[]);
        assert_eq!(table, "ID  DATE  CATEGORY  AMOUNT  NOTE");
    }

    #[test]
    fn negative_amounts_stay_aligned() {
        let refund = vec![
            expense(1, -1500, "groceries", None, date(2026, 9, 1)),
            expense(2, 100, "coffee", None, date(2026, 9, 2)),
        ];
        let lines: Vec<String> = render(&refund).lines().map(str::to_string).collect();
        // "-$15.00" is wider than "$1.00", so the column is sized to it.
        assert!(lines[1].contains("-$15.00"));
        assert!(lines[2].contains("  $1.00"));
    }

    #[test]
    fn multibyte_categories_do_not_break_alignment() {
        // "café" is 5 bytes but 4 characters. Measuring in bytes would pad it
        // one column short and skew every line after it.
        let rows = vec![
            expense(1, 100, "café", None, date(2026, 9, 1)),
            expense(2, 200, "coffee", None, date(2026, 9, 2)),
        ];
        let table = render(&rows);
        let lines: Vec<&str> = table.lines().collect();

        let amount_offsets: Vec<usize> = lines
            .iter()
            .map(|line| line.chars().position(|c| c == '$').unwrap_or(0))
            .collect();
        assert_eq!(amount_offsets[1], amount_offsets[2]);
    }

    // --- empty state -------------------------------------------------------

    #[test]
    fn empty_message_mentions_the_filters() {
        assert_eq!(empty_message(None, None), "No expenses found.");
        assert_eq!(
            empty_message(Some("2026-09".parse().unwrap()), None),
            "No expenses found for 2026-09."
        );
        assert_eq!(
            empty_message(None, Some("coffee")),
            "No expenses found in `coffee`."
        );
        assert_eq!(
            empty_message(Some("2026-09".parse().unwrap()), Some("coffee")),
            "No expenses found in `coffee` for 2026-09."
        );
    }

    // --- run ---------------------------------------------------------------

    #[test]
    fn returns_everything_by_default() {
        let conn = seeded();
        assert_eq!(run(&conn, None, None, None).unwrap().len(), 4);
    }

    #[test]
    fn filters_by_month_and_category() {
        let conn = seeded();
        let month = Some("2026-09".parse().unwrap());
        assert_eq!(run(&conn, month, None, None).unwrap().len(), 3);
        assert_eq!(run(&conn, month, Some("groceries"), None).unwrap().len(), 2);
    }

    #[test]
    fn the_category_filter_is_normalized_like_stored_categories() {
        // Without normalizing the filter, this matches nothing: the database
        // holds "groceries" and the comparison is exact.
        let conn = seeded();
        for spelling in ["Groceries", "GROCERIES", "  groceries  "] {
            assert_eq!(
                run(&conn, None, Some(spelling), None).unwrap().len(),
                2,
                "`{spelling}` should match the stored category"
            );
        }
    }

    #[test]
    fn an_invalid_category_filter_is_an_error() {
        let conn = seeded();
        assert!(run(&conn, None, Some("two words"), None).is_err());
    }

    #[test]
    fn respects_the_limit() {
        let conn = seeded();
        assert_eq!(run(&conn, None, None, Some(2)).unwrap().len(), 2);
    }

    #[test]
    fn no_matches_is_an_empty_vec_not_an_error() {
        let conn = seeded();
        let found = run(&conn, Some("1999-01".parse().unwrap()), None, None).unwrap();
        assert!(found.is_empty());
    }
}
