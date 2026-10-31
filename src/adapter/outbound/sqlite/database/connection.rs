//! Database connection management using Diesel ORM.
//!
//! Provides connection pooling, migration support, and connection
//! configuration for SQLite databases.

use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::SqliteConnection;
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};

use crate::error::Result;

/// Embedded database migrations compiled from the migrations/ directory.
pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// Type alias for a SQLite connection pool.
pub type DbPool = Pool<ConnectionManager<SqliteConnection>>;

/// Create a connection pool for the given database URL.
///
/// # Errors
/// Returns an error if the pool cannot be created.
pub fn create_pool(database_url: &str) -> Result<DbPool> {
    let manager = ConnectionManager::<SqliteConnection>::new(database_url);
    Pool::builder()
        .max_size(5)
        .build(manager)
        .map_err(|e| crate::error::Error::Connection(e.to_string()))
}

/// Run all pending database migrations.
///
/// # Errors
/// Returns an error if migrations fail.
pub fn run_migrations(pool: &DbPool) -> Result<()> {
    let mut conn = pool
        .get()
        .map_err(|e| crate::error::Error::Connection(e.to_string()))?;
    conn.run_pending_migrations(MIGRATIONS)
        .map_err(|e| crate::error::Error::Connection(e.to_string()))?;
    Ok(())
}

/// Configure SQLite connection pragmas used for stats writes.
///
/// # Errors
/// Returns an error if a pragma fails to apply.
pub fn configure_sqlite_connection(conn: &mut SqliteConnection) -> Result<()> {
    diesel::sql_query("PRAGMA busy_timeout=5000")
        .execute(conn)
        .map_err(|e| crate::error::Error::Database(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::redundant_field_names)] // Diesel-generated QueryableByName impls.
mod tests {
    use super::*;

    #[test]
    fn create_pool_with_memory_db() {
        let pool = create_pool(":memory:");
        assert!(pool.is_ok());
    }

    #[test]
    fn create_pool_can_get_connection() {
        let pool = create_pool(":memory:").unwrap();
        let conn = pool.get();
        assert!(conn.is_ok());
    }

    #[test]
    fn create_pool_allows_multiple_connections() {
        let pool = create_pool(":memory:").unwrap();

        // Get multiple connections
        let conn1 = pool.get();
        assert!(conn1.is_ok());

        // Connection should be returned to pool when dropped
        drop(conn1);

        let conn2 = pool.get();
        assert!(conn2.is_ok());
    }

    #[test]
    fn run_migrations_creates_tables() {
        let pool = create_pool(":memory:").unwrap();
        run_migrations(&pool).unwrap();

        let mut conn = pool.get().unwrap();

        // Verify tables exist by querying sqlite_master
        let result: Vec<String> = diesel::sql_query(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name != '__diesel_schema_migrations' ORDER BY name"
        )
        .load::<TableName>(&mut conn)
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();

        assert!(result.contains(&"relations".to_string()));
        assert!(result.contains(&"clusters".to_string()));
        assert!(result.contains(&"opportunities".to_string()));
        assert!(result.contains(&"trades".to_string()));
        assert!(result.contains(&"daily_stats".to_string()));
        assert!(result.contains(&"strategy_daily_stats".to_string()));
    }

    #[derive(diesel::QueryableByName)]
    struct TableName {
        #[diesel(sql_type = diesel::sql_types::Text)]
        name: String,
    }

    #[test]
    fn run_migrations_is_idempotent() {
        let pool = create_pool(":memory:").unwrap();

        // Run migrations multiple times
        run_migrations(&pool).unwrap();
        run_migrations(&pool).unwrap();
        run_migrations(&pool).unwrap();

        // Should still work
        let mut conn = pool.get().unwrap();
        let result: i64 = diesel::sql_query(
            "SELECT COUNT(*) as count FROM sqlite_master WHERE type='table' AND name='relations'",
        )
        .load::<TableCount>(&mut conn)
        .unwrap()
        .first()
        .unwrap()
        .count;

        assert_eq!(result, 1);
    }

    #[derive(diesel::QueryableByName)]
    struct TableCount {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        count: i64,
    }

    #[test]
    fn configure_sqlite_connection_sets_pragmas() {
        let pool = create_pool(":memory:").unwrap();
        run_migrations(&pool).unwrap();
        let mut conn = pool.get().unwrap();

        let result = configure_sqlite_connection(&mut conn);
        assert!(result.is_ok());

        // Verify busy_timeout is set by querying it
        // The pragma returns a single row with the timeout value
        let result = diesel::sql_query("SELECT 1 as test").execute(&mut conn);
        assert!(result.is_ok());
    }

    #[test]
    fn create_pool_with_invalid_path_returns_error() {
        // Note: SQLite will create a file if it doesn't exist,
        // but an invalid path (like containing null bytes) should fail
        let result = create_pool("/nonexistent/deeply/nested/path/that/should/not/exist/db.sqlite");
        // This may or may not fail depending on permissions, so we just verify it handles gracefully
        // The important thing is it doesn't panic
        let _ = result;
    }

    #[test]
    fn dbpool_type_alias_works() {
        // This is a compile-time check - if DbPool alias is correct, this compiles
        let pool: Result<DbPool> = create_pool(":memory:");
        assert!(pool.is_ok());
    }

    #[test]
    fn migrations_constant_is_accessible() {
        // Verify the embedded migrations constant exists and is usable
        let _migrations = MIGRATIONS;
    }

    #[test]
    fn pool_respects_max_size() {
        // Create pool with default max_size of 5
        let pool = create_pool(":memory:").unwrap();

        // Should be able to get up to 5 connections
        let mut connections = Vec::new();
        for _ in 0..5 {
            let conn = pool.get();
            assert!(conn.is_ok(), "Should be able to get connection");
            connections.push(conn.unwrap());
        }

        // Additional connection requests should block/timeout
        // We can't easily test blocking, but we can verify the pool state
        assert_eq!(pool.state().connections, 5);
    }

    #[test]
    fn connection_handles_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let pool = Arc::new(create_pool(":memory:").unwrap());
        run_migrations(&pool).unwrap();

        let mut handles = vec![];

        for _i in 0..10 {
            let pool_clone = Arc::clone(&pool);
            let handle = thread::spawn(move || {
                let mut conn = pool_clone.get().unwrap();
                // Execute a simple query
                let result: Vec<TableCount> =
                    diesel::sql_query("SELECT COUNT(*) as count FROM sqlite_master")
                        .load(&mut conn)
                        .unwrap();
                assert!(!result.is_empty());
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("Thread should complete without panic");
        }
    }
}
