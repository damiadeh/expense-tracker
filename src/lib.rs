//! Expense tracker library.
//!
//! The binary in `src/main.rs` is deliberately thin — all the real logic lives
//! here so that integration tests in `tests/` can exercise it directly.

pub mod db;
pub mod error;
pub mod models;
pub mod money;

// Re-export the types callers reach for most, so they can write
// `use expense_tracker::Money;` instead of `expense_tracker::money::Money`.
pub use error::{AppError, Result};
pub use models::{Expense, Month, NewExpense};
pub use money::{Money, ParseMoneyError};
