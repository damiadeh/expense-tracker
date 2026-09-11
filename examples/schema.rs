//! Creates (or opens) the real database and prints what is inside it.
//!
//! The CLI does not exist until Stage 6, so this is how Stage 4 gets smoke
//! tested against a real file on disk rather than an in-memory database.
//!
//! Run it with:
//!
//! ```sh
//! cargo run --example schema
//! ET_DB_PATH=/tmp/et-scratch.db cargo run --example schema
//! ```
//!
//! Files in `examples/` are built by `cargo test` but shipped with nothing —
//! they are a good home for small throwaway programs like this one.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = expense_tracker::db::default_db_path()?;
    println!("database: {}", path.display());

    let conn = expense_tracker::db::open(&path)?;

    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    println!("schema version: {version}");

    let mut stmt = conn
        .prepare("SELECT sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY type DESC, name")?;
    let objects = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for object in objects {
        println!("\n{}", object?);
    }

    Ok(())
}
