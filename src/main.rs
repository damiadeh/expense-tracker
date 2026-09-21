//! The `et` binary.
//!
//! Thin on purpose: parse arguments, open one connection, dispatch, print.
//! Every decision worth testing lives in the library, where a test can reach it
//! without spawning a process.

use clap::Parser;
use rusqlite::Connection;

use expense_tracker::cli::{Cli, Command};
use expense_tracker::commands;
use expense_tracker::error::Result;

fn main() {
    // `parse` handles `--help`, `--version`, and bad input by printing and
    // exiting 2 on its own. Anything that comes back is already valid.
    let cli = Cli::parse();

    // Errors are printed here, once, rather than at each call site. Returning
    // `Result` from `main` would work too, but it prints the `Debug`
    // representation — `Error: NotFound(99)` instead of a sentence.
    if let Err(error) = run(&cli) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run(cli: &Cli) -> Result<()> {
    // Opened once and lent to each command as `&Connection`. The commands
    // borrow it; none of them take ownership, so `main` can keep using it.
    let conn = db_connection(cli)?;

    match &cli.command {
        Command::Add {
            amount,
            category,
            note,
            date,
        } => {
            let saved = commands::add::run(
                &conn,
                *amount,
                category,
                // `as_deref` turns `&Option<String>` into `Option<&str>` —
                // borrowing the contents instead of cloning them.
                note.as_deref(),
                *date,
            )?;
            println!("{}", commands::add::confirmation(&saved));
        }

        Command::List { .. } => println!("list: not implemented yet (stage 8)"),
        Command::Summary { .. } => println!("summary: not implemented yet (stage 9)"),
        Command::Delete { .. } => println!("delete: not implemented yet (stage 10)"),
    }

    Ok(())
}

/// Opens the database this invocation should use, creating it if needed.
fn db_connection(cli: &Cli) -> Result<Connection> {
    expense_tracker::db::open(&cli.db_path()?)
}
