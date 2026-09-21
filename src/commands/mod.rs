//! One module per subcommand.
//!
//! Each command function takes an open connection plus already-parsed
//! arguments, does its work, and **returns** what happened rather than printing
//! it. Rendering lives in a separate function alongside it.
//!
//! That split is what makes these testable without capturing stdout: a test
//! calls `run` and inspects the returned value, or calls the formatter with a
//! value it built itself. Nothing here ever calls `println!`.

pub mod add;
pub mod list;
