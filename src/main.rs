//! Reproducer: SIGSEGV on process exit with sqlx + bundled SQLCipher >= 4.7.
//!
//! Dropping an sqlx `SqlitePool` only signals the per-connection worker
//! threads to close their connections; it does not wait for them. If the
//! process exits right after, SQLCipher's exit cleanup (registered via
//! `atexit()` and platform static destructors) wipes and frees its global
//! private heap, which holds the codec contexts of the connections still
//! being closed on the worker threads. The worker thread then dereferences
//! wiped memory in `sqlite3FreeCodecArg` and crashes.
//!
//! Run in a loop and watch for exit code 139 (SIGSEGV):
//!
//! ```sh
//! cargo build --release
//! for i in $(seq 50); do ./target/release/sqlcipher-exit-race || echo "CRASH $i: $?"; done
//! ```

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;

    let mut pools = Vec::new();
    for i in 0..4 {
        let opts = SqliteConnectOptions::new()
            .filename(dir.path().join(format!("db-{i}.sqlite3")))
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .pragma("key", "'secret'");
        let pool = SqlitePool::connect_with(opts).await?;
        // Write through the codec; WAL keeps a checkpoint pending so that
        // closing the connection does real work, widening the race window.
        sqlx::query("CREATE TABLE IF NOT EXISTS t (x INTEGER)")
            .execute(&pool)
            .await?;
        for _ in 0..8 {
            sqlx::query("INSERT INTO t (x) VALUES (1)")
                .execute(&pool)
                .await?;
        }
        pools.push(pool);
    }

    // With --close, close the pools gracefully; this waits until all
    // connections are fully closed and the crash disappears.
    if std::env::args().any(|arg| arg == "--close") {
        for pool in &pools {
            pool.close().await;
        }
    }

    // Otherwise drop the pools and return immediately. The drop only signals
    // the sqlx worker threads to close their connections; process exit then
    // runs SQLCipher's atexit/static-destructor teardown while those threads
    // are still inside sqlite3Close.
    Ok(())
}
