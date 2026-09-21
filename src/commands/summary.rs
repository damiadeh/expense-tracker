//! `et summary` — total spending per category, largest first.

use std::fmt::Write as _;

use rusqlite::Connection;

use crate::db::{self, CategoryTotal};
use crate::error::Result;
use crate::models::Month;
use crate::money::Money;

/// Width of the longest bar. Every other bar is scaled against it, so this is
/// the chart's resolution as well as its size.
const BAR_WIDTH: usize = 16;

/// Full block. A bar drawn from these reads as a solid rectangle at any width.
const BAR_CHAR: char = '█';

/// Which month a summary covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Month(Month),
    AllTime,
}

impl Period {
    /// Works out the period from the two flags the CLI offers.
    ///
    /// `--month` wins if given, `--all` means every month, and giving neither
    /// means the current month — the answer to "what have I spent?" is almost
    /// always about right now.
    pub fn resolve(month: Option<Month>, all: bool) -> Self {
        match (month, all) {
            (Some(month), _) => Period::Month(month),
            (None, true) => Period::AllTime,
            (None, false) => Period::Month(Month::current()),
        }
    }

    fn filter(self) -> Option<Month> {
        match self {
            Period::Month(month) => Some(month),
            Period::AllTime => None,
        }
    }
}

impl std::fmt::Display for Period {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Period::Month(month) => write!(f, "{month}"),
            Period::AllTime => write!(f, "all time"),
        }
    }
}

/// Totals spending per category over a period.
pub fn run(conn: &Connection, period: Period) -> Result<Vec<CategoryTotal>> {
    db::summary(conn, period.filter())
}

/// Renders the totals as a table with a bar chart.
pub fn render(totals: &[CategoryTotal]) -> String {
    // The denominator for percentages is the sum of *absolute* values, not the
    // grand total. With a refund in the data the grand total can be zero or
    // negative, and "-340% of spending" helps nobody. Summing absolute values
    // keeps the percentages meaningful and always adds up to 100%.
    let scale: i64 = totals.iter().map(|t| t.total.cents().abs()).sum();

    // The largest category fills the bar; everything else is drawn relative to
    // it. Scaling against the grand total instead would leave every bar short
    // in a summary with many categories.
    let largest = totals
        .iter()
        .map(|t| t.total.cents().abs())
        .max()
        .unwrap_or(0);

    let rows: Vec<(String, String, String, String)> = totals
        .iter()
        .map(|t| {
            (
                t.category.clone(),
                t.total.to_string(),
                bar(t.total, largest),
                percentage(t.total, scale),
            )
        })
        .collect();

    let category_width = rows
        .iter()
        .map(|(category, ..)| category.chars().count())
        .chain(std::iter::once("TOTAL".len()))
        .max()
        .unwrap_or(0);

    let grand = Money::from_cents(totals.iter().map(|t| t.total.cents()).sum());
    let amount_width = rows
        .iter()
        .map(|(_, amount, ..)| amount.chars().count())
        .chain(std::iter::once(grand.to_string().chars().count()))
        .max()
        .unwrap_or(0);

    let mut out = String::new();

    for (category, amount, bar, percent) in &rows {
        let line = format!(
            "{category:<category_width$}  {amount:>amount_width$}  {bar:<BAR_WIDTH$}  {percent:>5}"
        );
        let _ = writeln!(out, "{}", line.trim_end());
    }

    // A rule as wide as the columns it closes, so it never dangles past the
    // numbers or stops short of them.
    let rule_width = category_width + 2 + amount_width;
    let _ = writeln!(out, "{}", "─".repeat(rule_width));
    let _ = write!(
        out,
        "{:<category_width$}  {:>amount_width$}",
        "TOTAL",
        grand.to_string()
    );

    out
}

/// A bar whose length is this total's share of the largest one.
///
/// Integer division throughout, and deliberately truncating rather than
/// rounding: a category has to genuinely reach a block's worth of spending
/// before that block appears.
fn bar(total: Money, largest: i64) -> String {
    if largest == 0 {
        return String::new();
    }

    let blocks = (total.cents().abs() * BAR_WIDTH as i64) / largest;
    BAR_CHAR.to_string().repeat(blocks as usize)
}

/// A percentage to one decimal place, computed without ever using a float.
///
/// Multiplying by 1000 before dividing keeps a tenth of a percent of precision
/// in integer arithmetic; `+ scale / 2` rounds to nearest rather than
/// truncating. `0.1 + 0.2 != 0.3` applies to percentages exactly as it does to
/// money, so the same discipline holds all the way to the last line of output.
fn percentage(total: Money, scale: i64) -> String {
    if scale == 0 {
        return String::new();
    }

    let tenths = (total.cents().abs() * 1000 + scale / 2) / scale;
    format!("{}.{}%", tenths / 10, tenths % 10)
}

/// What to say when a period has no expenses in it.
pub fn empty_message(period: Period) -> String {
    match period {
        Period::Month(month) => format!("No expenses found for {month}."),
        Period::AllTime => "No expenses recorded yet.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::add;
    use crate::models::parse_date;

    fn total(category: &str, cents: i64) -> CategoryTotal {
        CategoryTotal {
            category: category.to_string(),
            total: Money::from_cents(cents),
        }
    }

    /// The worked example from the top of the project spec.
    fn sample() -> Vec<CategoryTotal> {
        vec![
            total("groceries", 13_064),
            total("transport", 5_200),
            total("coffee", 3_070),
        ]
    }

    fn seeded() -> Connection {
        let conn = db::open_in_memory().unwrap();
        let rows = [
            (4250, "groceries", "2026-09-10"),
            (8814, "groceries", "2026-09-03"),
            (5200, "transport", "2026-09-07"),
            (3070, "coffee", "2026-09-01"),
            (1000, "coffee", "2026-08-28"),
        ];
        for (cents, category, date) in rows {
            add::run(
                &conn,
                Money::from_cents(cents),
                category,
                None,
                Some(parse_date(date).unwrap()),
            )
            .unwrap();
        }
        conn
    }

    // --- periods -----------------------------------------------------------

    #[test]
    fn an_explicit_month_wins() {
        let september: Month = "2026-09".parse().unwrap();
        assert_eq!(
            Period::resolve(Some(september), false),
            Period::Month(september)
        );
    }

    #[test]
    fn all_means_every_month() {
        assert_eq!(Period::resolve(None, true), Period::AllTime);
        assert_eq!(Period::AllTime.filter(), None);
    }

    #[test]
    fn no_flags_means_the_current_month() {
        assert_eq!(
            Period::resolve(None, false),
            Period::Month(Month::current())
        );
    }

    // --- percentages -------------------------------------------------------

    #[test]
    fn percentages_match_the_worked_example() {
        let totals = sample();
        let scale: i64 = totals.iter().map(|t| t.total.cents().abs()).sum();
        let percents: Vec<String> = totals.iter().map(|t| percentage(t.total, scale)).collect();
        assert_eq!(percents, ["61.2%", "24.4%", "14.4%"]);
    }

    #[test]
    fn percentages_sum_to_one_hundred() {
        // Rounding each share independently can add up to 99.9% or 100.1%.
        // Worth knowing which, rather than discovering it in front of a user.
        let totals = sample();
        let scale: i64 = totals.iter().map(|t| t.total.cents().abs()).sum();

        let tenths: i64 = totals
            .iter()
            .map(|t| (t.total.cents().abs() * 1000 + scale / 2) / scale)
            .sum();
        assert_eq!(
            tenths,
            1000,
            "percentages summed to {}.{}%",
            tenths / 10,
            tenths % 10
        );
    }

    #[test]
    fn percentages_round_to_nearest_not_down() {
        // One third of three dollars is 33.333%, which must not truncate to
        // 33.3% in one place and 33.4% in another.
        let scale = 300;
        assert_eq!(percentage(Money::from_cents(100), scale), "33.3%");
        assert_eq!(percentage(Money::from_cents(200), scale), "66.7%");
    }

    #[test]
    fn a_single_category_is_all_of_it() {
        assert_eq!(percentage(Money::from_cents(500), 500), "100.0%");
    }

    #[test]
    fn a_zero_scale_yields_no_percentage_rather_than_dividing_by_zero() {
        assert_eq!(percentage(Money::ZERO, 0), "");
    }

    // --- bars --------------------------------------------------------------

    #[test]
    fn the_largest_category_fills_the_bar() {
        assert_eq!(
            bar(Money::from_cents(13_064), 13_064).chars().count(),
            BAR_WIDTH
        );
    }

    #[test]
    fn bars_are_scaled_against_the_largest_not_the_total() {
        // transport is 39.8% of groceries, so 16 * 0.398 = 6 blocks.
        assert_eq!(bar(Money::from_cents(5_200), 13_064).chars().count(), 6);
        assert_eq!(bar(Money::from_cents(3_070), 13_064).chars().count(), 3);
    }

    #[test]
    fn bars_truncate_rather_than_round_up() {
        // A category has to genuinely reach a block's worth before it gets one,
        // so a rounding-up bar cannot overstate a tiny category.
        assert_eq!(bar(Money::from_cents(1), 13_064), "");
    }

    #[test]
    fn a_zero_largest_yields_no_bar_rather_than_dividing_by_zero() {
        assert_eq!(bar(Money::ZERO, 0), "");
    }

    #[test]
    fn negative_totals_still_draw_a_bar() {
        // A refund's size is still worth seeing; the minus sign is carried by
        // the amount column.
        assert_eq!(
            bar(Money::from_cents(-13_064), 13_064).chars().count(),
            BAR_WIDTH
        );
    }

    // --- rendering ---------------------------------------------------------

    #[test]
    fn renders_the_worked_example_from_the_spec() {
        let rendered = render(&sample());
        assert_eq!(
            rendered,
            "\
groceries  $130.64  ████████████████  61.2%
transport   $52.00  ██████            24.4%
coffee      $30.70  ███               14.4%
──────────────────
TOTAL      $213.34"
        );
    }

    #[test]
    fn the_total_is_the_sum_of_the_rows() {
        let rendered = render(&sample());
        let last = rendered.lines().last().unwrap();
        assert!(last.starts_with("TOTAL"));
        assert!(last.ends_with("$213.34"));
    }

    #[test]
    fn the_rule_spans_the_category_and_amount_columns() {
        let rendered = render(&sample());
        let lines: Vec<&str> = rendered.lines().collect();
        let rule = lines[lines.len() - 2];
        let total_line = lines[lines.len() - 1];

        assert!(rule.chars().all(|c| c == '─'));
        assert_eq!(rule.chars().count(), total_line.chars().count());
    }

    #[test]
    fn no_line_has_trailing_whitespace() {
        for line in render(&sample()).lines() {
            assert_eq!(line, line.trim_end(), "trailing whitespace in: {line:?}");
        }
    }

    #[test]
    fn columns_stay_aligned_with_a_long_category_name() {
        let totals = vec![total("home-improvement", 100), total("tea", 50)];
        let rendered = render(&totals);
        let lines: Vec<&str> = rendered.lines().collect();

        let first_amount = lines[0].find('$').unwrap();
        let second_amount = lines[1].find('$').unwrap();
        assert_eq!(first_amount, second_amount);
    }

    #[test]
    fn renders_a_refund_without_breaking_alignment() {
        let totals = vec![total("groceries", 10_000), total("returns", -2_500)];
        let rendered = render(&totals);

        assert!(rendered.contains("-$25.00"));
        // $100.00 + -$25.00
        assert!(rendered.lines().last().unwrap().ends_with("$75.00"));
    }

    #[test]
    fn an_empty_summary_renders_just_a_zero_total() {
        // `main` prints the empty message instead of calling this, but a
        // renderer that panics on empty input is a trap.
        let rendered = render(&[]);
        assert!(rendered.ends_with("TOTAL  $0.00"));
    }

    // --- empty state -------------------------------------------------------

    #[test]
    fn empty_message_names_the_period() {
        assert_eq!(
            empty_message(Period::Month("2026-09".parse().unwrap())),
            "No expenses found for 2026-09."
        );
        assert_eq!(empty_message(Period::AllTime), "No expenses recorded yet.");
    }

    // --- run ---------------------------------------------------------------

    #[test]
    fn run_totals_a_single_month() {
        let conn = seeded();
        let totals = run(&conn, Period::Month("2026-09".parse().unwrap())).unwrap();
        assert_eq!(totals, sample());
    }

    #[test]
    fn run_over_all_time_includes_every_month() {
        let conn = seeded();
        let totals = run(&conn, Period::AllTime).unwrap();
        let coffee = totals.iter().find(|t| t.category == "coffee").unwrap();
        // September's $30.70 plus August's $10.00.
        assert_eq!(coffee.total, Money::from_cents(4070));
    }

    #[test]
    fn run_on_a_month_with_nothing_in_it_returns_empty() {
        let conn = seeded();
        let totals = run(&conn, Period::Month("1999-01".parse().unwrap())).unwrap();
        assert!(totals.is_empty());
    }

    #[test]
    fn the_rendered_total_matches_the_database() {
        // Cross-check: the aggregate SQL, the Sum impl from Stage 1, and the
        // rendered footer must all agree.
        let conn = seeded();
        let september = Period::Month("2026-09".parse().unwrap());
        let totals = run(&conn, september).unwrap();

        let summed: Money = totals.iter().map(|t| t.total).sum();
        assert_eq!(summed, Money::from_cents(21_334));
        assert!(render(&totals).ends_with(&summed.to_string()));
    }
}
