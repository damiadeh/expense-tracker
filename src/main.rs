//! The `et` binary.
//!
//! Deliberately thin. Stage 6 only proves that arguments parse and reach the
//! right arm; Stage 7 onwards replaces each `println!` with a real command.

use clap::Parser;

use expense_tracker::cli::{Cli, Command};

fn main() {
    // `parse` handles `--help`, `--version`, and bad input by printing and
    // exiting on its own. Anything that comes back is already valid.
    let cli = Cli::parse();

    let db_path = match cli.db_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    };
    println!("database: {}", db_path.display());

    // Exhaustive: adding a variant to `Command` without handling it here is a
    // compile error, not a bug that shows up at runtime.
    match cli.command {
        Command::Add {
            amount,
            category,
            note,
            date,
        } => {
            println!("add {amount} to `{category}`");
            println!("  note: {}", note.as_deref().unwrap_or("(none)"));
            match date {
                Some(date) => println!("  date: {date}"),
                None => println!("  date: (today)"),
            }
        }
        Command::List {
            month,
            category,
            limit,
        } => {
            println!("list expenses");
            println!("  month:    {}", describe(month));
            println!("  category: {}", describe(category));
            println!("  limit:    {}", describe(limit));
        }
        Command::Summary { month } => {
            println!("summary");
            println!("  month: {}", describe(month));
        }
        Command::Delete { id, yes } => {
            println!("delete #{id}");
            println!("  skip confirmation: {yes}");
        }
    }
}

/// Renders an optional filter for the placeholder output.
fn describe<T: std::fmt::Display>(value: Option<T>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "(any)".to_string(),
    }
}
