//! Command-line surface: the argument types, and nothing else.
//!
//! This module describes *what the user can type*. It does no work — no
//! database, no printing, no validation beyond parsing. Keeping it that way
//! means the argument structure can be tested without a database, and the
//! command implementations in Stage 7 onwards can be tested without clap.

use std::path::PathBuf;

use chrono::NaiveDate;
use clap::{Parser, Subcommand};

use crate::db;
use crate::error::Result;
use crate::models::{self, Month};
use crate::money::Money;

/// Track expenses from the command line.
#[derive(Debug, Parser)]
#[command(
    name = "et",
    // Pulls the version straight from Cargo.toml, so `et --version` can never
    // drift from the package metadata.
    version,
    about,
    // Bare `et` prints help instead of a terse "missing subcommand" error.
    arg_required_else_help = true
)]
pub struct Cli {
    // NOTE: `///` doc comments on these fields become the text users see in
    // `--help`. Implementation notes go in `//` comments like this one, or
    // they leak into the interface.
    //
    // `global = true` means the flag is accepted after any subcommand too —
    // both `et --db x.db list` and `et list --db x.db` work.
    /// Use this database file instead of the default
    #[arg(long, short = 'D', global = true, value_name = "PATH")]
    pub db: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// Where this invocation should read and write.
    ///
    /// An explicit `--db` beats the `ET_DB_PATH` environment variable, which in
    /// turn beats the platform data directory. Flag over environment over
    /// default is the conventional precedence; users expect it.
    pub fn db_path(&self) -> Result<PathBuf> {
        match &self.db {
            Some(path) => Ok(path.clone()),
            None => db::default_db_path(),
        }
    }
}

/// The subcommands.
///
/// An enum, so adding a variant makes every `match` over it a compile error
/// until it is handled. That is the whole reason dispatch is modelled this way
/// rather than as a string compared in an `if`/`else` chain.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Record a new expense.
    Add {
        // Parsed by the `FromStr` impl on `Money` from Stage 1. clap finds it
        // automatically, so "banana" is rejected here, with our own error
        // message, before any of our code runs.
        /// Amount spent, e.g. 42.50
        amount: Money,

        /// What it was for, e.g. groceries
        #[arg(short, long)]
        category: String,

        /// An optional reminder of what this was
        #[arg(short, long)]
        note: Option<String>,

        // `value_parser` points at our own function so a bad date produces
        // `AppError::InvalidDate` rather than chrono's wording.
        /// When it happened, as YYYY-MM-DD [default: today]
        #[arg(short, long, value_parser = models::parse_date)]
        date: Option<NaiveDate>,
    },

    /// List expenses, newest first.
    List {
        /// Only this month, as YYYY-MM
        #[arg(short, long)]
        month: Option<Month>,

        /// Only this category
        #[arg(short, long)]
        category: Option<String>,

        /// Show at most this many
        #[arg(short, long)]
        limit: Option<u32>,
    },

    /// Total spending per category.
    Summary {
        /// Which month, as YYYY-MM [default: this month]
        #[arg(short, long)]
        month: Option<Month>,
    },

    /// Delete an expense by id.
    Delete {
        /// The id shown by `et list`
        id: i64,

        /// Skip the confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// Parses an argument list as if it were typed at a shell.
    ///
    /// `try_parse_from` returns a `Result` instead of exiting the process the
    /// way `parse` does, which is what makes the parser testable at all.
    fn parse(args: &[&str]) -> std::result::Result<Cli, clap::Error> {
        Cli::try_parse_from(args)
    }

    #[test]
    fn the_command_definition_is_internally_consistent() {
        // clap's own sanity check: duplicated short flags, an argument
        // referring to a group that does not exist, and similar mistakes are
        // caught here rather than at runtime in front of a user.
        Cli::command().debug_assert();
    }

    #[test]
    fn add_parses_every_option() {
        let cli = parse(&[
            "et",
            "add",
            "42.50",
            "--category",
            "groceries",
            "--note",
            "Trader Joe's",
            "--date",
            "2026-09-10",
        ])
        .unwrap();

        match cli.command {
            Command::Add {
                amount,
                category,
                note,
                date,
            } => {
                assert_eq!(amount, Money::from_cents(4250));
                assert_eq!(category, "groceries");
                assert_eq!(note.as_deref(), Some("Trader Joe's"));
                assert_eq!(date, Some(models::parse_date("2026-09-10").unwrap()));
            }
            other => panic!("expected Add, got {other:?}"),
        }
    }

    #[test]
    fn short_flags_match_long_ones() {
        let long = parse(&["et", "add", "42.50", "--category", "groceries"]).unwrap();
        let short = parse(&["et", "add", "42.50", "-c", "groceries"]).unwrap();
        assert_eq!(format!("{long:?}"), format!("{short:?}"));
    }

    #[test]
    fn optional_arguments_default_to_none() {
        let cli = parse(&["et", "add", "42.50", "-c", "groceries"]).unwrap();
        match cli.command {
            Command::Add { note, date, .. } => {
                assert_eq!(note, None);
                // `None` means "today", decided at Stage 7 rather than here —
                // the parser's job is to report what was typed, not to fill in
                // defaults that depend on the clock.
                assert_eq!(date, None);
            }
            other => panic!("expected Add, got {other:?}"),
        }
    }

    #[test]
    fn money_is_parsed_by_our_own_from_str() {
        // The Stage 1 impl, used by clap with no glue code.
        let err = parse(&["et", "add", "banana", "-c", "groceries"]).unwrap_err();
        assert!(err.to_string().contains("is not a valid amount"), "{err}");
    }

    #[test]
    fn dates_are_parsed_by_our_own_value_parser() {
        // chrono would say "input is out of range"; we say something clearer.
        let err = parse(&["et", "add", "10", "-c", "coffee", "-d", "2026-02-30"]).unwrap_err();
        assert!(err.to_string().contains("expected YYYY-MM-DD"), "{err}");
    }

    #[test]
    fn months_are_parsed_by_our_own_from_str() {
        let err = parse(&["et", "summary", "--month", "2026-9"]).unwrap_err();
        assert!(err.to_string().contains("expected YYYY-MM"), "{err}");

        let cli = parse(&["et", "summary", "--month", "2026-09"]).unwrap();
        match cli.command {
            Command::Summary { month } => assert_eq!(month, Some("2026-09".parse().unwrap())),
            other => panic!("expected Summary, got {other:?}"),
        }
    }

    #[test]
    fn category_is_required_for_add() {
        let err = parse(&["et", "add", "42.50"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn list_filters_are_all_optional() {
        let cli = parse(&["et", "list"]).unwrap();
        match cli.command {
            Command::List {
                month,
                category,
                limit,
            } => {
                assert_eq!(month, None);
                assert_eq!(category, None);
                assert_eq!(limit, None);
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn delete_takes_a_positional_id_and_a_yes_flag() {
        let cli = parse(&["et", "delete", "17", "--yes"]).unwrap();
        match cli.command {
            Command::Delete { id, yes } => {
                assert_eq!(id, 17);
                assert!(yes);
            }
            other => panic!("expected Delete, got {other:?}"),
        }
    }

    #[test]
    fn the_yes_flag_defaults_to_false() {
        let cli = parse(&["et", "delete", "17"]).unwrap();
        match cli.command {
            Command::Delete { yes, .. } => assert!(!yes),
            other => panic!("expected Delete, got {other:?}"),
        }
    }

    #[test]
    fn the_db_flag_works_on_either_side_of_the_subcommand() {
        // That is what `global = true` buys, and it is easy to break by
        // accident, so it gets a test.
        let before = parse(&["et", "--db", "/tmp/a.db", "list"]).unwrap();
        let after = parse(&["et", "list", "--db", "/tmp/a.db"]).unwrap();

        assert_eq!(before.db, Some(PathBuf::from("/tmp/a.db")));
        assert_eq!(after.db, Some(PathBuf::from("/tmp/a.db")));
    }

    #[test]
    fn an_explicit_db_flag_beats_the_default() {
        let cli = parse(&["et", "--db", "/tmp/chosen.db", "list"]).unwrap();
        assert_eq!(cli.db_path().unwrap(), PathBuf::from("/tmp/chosen.db"));
    }

    #[test]
    fn a_bare_invocation_is_rejected() {
        // `arg_required_else_help` turns this into help rather than a terse
        // error, but either way it must not parse.
        let err = parse(&["et"]).unwrap_err();
        assert!(matches!(
            err.kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                | clap::error::ErrorKind::MissingSubcommand
        ));
    }

    #[test]
    fn unknown_subcommands_are_rejected() {
        let err = parse(&["et", "frobnicate"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn version_comes_from_cargo_toml() {
        let err = parse(&["et", "--version"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
        assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")));
    }
}
