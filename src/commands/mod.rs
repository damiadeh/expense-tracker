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
pub mod delete;
pub mod list;
pub mod summary;

use crate::models::Expense;

/// One expense, written the way every command refers to one.
///
/// `add` says "Added {this}", `delete` asks "Delete {this}?" and then says
/// "Deleted {this}". Sharing the middle means the three can never drift into
/// describing the same row three different ways.
pub fn describe(expense: &Expense) -> String {
    let mut text = format!("#{}: {} {}", expense.id, expense.amount, expense.category);

    if let Some(note) = &expense.note {
        text.push_str(&format!(" — {note}"));
    }

    text.push_str(&format!(" ({})", expense.date));
    text
}
