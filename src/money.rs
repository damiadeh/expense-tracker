//! A monetary amount, stored as a whole number of cents.

use std::fmt;
use std::iter::Sum;
use std::ops::{Add, AddAssign, Neg, Sub};
use std::str::FromStr;

/// An exact monetary amount.
///
/// This is a *newtype*: a tuple struct wrapping a single `i64`. It costs
/// nothing at runtime (the compiler lays it out exactly like an `i64`) but
/// makes `Money` a distinct type, so you cannot accidentally pass a row ID
/// where an amount is expected.
///
/// The inner field is private, so cents can only be produced through
/// [`Money::from_cents`] or by parsing — there is no way to construct one
/// from a float.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Money(i64);

impl Money {
    /// Zero. Useful as the starting value of a fold.
    pub const ZERO: Money = Money(0);

    /// Build an amount from a raw cent count.
    pub const fn from_cents(cents: i64) -> Self {
        Money(cents)
    }

    /// The raw cent count. This is what gets written to SQLite.
    pub const fn cents(self) -> i64 {
        self.0
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    /// Addition that returns `None` instead of overflowing.
    ///
    /// The `Add` impl below uses plain `+`, which panics on overflow in debug
    /// builds and wraps in release builds. Use this when the inputs are not
    /// under your control.
    pub const fn checked_add(self, rhs: Money) -> Option<Money> {
        match self.0.checked_add(rhs.0) {
            Some(cents) => Some(Money(cents)),
            None => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Everything that can go wrong turning a string into a [`Money`].
///
/// Stage 1 implemented `Display` and `Error` for this type by hand, in about
/// twenty lines. `#[derive(Error)]` from the `thiserror` crate generates
/// exactly those two impls from the `#[error("...")]` attributes below —
/// the strings are format templates, and the fields are in scope inside them.
///
/// Note what `thiserror` is *not*: it is not a runtime dependency doing work
/// while your program runs. It is a compile-time macro that writes ordinary
/// Rust and then gets out of the way.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseMoneyError {
    /// The input was empty or only whitespace.
    #[error("amount is empty")]
    Empty,

    /// The input contained something that is not a digit, sign, or separator.
    ///
    /// `{0}` refers to the first (and only) field of this tuple variant.
    #[error("`{0}` is not a valid amount")]
    InvalidCharacter(String),

    /// There were zero, or more than two, digits after the decimal point.
    ///
    /// We reject `"1.234"` rather than rounding it. Silently turning a third
    /// decimal place into a rounding decision is how money quietly goes missing.
    ///
    /// `{found}` refers to the named field of this struct variant.
    #[error("expected 1 or 2 digits after the decimal point, found {found}")]
    DecimalPlaces { found: usize },

    /// The amount does not fit in an `i64` cent count.
    #[error("amount is too large")]
    Overflow,
}

impl FromStr for Money {
    type Err = ParseMoneyError;

    /// Parses `"42.50"`, `"42"`, `".50"`, `"-3.99"`, `"$1,234.56"`.
    ///
    /// Implementing `FromStr` is what makes `"42.50".parse::<Money>()` work,
    /// and it is also the trait clap uses to turn a command-line argument into
    /// a `Money` without any extra code from us.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(ParseMoneyError::Empty);
        }

        // `strip_prefix` returns `Option<&str>` — `Some` with the remainder if
        // the prefix matched, `None` otherwise. Stripping `$` on both sides of
        // the sign accepts `-$5` and `$-5` alike.
        let body = trimmed.strip_prefix('$').unwrap_or(trimmed);
        let (negative, body) = match body.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, body.strip_prefix('+').unwrap_or(body)),
        };
        let body = body.strip_prefix('$').unwrap_or(body);

        // `split_once` splits at the *first* match only. A second '.' stays in
        // `frac` and gets caught by the digit check below.
        let (whole, frac) = match body.split_once('.') {
            Some((w, f)) => (w, Some(f)),
            None => (body, None),
        };

        let invalid = || ParseMoneyError::InvalidCharacter(trimmed.to_string());

        let mut whole_cents: i64 = 0;
        let mut saw_digit = false;
        for ch in whole.chars() {
            // Thousands separators are accepted and ignored: "1,234" == "1234".
            if ch == ',' || ch == '_' {
                continue;
            }
            let digit = ch.to_digit(10).ok_or_else(invalid)?;
            saw_digit = true;
            whole_cents = whole_cents
                .checked_mul(10)
                .and_then(|v| v.checked_add(i64::from(digit)))
                .ok_or(ParseMoneyError::Overflow)?;
        }

        let frac_cents = match frac {
            None => {
                if !saw_digit {
                    return Err(invalid());
                }
                0
            }
            Some(f) => {
                // Check for stray characters *before* counting decimal places,
                // so "12.3.4" is reported as an invalid amount rather than as
                // having three decimal places. Order of validation is part of
                // the error message quality.
                if !f.chars().all(|c| c.is_ascii_digit()) {
                    return Err(invalid());
                }
                if f.is_empty() || f.len() > 2 {
                    return Err(ParseMoneyError::DecimalPlaces { found: f.len() });
                }
                // Safe from overflow: at most two digits reach this point.
                let mut value: i64 = 0;
                for ch in f.chars() {
                    let digit = ch.to_digit(10).ok_or_else(invalid)?;
                    value = value * 10 + i64::from(digit);
                }
                // "5.5" means 50 cents, not 5.
                if f.len() == 1 { value * 10 } else { value }
            }
        };

        let cents = whole_cents
            .checked_mul(100)
            .and_then(|v| v.checked_add(frac_cents))
            .ok_or(ParseMoneyError::Overflow)?;

        Ok(Money(if negative { -cents } else { cents }))
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        // `unsigned_abs` returns a `u64`, which is why this cannot panic on
        // `i64::MIN` the way `abs()` would.
        let abs = self.0.unsigned_abs();
        // `{:02}` pads to two digits with zeros, so 5 cents prints as "$0.05".
        write!(f, "{sign}${}.{:02}", abs / 100, abs % 100)
    }
}

// ---------------------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------------------

impl Add for Money {
    type Output = Money;

    fn add(self, rhs: Money) -> Money {
        Money(self.0 + rhs.0)
    }
}

impl Sub for Money {
    type Output = Money;

    fn sub(self, rhs: Money) -> Money {
        Money(self.0 - rhs.0)
    }
}

impl Neg for Money {
    type Output = Money;

    fn neg(self) -> Money {
        Money(-self.0)
    }
}

impl AddAssign for Money {
    fn add_assign(&mut self, rhs: Money) {
        self.0 += rhs.0;
    }
}

/// Lets you write `amounts.into_iter().sum::<Money>()`.
impl Sum for Money {
    fn sum<I: Iterator<Item = Money>>(iter: I) -> Money {
        iter.fold(Money::ZERO, |acc, m| acc + m)
    }
}

/// The same, for iterators that yield references — `slice.iter().sum()`.
///
/// Two impls are needed because `Vec<Money>::iter()` yields `&Money`, not
/// `Money`. The `'a` lifetime says the references only need to outlive the
/// iteration, not the returned total — the total is a fresh `Money` that
/// borrows nothing.
impl<'a> Sum<&'a Money> for Money {
    fn sum<I: Iterator<Item = &'a Money>>(iter: I) -> Money {
        iter.copied().sum()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// `#[cfg(test)]` means this module is only compiled when running `cargo test`.
// It adds nothing to the shipped binary.
#[cfg(test)]
mod tests {
    // `super::*` pulls in everything from the parent module, including the
    // private field, which is why unit tests can reach internals that an
    // integration test in `tests/` could not.
    use super::*;

    fn parse(s: &str) -> Result<Money, ParseMoneyError> {
        s.parse::<Money>()
    }

    #[test]
    fn parses_plain_decimal() {
        assert_eq!(parse("42.50").unwrap(), Money::from_cents(4250));
        assert_eq!(parse("0.99").unwrap(), Money::from_cents(99));
        assert_eq!(parse("0.05").unwrap(), Money::from_cents(5));
    }

    #[test]
    fn parses_whole_numbers() {
        assert_eq!(parse("42").unwrap(), Money::from_cents(4200));
        assert_eq!(parse("0").unwrap(), Money::ZERO);
    }

    #[test]
    fn parses_leading_dot() {
        assert_eq!(parse(".50").unwrap(), Money::from_cents(50));
    }

    #[test]
    fn single_decimal_digit_means_tenths() {
        // "5.5" is five dollars fifty, not five dollars five cents.
        assert_eq!(parse("5.5").unwrap(), Money::from_cents(550));
    }

    #[test]
    fn parses_negatives() {
        assert_eq!(parse("-3.99").unwrap(), Money::from_cents(-399));
        assert_eq!(parse("-$3.99").unwrap(), Money::from_cents(-399));
        assert_eq!(parse("$-3.99").unwrap(), Money::from_cents(-399));
    }

    #[test]
    fn parses_currency_symbol_and_separators() {
        assert_eq!(parse("$1,234.56").unwrap(), Money::from_cents(123_456));
        assert_eq!(parse("1_000").unwrap(), Money::from_cents(100_000));
    }

    #[test]
    fn ignores_surrounding_whitespace() {
        assert_eq!(parse("  42.50  ").unwrap(), Money::from_cents(4250));
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(parse(""), Err(ParseMoneyError::Empty));
        assert_eq!(parse("   "), Err(ParseMoneyError::Empty));
    }

    #[test]
    fn rejects_non_numeric() {
        assert!(matches!(
            parse("banana"),
            Err(ParseMoneyError::InvalidCharacter(_))
        ));
        assert!(matches!(
            parse("12.3.4"),
            Err(ParseMoneyError::InvalidCharacter(_))
        ));
        assert!(matches!(
            parse("$"),
            Err(ParseMoneyError::InvalidCharacter(_))
        ));
    }

    #[test]
    fn rejects_bad_decimal_places() {
        // Rounding is refused rather than guessed at.
        assert_eq!(
            parse("1.234"),
            Err(ParseMoneyError::DecimalPlaces { found: 3 })
        );
        assert_eq!(
            parse("5."),
            Err(ParseMoneyError::DecimalPlaces { found: 0 })
        );
    }

    #[test]
    fn rejects_overflow() {
        assert_eq!(
            parse("99999999999999999999"),
            Err(ParseMoneyError::Overflow)
        );
    }

    #[test]
    fn displays_with_two_decimal_places() {
        assert_eq!(Money::from_cents(4250).to_string(), "$42.50");
        assert_eq!(Money::from_cents(5).to_string(), "$0.05");
        assert_eq!(Money::from_cents(100).to_string(), "$1.00");
        assert_eq!(Money::ZERO.to_string(), "$0.00");
        assert_eq!(Money::from_cents(-399).to_string(), "-$3.99");
    }

    #[test]
    fn display_survives_i64_min() {
        // `abs()` would panic here; `unsigned_abs()` does not.
        let _ = Money::from_cents(i64::MIN).to_string();
    }

    #[test]
    fn parse_display_round_trip() {
        for input in ["42.50", "0.05", "1.00", "-3.99", "0.00"] {
            let parsed: Money = input.parse().unwrap();
            let shown = parsed.to_string();
            let reparsed: Money = shown.parse().unwrap();
            assert_eq!(parsed, reparsed, "round trip failed for {input}");
        }
    }

    #[test]
    fn arithmetic() {
        let a = Money::from_cents(4250);
        let b = Money::from_cents(1075);
        assert_eq!(a + b, Money::from_cents(5325));
        assert_eq!(a - b, Money::from_cents(3175));
        assert_eq!(-b, Money::from_cents(-1075));

        let mut total = Money::ZERO;
        total += a;
        total += b;
        assert_eq!(total, Money::from_cents(5325));
    }

    #[test]
    fn checked_add_reports_overflow() {
        let big = Money::from_cents(i64::MAX);
        assert_eq!(big.checked_add(Money::from_cents(1)), None);
        assert_eq!(
            Money::from_cents(1).checked_add(Money::from_cents(2)),
            Some(Money::from_cents(3))
        );
    }

    #[test]
    fn sums_owned_and_borrowed() {
        let amounts = vec![
            Money::from_cents(4250),
            Money::from_cents(1075),
            Money::from_cents(25),
        ];
        // Borrowed: `iter()` yields `&Money`.
        let by_ref: Money = amounts.iter().sum();
        // Owned: `into_iter()` consumes the vec and yields `Money`.
        let by_value: Money = amounts.clone().into_iter().sum();

        assert_eq!(by_ref, Money::from_cents(5350));
        assert_eq!(by_value, by_ref);
    }

    #[test]
    fn empty_sum_is_zero() {
        let none: Vec<Money> = Vec::new();
        assert_eq!(none.iter().sum::<Money>(), Money::ZERO);
    }

    #[test]
    fn sorts_by_amount() {
        // `Ord` is derived, so sorting works on the underlying cent count.
        // An array, not a `vec!` — the length is fixed and known at compile
        // time, so this needs no heap allocation. Clippy's `useless_vec` lint
        // catches exactly this.
        let mut amounts = [
            Money::from_cents(500),
            Money::from_cents(-100),
            Money::from_cents(250),
        ];
        amounts.sort();
        assert_eq!(amounts.first().copied(), Some(Money::from_cents(-100)));
        assert_eq!(amounts.last().copied(), Some(Money::from_cents(500)));
    }

    #[test]
    fn no_floating_point_drift() {
        // The reason this type exists: summing 0.1 ten times as f64 does not
        // equal 1.0, but summing it as Money does.
        let dime: Money = "0.10".parse().unwrap();
        let total: Money = std::iter::repeat_n(dime, 10).sum();
        assert_eq!(total, Money::from_cents(100));
        assert_eq!(total.to_string(), "$1.00");
    }
}
