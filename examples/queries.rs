//! Exercises the Stage 5 query functions against a real database file.
//!
//! ```sh
//! ET_DB_PATH=/tmp/et-demo.db cargo run --example queries
//! ```

use expense_tracker::db::{self, ListFilter};
use expense_tracker::{Money, Month, NewExpense};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = db::default_db_path()?;
    println!("database: {}\n", path.display());
    let conn = db::open(&path)?;

    let seed = [
        (4250, "groceries", Some("Trader Joe's"), "2026-09-10"),
        (8814, "groceries", Some("Costco"), "2026-09-03"),
        (5200, "transport", None, "2026-09-07"),
        (3070, "coffee", None, "2026-09-01"),
        (1000, "coffee", None, "2026-08-28"),
    ];
    for (cents, category, note, date) in seed {
        let expense = NewExpense::new(
            Money::from_cents(cents),
            category,
            note,
            expense_tracker::models::parse_date(date)?,
        )?;
        db::insert(&conn, &expense)?;
    }

    let september: Month = "2026-09".parse()?;

    println!("-- list, September --");
    let filter = ListFilter {
        month: Some(september),
        ..Default::default()
    };
    for e in db::list(&conn, &filter)? {
        println!(
            "{:>3}  {}  {:<10} {:>9}  {}",
            e.id,
            e.date,
            e.category,
            e.amount.to_string(),
            e.note.as_deref().unwrap_or("")
        );
    }

    println!("\n-- summary, September --");
    let totals = db::summary(&conn, Some(september))?;
    for t in &totals {
        println!("{:<10} {:>9}", t.category, t.total.to_string());
    }
    let grand: Money = totals.iter().map(|t| t.total).sum();
    println!("{:<10} {:>9}", "TOTAL", grand.to_string());

    println!("\n-- delete --");
    let id = db::list(&conn, &ListFilter::default())?[0].id;
    db::delete(&conn, id)?;
    println!("deleted #{id}");
    match db::delete(&conn, id) {
        Err(e) => println!("deleting it again: {e}"),
        Ok(()) => println!("deleting it again unexpectedly succeeded"),
    }

    Ok(())
}
