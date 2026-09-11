//! The shapes that move through the application.
//!
//! Everything here is plain data plus the validation that keeps it honest.
//! No SQL, no printing, no command-line parsing — those live in `db`, in the
//! `commands` module, and in `cli` respectively. Keeping the domain types free
//! of those concerns is what lets you unit-test them without a database.

use std::fmt;
use std::str::FromStr;

use chrono::{Datelike, Local, NaiveDate};

use crate::error::{AppError, Result};
use crate::money::Money;

/// The longest category we will store. Long enough for "home-improvement",
/// short enough that the `list` table never wraps.
const MAX_CATEGORY_LEN: usize = 32;

// ---------------------------------------------------------------------------
// Expenses
// ---------------------------------------------------------------------------

/// An expense that exists in the database.
///
/// # Why this is a separate type from [`NewExpense`]
///
/// The obvious design is one struct with `id: Option<i64>` — `None` before it
/// is saved, `Some` after. That forces every reader to handle a `None` that,
/// in practice, can never happen: anything that came back from a `SELECT`
/// always has an id.
///
/// Two types make the illegal state unrepresentable. `NewExpense` has no id
/// field to be wrong about, `Expense` has a plain `i64`, and the transition
/// between them happens in exactly one place: [`NewExpense::saved_as`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expense {
    pub id: i64,
    pub amount: Money,
    pub category: String,
    /// Genuinely optional — most expenses do not need a note.
    pub note: Option<String>,
    /// A date, not a timestamp. An expense happened on a day; the hour it was
    /// typed in is not information anyone wants.
    pub date: NaiveDate,
}

/// An expense that has been validated but not yet stored.
///
/// The database assigns the id, so this type does not have one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewExpense {
    pub amount: Money,
    pub category: String,
    pub note: Option<String>,
    pub date: NaiveDate,
}

impl NewExpense {
    /// Validates and normalizes user input into a storable expense.
    ///
    /// This is the *only* constructor, and the fields it fills are already
    /// normalized — so anything holding a `NewExpense` can trust it. Validation
    /// belongs here at the edge, not in the database layer, so that the error
    /// messages can still refer to what the user actually typed.
    ///
    /// Takes `&str` for borrowed input and produces owned `String`s, because
    /// the struct outlives the command-line arguments it came from.
    pub fn new(amount: Money, category: &str, note: Option<&str>, date: NaiveDate) -> Result<Self> {
        Ok(NewExpense {
            amount,
            category: normalize_category(category)?,
            note: normalize_note(note),
            date,
        })
    }

    /// Pairs this with the id the database assigned, producing an [`Expense`].
    ///
    /// Takes `self` by value: once an expense is saved, the unsaved version is
    /// meaningless and should not be reachable. Ownership enforces that.
    pub fn saved_as(self, id: i64) -> Expense {
        Expense {
            id,
            amount: self.amount,
            category: self.category,
            note: self.note,
            date: self.date,
        }
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Trims, lowercases, and checks a category.
///
/// Categories are free-form text rather than an enum on purpose: an enum would
/// mean editing and recompiling the program to record a trip to the dentist.
/// We validate *shape* instead of membership.
///
/// Lowercasing at the boundary means `Groceries`, `groceries`, and `GROCERIES`
/// all become one category rather than three — normalization is what makes
/// `GROUP BY category` give the answer people expect.
pub fn normalize_category(input: &str) -> Result<String> {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return Err(AppError::InvalidCategory(
            input.to_string(),
            "must not be empty",
        ));
    }

    if trimmed.chars().any(char::is_whitespace) {
        return Err(AppError::InvalidCategory(
            input.to_string(),
            "must be a single word (try a hyphen)",
        ));
    }

    // `.chars().count()` counts characters, not bytes — `len()` would reject
    // an accented category several characters early.
    if trimmed.chars().count() > MAX_CATEGORY_LEN {
        return Err(AppError::InvalidCategory(
            input.to_string(),
            "is too long (max 32 characters)",
        ));
    }

    Ok(trimmed.to_lowercase())
}

/// Trims a note, treating whitespace-only input as no note at all.
///
/// Not a `Result`: there is no way for a note to be invalid, only absent.
fn normalize_note(note: Option<&str>) -> Option<String> {
    note.map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

/// Parses a `YYYY-MM-DD` date.
///
/// Rejects impossible dates like `2026-02-30`; chrono does that work for us.
pub fn parse_date(input: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(input.trim(), "%Y-%m-%d")
        .map_err(|_| AppError::InvalidDate(input.to_string()))
}

/// Today, in the machine's local timezone.
pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

// ---------------------------------------------------------------------------
// Months
// ---------------------------------------------------------------------------

/// A calendar month, used to filter lists and summaries.
///
/// Another newtype, for the same reason as [`Money`]: a `(i32, u32)` pair would
/// let you swap year and month at a call site with no complaint from the
/// compiler, and `"2026-09"` as a bare `String` would let an unvalidated one
/// reach the SQL layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Month {
    year: i32,
    month: u32,
}

impl Month {
    /// Builds a month, rejecting a month number outside 1–12.
    pub fn new(year: i32, month: u32) -> Result<Self> {
        if !(1..=12).contains(&month) {
            return Err(AppError::InvalidMonth(format!("{year:04}-{month:02}")));
        }
        Ok(Month { year, month })
    }

    /// The month a given date falls in.
    pub fn containing(date: NaiveDate) -> Self {
        Month {
            year: date.year(),
            month: date.month(),
        }
    }

    /// The current month.
    pub fn current() -> Self {
        Month::containing(today())
    }

    pub fn year(self) -> i32 {
        self.year
    }

    pub fn month(self) -> u32 {
        self.month
    }

    /// The pattern for a SQL `LIKE` against a `YYYY-MM-DD` text column.
    ///
    /// Dates are stored as zero-padded ISO text specifically so that this
    /// works — `LIKE '2026-09%'` and `ORDER BY date` both behave correctly on
    /// strings in that format, with no date functions needed.
    pub fn like_prefix(self) -> String {
        format!("{self}%")
    }
}

impl fmt::Display for Month {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

impl FromStr for Month {
    type Err = AppError;

    /// Parses `"2026-09"`.
    ///
    /// Implementing `FromStr` here does the same double duty it did for
    /// `Money`: `"2026-09".parse::<Month>()` works, and clap will use it to
    /// parse `--month` with no extra code in Stage 6.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let trimmed = s.trim();
        let invalid = || AppError::InvalidMonth(s.to_string());

        let (year, month) = trimmed.split_once('-').ok_or_else(invalid)?;

        // Fixed widths, so "2026-9" and "26-09" are rejected rather than
        // guessed at.
        if year.len() != 4 || month.len() != 2 {
            return Err(invalid());
        }

        let year: i32 = year.parse().map_err(|_| invalid())?;
        let month: u32 = month.parse().map_err(|_| invalid())?;

        Month::new(year, month).map_err(|_| invalid())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("test date must be valid")
    }

    // --- categories --------------------------------------------------------

    #[test]
    fn category_is_trimmed_and_lowercased() {
        assert_eq!(normalize_category("  Groceries  ").unwrap(), "groceries");
        assert_eq!(normalize_category("COFFEE").unwrap(), "coffee");
    }

    #[test]
    fn category_normalization_merges_casings() {
        // The reason lowercasing happens at the boundary: these must not end
        // up as three separate rows in a GROUP BY.
        let variants = ["Groceries", "groceries", "GROCERIES"];
        let normalized: Vec<String> = variants
            .iter()
            .map(|c| normalize_category(c).unwrap())
            .collect();
        assert!(normalized.iter().all(|c| c == "groceries"));
    }

    #[test]
    fn category_rejects_empty() {
        assert!(matches!(
            normalize_category(""),
            Err(AppError::InvalidCategory(_, _))
        ));
        assert!(matches!(
            normalize_category("   "),
            Err(AppError::InvalidCategory(_, _))
        ));
    }

    #[test]
    fn category_rejects_internal_whitespace() {
        let err = normalize_category("home improvement").unwrap_err();
        assert!(matches!(err, AppError::InvalidCategory(_, _)));
        assert!(err.to_string().contains("single word"));
    }

    #[test]
    fn category_accepts_hyphens_and_underscores() {
        assert_eq!(
            normalize_category("home-improvement").unwrap(),
            "home-improvement"
        );
        assert_eq!(normalize_category("car_repair").unwrap(), "car_repair");
    }

    #[test]
    fn category_length_is_counted_in_characters_not_bytes() {
        // "é" is two bytes but one character. A `len()` check would reject
        // this 32-character category as though it were 64 bytes long.
        let accented = "é".repeat(MAX_CATEGORY_LEN);
        assert!(accented.len() > MAX_CATEGORY_LEN);
        assert!(normalize_category(&accented).is_ok());

        let too_long = "a".repeat(MAX_CATEGORY_LEN + 1);
        assert!(matches!(
            normalize_category(&too_long),
            Err(AppError::InvalidCategory(_, _))
        ));
    }

    #[test]
    fn category_error_quotes_the_original_input() {
        // Not the trimmed version — the user should see what they typed.
        let err = normalize_category("  Home Improvement  ").unwrap_err();
        assert!(err.to_string().contains("Home Improvement"));
    }

    // --- notes -------------------------------------------------------------

    #[test]
    fn note_is_trimmed() {
        assert_eq!(
            normalize_note(Some("  Trader Joe's  ")),
            Some("Trader Joe's".to_string())
        );
    }

    #[test]
    fn blank_note_becomes_none() {
        // An empty string and an absent note mean the same thing, so they get
        // the same representation. Otherwise `list` would print a stray blank.
        assert_eq!(normalize_note(Some("   ")), None);
        assert_eq!(normalize_note(Some("")), None);
        assert_eq!(normalize_note(None), None);
    }

    // --- construction ------------------------------------------------------

    #[test]
    fn new_expense_normalizes_everything() {
        let expense = NewExpense::new(
            Money::from_cents(4250),
            "  Groceries ",
            Some("  Trader Joe's "),
            date(2026, 9, 10),
        )
        .unwrap();

        assert_eq!(expense.category, "groceries");
        assert_eq!(expense.note.as_deref(), Some("Trader Joe's"));
        assert_eq!(expense.amount, Money::from_cents(4250));
    }

    #[test]
    fn new_expense_rejects_bad_category() {
        let result = NewExpense::new(Money::from_cents(1), "", None, date(2026, 9, 10));
        assert!(matches!(result, Err(AppError::InvalidCategory(_, _))));
    }

    #[test]
    fn saved_as_carries_every_field_across() {
        let new = NewExpense::new(
            Money::from_cents(4250),
            "groceries",
            Some("Trader Joe's"),
            date(2026, 9, 10),
        )
        .unwrap();
        // Clone first, because `saved_as` consumes `self` — which is the point.
        let expected = new.clone();
        let saved = new.saved_as(17);

        assert_eq!(saved.id, 17);
        assert_eq!(saved.amount, expected.amount);
        assert_eq!(saved.category, expected.category);
        assert_eq!(saved.note, expected.note);
        assert_eq!(saved.date, expected.date);
    }

    // --- dates -------------------------------------------------------------

    #[test]
    fn parses_iso_dates() {
        assert_eq!(parse_date("2026-09-10").unwrap(), date(2026, 9, 10));
        assert_eq!(parse_date("  2026-09-10  ").unwrap(), date(2026, 9, 10));
    }

    #[test]
    fn rejects_impossible_dates() {
        // chrono does the calendar work: there is no 30th of February.
        assert!(matches!(
            parse_date("2026-02-30"),
            Err(AppError::InvalidDate(_))
        ));
        assert!(matches!(
            parse_date("2026-13-01"),
            Err(AppError::InvalidDate(_))
        ));
        assert!(matches!(
            parse_date("10/09/2026"),
            Err(AppError::InvalidDate(_))
        ));
    }

    #[test]
    fn leap_days_are_accepted_when_real() {
        assert!(parse_date("2028-02-29").is_ok());
        assert!(parse_date("2026-02-29").is_err());
    }

    // --- months ------------------------------------------------------------

    #[test]
    fn parses_months() {
        let m: Month = "2026-09".parse().unwrap();
        assert_eq!(m.year(), 2026);
        assert_eq!(m.month(), 9);
    }

    #[test]
    fn month_rejects_malformed_input() {
        // Fixed widths: no guessing at what "2026-9" or "26-09" meant.
        for bad in ["2026-9", "26-09", "2026", "2026-13", "2026-00", "banana"] {
            assert!(
                bad.parse::<Month>().is_err(),
                "{bad} should not parse as a month"
            );
        }
    }

    #[test]
    fn month_round_trips_through_display() {
        for input in ["2026-09", "2026-01", "2026-12"] {
            let parsed: Month = input.parse().unwrap();
            assert_eq!(parsed.to_string(), input);
        }
    }

    #[test]
    fn month_like_prefix_matches_iso_dates() {
        let september: Month = "2026-09".parse().unwrap();
        assert_eq!(september.like_prefix(), "2026-09%");

        // Zero-padded ISO text is what makes a plain string LIKE correct.
        let prefix = september.like_prefix();
        let stem = prefix.trim_end_matches('%');
        assert!(date(2026, 9, 1).to_string().starts_with(stem));
        assert!(date(2026, 9, 30).to_string().starts_with(stem));
        assert!(!date(2026, 10, 1).to_string().starts_with(stem));
    }

    #[test]
    fn month_containing_a_date() {
        assert_eq!(
            Month::containing(date(2026, 9, 10)),
            Month::new(2026, 9).unwrap()
        );
    }

    #[test]
    fn months_sort_chronologically() {
        // Derived `Ord` compares year first, then month, because that is the
        // field order in the struct. Swapping the fields would silently break
        // this — worth knowing before you reorder a struct that derives Ord.
        let mut months: Vec<Month> = ["2026-01", "2025-12", "2026-09"]
            .iter()
            .map(|m| m.parse().unwrap())
            .collect();
        months.sort();
        let sorted: Vec<String> = months.iter().map(Month::to_string).collect();
        assert_eq!(sorted, ["2025-12", "2026-01", "2026-09"]);
    }
}
