//! The `et` binary.
//!
//! Thin on purpose: parse arguments, open one connection, dispatch, print.
//! Every decision worth testing lives in the library, where a test can reach it
//! without spawning a process.

use std::io::{self, Write};

use clap::Parser;
use rusqlite::Connection;

use expense_tracker::cli::{Cli, Command};
use expense_tracker::commands;
use expense_tracker::error::{AppError, Result};

fn main() {
    // `parse` handles `--help`, `--version`, and bad input by printing and
    // exiting 2 on its own. Anything that comes back is already valid.
    let cli = Cli::parse();

    // Errors are printed here, once, rather than at each call site. Returning
    // `Result` from `main` would work too, but it prints the `Debug`
    // representation — `Error: NotFound(99)` instead of a sentence.
    match run(&cli) {
        Ok(()) => {}
        // `et list | head -3` closes the pipe while we are still writing. That
        // is not a failure — the reader got what it asked for — so exit 0 and
        // say nothing. See `emit` below for why this reaches us as an error at
        // all rather than killing the process the way it would in C.
        Err(error) if is_broken_pipe(&error) => {}
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
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
            emit(&commands::add::confirmation(&saved))?;
        }

        Command::List {
            month,
            category,
            limit,
        } => {
            let found = commands::list::run(&conn, *month, category.as_deref(), *limit)?;
            if found.is_empty() {
                // Not an error: the filters were simply too narrow. Exit 0.
                emit(&commands::list::empty_message(*month, category.as_deref()))?;
            } else {
                emit(&commands::list::render(&found))?;
            }
        }

        Command::Summary { .. } => emit("summary: not implemented yet (stage 9)")?,
        Command::Delete { .. } => emit("delete: not implemented yet (stage 10)")?,
    }

    Ok(())
}

/// Writes a line to stdout, returning write errors instead of panicking.
///
/// `println!` unwraps the underlying write, so a closed pipe becomes
/// `panicked at 'failed printing to stdout: Broken pipe'` — a backtrace at the
/// user for doing something completely reasonable like `et list | head -3`.
///
/// The reason it happens at all is that Rust sets `SIGPIPE` to ignored at
/// startup, unlike a C program, which would simply be killed by the signal.
/// The write therefore returns `EPIPE` rather than terminating the process,
/// and it is on us to decide what that means. `main` treats it as success.
fn emit(text: &str) -> Result<()> {
    // Locking once is also the faster path: every unlocked `println!` acquires
    // and releases the lock on its own.
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    writeln!(handle, "{text}")?;
    handle.flush()?;
    Ok(())
}

/// Whether an error is just a reader that stopped listening.
fn is_broken_pipe(error: &AppError) -> bool {
    matches!(error, AppError::Io(io_error) if io_error.kind() == io::ErrorKind::BrokenPipe)
}

/// Opens the database this invocation should use, creating it if needed.
fn db_connection(cli: &Cli) -> Result<Connection> {
    expense_tracker::db::open(&cli.db_path()?)
}
