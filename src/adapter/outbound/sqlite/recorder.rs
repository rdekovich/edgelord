//! SQLite statistics persistence.
//!
//! Records opportunities, trades, and daily aggregates for historical analysis.
//! Implements the [`StatsRecorder`](crate::port::outbound::stats::StatsRecorder)
//! trait for the statistics recording port.

// Diesel QueryableByName emits redundant field names in generated impls.
#![allow(clippy::redundant_field_names)]

use std::sync::Arc;

use chrono::{NaiveDate, Utc};
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::OptionalExtension;
use diesel::SqliteConnection;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal::Decimal;
use tracing::{debug, warn};

use crate::adapter::outbound::sqlite::database::connection::configure_sqlite_connection;
use crate::adapter::outbound::sqlite::database::model::{
    DailyStatsRow, NewOpportunityRow, NewTradeRow, OpportunityRow, StrategyDailyStatsRow, TradeRow,
};
use crate::adapter::outbound::sqlite::database::schema::{
    daily_stats, opportunities, strategy_daily_stats, trades,
};
use crate::domain::stats::{
    OpportunitySummary, RecordedOpportunity, StatsSummary, TradeCloseEvent, TradeOpenEvent,
};

/// Convert a decimal to f32 for storage.
fn decimal_to_f32(d: Decimal) -> f32 {
    d.to_f32().unwrap_or(0.0)
}

/// Convert f32 to Decimal for summary calculations.
#[must_use]
pub fn f32_to_decimal(f: f32) -> Decimal {
    Decimal::from_f32(f).unwrap_or(Decimal::ZERO)
}

/// SQLite-backed statistics recorder.
///
/// Records trading events and maintains daily aggregate statistics.
/// Implements the [`StatsRecorder`](crate::port::outbound::stats::StatsRecorder) trait.
pub struct SqliteRecorder {
    /// Database connection pool.
    pool: Pool<ConnectionManager<SqliteConnection>>,
}

// Diesel generates a redundant field initializer on recent compilers.
#[allow(clippy::redundant_field_names)]
#[derive(QueryableByName)]
struct LastInsertRowId {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    #[diesel(column_name = "id")]
    id: i32,
}

impl SqliteRecorder {
    /// Create a new statistics recorder with the given connection pool.
    #[must_use]
    pub fn new(pool: Pool<ConnectionManager<SqliteConnection>>) -> Self {
        Self { pool }
    }

    /// Record an opportunity detection event.
    ///
    /// Returns the database row ID if successful, or `None` on failure.
    pub fn record_opportunity(&self, event: &RecordedOpportunity) -> Option<i32> {
        let market_ids_json = serde_json::to_string(&event.market_ids).unwrap_or_default();
        let now = Utc::now().to_rfc3339();
        let today = Utc::now().date_naive().to_string();

        let row = NewOpportunityRow {
            strategy: event.strategy.clone(),
            market_ids: market_ids_json,
            edge: decimal_to_f32(event.edge),
            expected_profit: decimal_to_f32(event.expected_profit),
            detected_at: now,
            executed: i32::from(event.executed),
            rejected_reason: event.rejected_reason.clone(),
        };

        let mut conn = self.pool.get().ok()?;
        if let Err(e) = configure_sqlite_connection(&mut conn) {
            warn!(error = %e, "Failed to configure SQLite connection");
        }

        let id = conn.transaction(|conn| {
            diesel::insert_into(opportunities::table)
                .values(&row)
                .execute(conn)?;

            let id: i32 = diesel::sql_query("SELECT last_insert_rowid() AS id")
                .get_result::<LastInsertRowId>(conn)
                .map(|row| row.id)?;

            self.update_daily_stats_with_conn(conn, &today, &event.strategy, |daily, strategy| {
                daily.opportunities_detected += 1;
                strategy.opportunities_detected += 1;
                if event.executed {
                    daily.opportunities_executed += 1;
                    strategy.opportunities_executed += 1;
                } else if event.rejected_reason.is_some() {
                    daily.opportunities_rejected += 1;
                }
            })?;

            Ok::<i32, diesel::result::Error>(id)
        });

        match id {
            Ok(id) => {
                debug!(id = id, strategy = %event.strategy, "Recorded opportunity");
                Some(id)
            }
            Err(e) => {
                warn!(error = %e, "Opportunity transaction failed");
                None
            }
        }
    }

    /// Record a trade opening event.
    ///
    /// Returns the database row ID if successful, or `None` on failure.
    pub fn record_trade_open(&self, event: &TradeOpenEvent) -> Option<i32> {
        let market_ids_json = serde_json::to_string(&event.market_ids).unwrap_or_default();
        let legs_json = serde_json::to_string(&event.legs).unwrap_or_default();
        let now = Utc::now().to_rfc3339();
        let today = Utc::now().date_naive().to_string();

        let row = NewTradeRow {
            opportunity_id: event.opportunity_id,
            strategy: event.strategy.clone(),
            market_ids: market_ids_json,
            legs: legs_json,
            size: decimal_to_f32(event.size),
            expected_profit: decimal_to_f32(event.expected_profit),
            status: "open".to_string(),
            opened_at: now,
        };

        let mut conn = self.pool.get().ok()?;
        if let Err(e) = configure_sqlite_connection(&mut conn) {
            warn!(error = %e, "Failed to configure SQLite connection");
        }

        let id = conn.transaction(|conn| {
            diesel::insert_into(trades::table)
                .values(&row)
                .execute(conn)?;

            let id: i32 = diesel::sql_query("SELECT last_insert_rowid() AS id")
                .get_result::<LastInsertRowId>(conn)
                .map(|row| row.id)?;

            self.update_daily_stats_with_conn(conn, &today, &event.strategy, |daily, strategy| {
                daily.trades_opened += 1;
                daily.total_volume += decimal_to_f32(event.size);
                strategy.trades_opened += 1;
            })?;

            Ok::<i32, diesel::result::Error>(id)
        });

        match id {
            Ok(id) => {
                debug!(id = id, strategy = %event.strategy, "Recorded trade open");
                Some(id)
            }
            Err(e) => {
                warn!(error = %e, "Trade-open transaction failed");
                None
            }
        }
    }

    /// Record a trade closing event.
    pub fn record_trade_close(&self, event: &TradeCloseEvent) {
        let now = Utc::now().to_rfc3339();
        let today = Utc::now().date_naive().to_string();

        let mut conn = match self.pool.get() {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "Failed to get database connection");
                return;
            }
        };

        let result = conn.transaction::<(), diesel::result::Error, _>(|conn| {
            let trade: TradeRow = trades::table
                .filter(trades::id.eq(event.trade_id))
                .first(conn)?;
            // Settlement replay may repeat after a crash. Close and aggregates
            // must commit together, and each trade contributes exactly once.
            if trade.status == "closed" {
                return Ok(());
            }
            let profit = decimal_to_f32(event.realized_profit);
            let is_win = event.realized_profit > Decimal::ZERO;
            diesel::update(trades::table.filter(trades::id.eq(event.trade_id)))
                .set((
                    trades::realized_profit.eq(Some(profit)),
                    trades::status.eq("closed"),
                    trades::closed_at.eq(Some(&now)),
                    trades::close_reason.eq(Some(&event.reason)),
                ))
                .execute(conn)?;
            self.update_daily_stats_with_conn(conn, &today, &trade.strategy, |daily, strat| {
                daily.trades_closed += 1;
                strat.trades_closed += 1;
                if is_win {
                    daily.profit_realized += profit;
                    daily.win_count += 1;
                    strat.profit_realized += profit;
                    strat.win_count += 1;
                } else {
                    daily.loss_realized += profit.abs();
                    daily.loss_count += 1;
                    strat.loss_count += 1;
                }
            })
        });
        if let Err(e) = result {
            warn!(error = %e, "Trade-close transaction failed");
            return;
        }

        debug!(trade_id = event.trade_id, profit = %event.realized_profit, "Recorded trade close");
    }

    /// Record a latency measurement sample.
    pub fn record_latency(&self, latency_ms: u32) {
        let today = Utc::now().date_naive().to_string();
        self.update_daily_stats(&today, "", |daily, _| {
            daily.latency_sum_ms += latency_ms as i32;
            daily.latency_count += 1;
        });
    }

    /// Update peak exposure if the current value is higher than recorded.
    pub fn update_peak_exposure(&self, exposure: Decimal) {
        let today = Utc::now().date_naive().to_string();
        let exp = decimal_to_f32(exposure);
        self.update_daily_stats(&today, "", |daily, _| {
            if exp > daily.peak_exposure {
                daily.peak_exposure = exp;
            }
        });
    }

    /// Retrieve aggregated statistics for a date range.
    #[must_use]
    pub fn get_summary(&self, from: NaiveDate, to: NaiveDate) -> StatsSummary {
        let mut conn = match self.pool.get() {
            Ok(c) => c,
            Err(_) => return StatsSummary::default(),
        };

        let rows: Vec<DailyStatsRow> = daily_stats::table
            .filter(daily_stats::date.ge(from.to_string()))
            .filter(daily_stats::date.le(to.to_string()))
            .load(&mut conn)
            .unwrap_or_default();

        summary_from_rows(&rows)
    }

    /// Retrieve today's aggregated statistics.
    #[must_use]
    pub fn get_today(&self) -> StatsSummary {
        let today = Utc::now().date_naive();
        self.get_summary(today, today)
    }

    /// Prune old records while preserving aggregated daily statistics.
    pub fn prune_old_records(&self, retention_days: u32) {
        let cutoff = Utc::now().date_naive() - chrono::Duration::days(i64::from(retention_days));
        let cutoff_str = cutoff.to_string();

        let mut conn = match self.pool.get() {
            Ok(c) => c,
            Err(_) => return,
        };

        let opp_deleted =
            diesel::delete(opportunities::table.filter(opportunities::detected_at.lt(&cutoff_str)))
                .execute(&mut conn)
                .unwrap_or(0);

        let trades_deleted =
            diesel::delete(trades::table.filter(trades::opened_at.lt(&cutoff_str)))
                .execute(&mut conn)
                .unwrap_or(0);

        if opp_deleted > 0 || trades_deleted > 0 {
            debug!(
                opportunities = opp_deleted,
                trades = trades_deleted,
                "Pruned old records"
            );
        }
    }

    fn update_daily_stats<F>(&self, date: &str, strategy: &str, updater: F)
    where
        F: FnOnce(&mut DailyStatsRow, &mut StrategyDailyStatsRow),
    {
        if let Ok(mut conn) = self.pool.get() {
            let _ = self.update_daily_stats_with_conn(&mut conn, date, strategy, updater);
        }
    }

    fn update_daily_stats_with_conn<F>(
        &self,
        conn: &mut SqliteConnection,
        date: &str,
        strategy: &str,
        updater: F,
    ) -> Result<(), diesel::result::Error>
    where
        F: FnOnce(&mut DailyStatsRow, &mut StrategyDailyStatsRow),
    {
        let mut daily: DailyStatsRow = daily_stats::table
            .filter(daily_stats::date.eq(date))
            .first(conn)
            .optional()?
            .unwrap_or_else(|| DailyStatsRow {
                date: date.to_string(),
                ..Default::default()
            });

        let mut strat = if strategy.is_empty() {
            StrategyDailyStatsRow::default()
        } else {
            strategy_daily_stats::table
                .filter(strategy_daily_stats::date.eq(date))
                .filter(strategy_daily_stats::strategy.eq(strategy))
                .first(conn)
                .optional()?
                .unwrap_or_else(|| StrategyDailyStatsRow {
                    date: date.to_string(),
                    strategy: strategy.to_string(),
                    ..Default::default()
                })
        };

        updater(&mut daily, &mut strat);

        diesel::replace_into(daily_stats::table)
            .values(&daily)
            .execute(conn)?;

        if !strategy.is_empty() {
            diesel::replace_into(strategy_daily_stats::table)
                .values(&strat)
                .execute(conn)?;
        }

        Ok(())
    }
}

impl crate::port::outbound::stats::StatsRecorder for SqliteRecorder {
    fn record_opportunity(&self, event: &RecordedOpportunity) -> Option<i32> {
        SqliteRecorder::record_opportunity(self, event)
    }

    fn record_trade_open(&self, event: &TradeOpenEvent) -> Option<i32> {
        SqliteRecorder::record_trade_open(self, event)
    }

    fn record_trade_close(&self, event: &TradeCloseEvent) {
        SqliteRecorder::record_trade_close(self, event)
    }

    fn record_latency(&self, latency_ms: u32) {
        SqliteRecorder::record_latency(self, latency_ms)
    }

    fn update_peak_exposure(&self, exposure: Decimal) {
        SqliteRecorder::update_peak_exposure(self, exposure)
    }

    fn get_summary(&self, from: NaiveDate, to: NaiveDate) -> StatsSummary {
        SqliteRecorder::get_summary(self, from, to)
    }

    fn get_today(&self) -> StatsSummary {
        SqliteRecorder::get_today(self)
    }
}

/// Create a statistics recorder from a database connection pool.
#[must_use]
pub fn create_recorder(
    pool: Pool<ConnectionManager<SqliteConnection>>,
) -> Arc<dyn crate::port::outbound::stats::StatsRecorder> {
    Arc::new(SqliteRecorder::new(pool))
}

/// Export daily statistics to CSV format.
#[must_use]
pub fn export_daily_csv(
    pool: &Pool<ConnectionManager<SqliteConnection>>,
    from: NaiveDate,
    to: NaiveDate,
) -> String {
    let mut conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return String::new(),
    };

    let rows: Vec<DailyStatsRow> = daily_stats::table
        .filter(daily_stats::date.ge(from.to_string()))
        .filter(daily_stats::date.le(to.to_string()))
        .order(daily_stats::date.asc())
        .load(&mut conn)
        .unwrap_or_default();

    let mut csv = String::from(
        "date,opportunities_detected,opportunities_executed,trades_opened,trades_closed,profit,loss,net,win_count,loss_count,win_rate,volume,peak_exposure\n"
    );

    for row in rows {
        let net = row.profit_realized - row.loss_realized;
        let total = row.win_count + row.loss_count;
        let win_rate = if total > 0 {
            row.win_count as f32 / total as f32 * 100.0
        } else {
            0.0
        };
        csv.push_str(&format!(
            "{},{},{},{},{},{:.2},{:.2},{:.2},{},{},{:.1},{:.2},{:.2}\n",
            row.date,
            row.opportunities_detected,
            row.opportunities_executed,
            row.trades_opened,
            row.trades_closed,
            row.profit_realized,
            row.loss_realized,
            net,
            row.win_count,
            row.loss_count,
            win_rate,
            row.total_volume,
            row.peak_exposure
        ));
    }

    csv
}

/// Retrieve recent opportunity records.
#[must_use]
pub fn recent_opportunities(
    pool: &Pool<ConnectionManager<SqliteConnection>>,
    limit: i64,
) -> Vec<OpportunitySummary> {
    let mut conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let rows: Vec<OpportunityRow> = opportunities::table
        .order(opportunities::detected_at.desc())
        .limit(limit)
        .load(&mut conn)
        .unwrap_or_default();

    rows.into_iter()
        .map(|r| OpportunitySummary {
            id: r.id.unwrap_or(0),
            strategy: r.strategy,
            edge: f32_to_decimal(r.edge),
            expected_profit: f32_to_decimal(r.expected_profit),
            executed: r.executed != 0,
            rejected_reason: r.rejected_reason,
            detected_at: r.detected_at,
        })
        .collect()
}

/// Build a statistics summary from daily stats rows.
#[must_use]
pub fn summary_from_rows(rows: &[DailyStatsRow]) -> StatsSummary {
    let mut summary = StatsSummary::default();
    for row in rows {
        summary.opportunities_detected += i64::from(row.opportunities_detected);
        summary.opportunities_executed += i64::from(row.opportunities_executed);
        summary.opportunities_rejected += i64::from(row.opportunities_rejected);
        summary.trades_opened += i64::from(row.trades_opened);
        summary.trades_closed += i64::from(row.trades_closed);
        summary.profit_realized += f32_to_decimal(row.profit_realized);
        summary.loss_realized += f32_to_decimal(row.loss_realized);
        summary.win_count += i64::from(row.win_count);
        summary.loss_count += i64::from(row.loss_count);
        summary.total_volume += f32_to_decimal(row.total_volume);
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::stats::TradeLeg;
    use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
    use rust_decimal_macros::dec;

    pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

    /// Creates a test database pool with a unique temporary SQLite database.
    /// Each test gets its own database file to ensure isolation between parallel tests.
    fn setup_test_db() -> Pool<ConnectionManager<SqliteConnection>> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        // Generate unique database URL for each test
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let db_url = format!("file:test_db_{}?mode=memory&cache=shared", id);

        let manager = ConnectionManager::<SqliteConnection>::new(&db_url);
        let pool = Pool::builder()
            .max_size(5)
            .build(manager)
            .expect("Failed to create pool");
        let mut conn = pool.get().expect("Failed to get connection");
        conn.run_pending_migrations(MIGRATIONS)
            .expect("Failed to run migrations");
        pool
    }

    fn make_opportunity(strategy: &str, edge: Decimal, executed: bool) -> RecordedOpportunity {
        RecordedOpportunity {
            strategy: strategy.to_string(),
            market_ids: vec!["market-1".to_string(), "market-2".to_string()],
            edge,
            expected_profit: edge * dec!(100),
            executed,
            rejected_reason: if executed {
                None
            } else {
                Some("risk_limit".to_string())
            },
        }
    }

    fn make_trade_open(opportunity_id: i32, strategy: &str, size: Decimal) -> TradeOpenEvent {
        TradeOpenEvent {
            opportunity_id,
            strategy: strategy.to_string(),
            market_ids: vec!["market-1".to_string()],
            legs: vec![TradeLeg {
                token_id: "token-1".to_string(),
                side: "buy".to_string(),
                price: dec!(0.50),
                size,
            }],
            size,
            expected_profit: size * dec!(0.05),
        }
    }

    fn make_trade_close(trade_id: i32, profit: Decimal) -> TradeCloseEvent {
        TradeCloseEvent {
            trade_id,
            realized_profit: profit,
            reason: "market_settled".to_string(),
        }
    }

    // -------------------------------------------------------------------------
    // Basic CRUD operations
    // -------------------------------------------------------------------------

    #[test]
    fn record_opportunity_returns_id() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool);

        let opportunity = make_opportunity("single_condition", dec!(0.05), true);
        let id = recorder.record_opportunity(&opportunity);

        assert!(id.is_some());
        assert!(id.unwrap() > 0);
    }

    #[test]
    fn record_multiple_opportunities_increments_ids() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool);

        let opp1 = make_opportunity("strat1", dec!(0.05), true);
        let opp2 = make_opportunity("strat2", dec!(0.10), false);

        let id1 = recorder.record_opportunity(&opp1).unwrap();
        let id2 = recorder.record_opportunity(&opp2).unwrap();

        assert_eq!(id2, id1 + 1);
    }

    #[test]
    fn record_trade_open_returns_id() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool);

        // First create an opportunity
        let opportunity = make_opportunity("single_condition", dec!(0.05), true);
        let opp_id = recorder.record_opportunity(&opportunity).unwrap();

        // Then open a trade
        let trade = make_trade_open(opp_id, "single_condition", dec!(100));
        let trade_id = recorder.record_trade_open(&trade);

        assert!(trade_id.is_some());
        assert!(trade_id.unwrap() > 0);
    }

    #[test]
    fn record_trade_close_updates_trade() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool.clone());

        // Create opportunity and trade
        let opportunity = make_opportunity("single_condition", dec!(0.05), true);
        let opp_id = recorder.record_opportunity(&opportunity).unwrap();
        let trade = make_trade_open(opp_id, "single_condition", dec!(100));
        let trade_id = recorder.record_trade_open(&trade).unwrap();

        // Close the trade with profit
        let close_event = make_trade_close(trade_id, dec!(5.50));
        recorder.record_trade_close(&close_event);

        // Verify trade status updated
        let mut conn = pool.get().unwrap();
        let trade_row: TradeRow = trades::table
            .filter(trades::id.eq(trade_id))
            .first(&mut conn)
            .unwrap();

        assert_eq!(trade_row.status, "closed");
        assert!(trade_row.realized_profit.is_some());
        assert!((trade_row.realized_profit.unwrap() - 5.50).abs() < 0.01);
        assert!(trade_row.closed_at.is_some());
        assert_eq!(trade_row.close_reason, Some("market_settled".to_string()));
    }

    // -------------------------------------------------------------------------
    // Daily stats aggregation
    // -------------------------------------------------------------------------

    #[test]
    fn record_opportunity_updates_daily_stats() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool.clone());

        let opp = make_opportunity("single_condition", dec!(0.05), true);
        recorder.record_opportunity(&opp);

        let summary = recorder.get_today();

        assert_eq!(summary.opportunities_detected, 1);
        assert_eq!(summary.opportunities_executed, 1);
        assert_eq!(summary.opportunities_rejected, 0);
    }

    #[test]
    fn record_rejected_opportunity_updates_rejected_count() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool);

        let opp = make_opportunity("single_condition", dec!(0.05), false);
        recorder.record_opportunity(&opp);

        let summary = recorder.get_today();

        assert_eq!(summary.opportunities_detected, 1);
        assert_eq!(summary.opportunities_executed, 0);
        assert_eq!(summary.opportunities_rejected, 1);
    }

    #[test]
    fn record_trade_open_updates_volume_and_count() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool);

        let opp = make_opportunity("single_condition", dec!(0.05), true);
        let opp_id = recorder.record_opportunity(&opp).unwrap();
        let trade = make_trade_open(opp_id, "single_condition", dec!(100));
        recorder.record_trade_open(&trade);

        let summary = recorder.get_today();

        assert_eq!(summary.trades_opened, 1);
        assert_eq!(summary.total_volume, dec!(100));
    }

    #[test]
    fn record_trade_close_updates_profit_and_win_count() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool);

        let opp = make_opportunity("single_condition", dec!(0.05), true);
        let opp_id = recorder.record_opportunity(&opp).unwrap();
        let trade = make_trade_open(opp_id, "single_condition", dec!(100));
        let trade_id = recorder.record_trade_open(&trade).unwrap();

        let close = make_trade_close(trade_id, dec!(10.00));
        recorder.record_trade_close(&close);
        recorder.record_trade_close(&close); // Restart replay must be idempotent.

        let summary = recorder.get_today();

        assert_eq!(summary.trades_closed, 1);
        assert_eq!(summary.win_count, 1);
        assert_eq!(summary.loss_count, 0);
        assert_eq!(summary.profit_realized, dec!(10));
    }

    #[test]
    fn record_losing_trade_updates_loss_count() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool);

        let opp = make_opportunity("single_condition", dec!(0.05), true);
        let opp_id = recorder.record_opportunity(&opp).unwrap();
        let trade = make_trade_open(opp_id, "single_condition", dec!(100));
        let trade_id = recorder.record_trade_open(&trade).unwrap();

        let close = make_trade_close(trade_id, dec!(-5.00));
        recorder.record_trade_close(&close);
        recorder.record_trade_close(&close); // Restart replay must be idempotent.

        let summary = recorder.get_today();

        assert_eq!(summary.loss_count, 1);
        assert_eq!(summary.win_count, 0);
        // Loss is recorded as positive in loss_realized
        assert_eq!(summary.loss_realized, dec!(5));
    }

    // -------------------------------------------------------------------------
    // Strategy-level stats
    // -------------------------------------------------------------------------

    #[test]
    fn record_opportunities_updates_strategy_daily_stats() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool.clone());

        let opp1 = make_opportunity("strat_a", dec!(0.05), true);
        let opp2 = make_opportunity("strat_b", dec!(0.10), true);
        let opp3 = make_opportunity("strat_a", dec!(0.08), false);

        recorder.record_opportunity(&opp1);
        recorder.record_opportunity(&opp2);
        recorder.record_opportunity(&opp3);

        // Check strategy_daily_stats table directly
        let today = Utc::now().date_naive().to_string();
        let mut conn = pool.get().unwrap();

        let strat_a: StrategyDailyStatsRow = strategy_daily_stats::table
            .filter(strategy_daily_stats::date.eq(&today))
            .filter(strategy_daily_stats::strategy.eq("strat_a"))
            .first(&mut conn)
            .unwrap();

        assert_eq!(strat_a.opportunities_detected, 2);
        assert_eq!(strat_a.opportunities_executed, 1);

        let strat_b: StrategyDailyStatsRow = strategy_daily_stats::table
            .filter(strategy_daily_stats::date.eq(&today))
            .filter(strategy_daily_stats::strategy.eq("strat_b"))
            .first(&mut conn)
            .unwrap();

        assert_eq!(strat_b.opportunities_detected, 1);
        assert_eq!(strat_b.opportunities_executed, 1);
    }

    // -------------------------------------------------------------------------
    // Date range queries
    // -------------------------------------------------------------------------

    #[test]
    fn get_summary_aggregates_across_date_range() {
        let pool = setup_test_db();

        // Insert stats for multiple days directly using the same pool
        {
            let mut conn = pool.get().unwrap();
            let day1 = DailyStatsRow {
                date: "2026-01-01".to_string(),
                opportunities_detected: 10,
                opportunities_executed: 5,
                trades_opened: 3,
                win_count: 2,
                profit_realized: 50.0,
                ..Default::default()
            };
            let day2 = DailyStatsRow {
                date: "2026-01-02".to_string(),
                opportunities_detected: 15,
                opportunities_executed: 8,
                trades_opened: 6,
                win_count: 4,
                profit_realized: 75.0,
                ..Default::default()
            };

            diesel::insert_into(daily_stats::table)
                .values(&day1)
                .execute(&mut conn)
                .unwrap();
            diesel::insert_into(daily_stats::table)
                .values(&day2)
                .execute(&mut conn)
                .unwrap();
        }

        // Use the same pool for the recorder
        let recorder = SqliteRecorder::new(pool);
        let from = NaiveDate::parse_from_str("2026-01-01", "%Y-%m-%d").unwrap();
        let to = NaiveDate::parse_from_str("2026-01-02", "%Y-%m-%d").unwrap();

        let summary = recorder.get_summary(from, to);

        assert_eq!(summary.opportunities_detected, 25);
        assert_eq!(summary.opportunities_executed, 13);
        assert_eq!(summary.trades_opened, 9);
        assert_eq!(summary.win_count, 6);
        assert_eq!(summary.profit_realized, dec!(125));
    }

    #[test]
    fn get_summary_returns_empty_for_no_data() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool);

        let from = NaiveDate::parse_from_str("2025-01-01", "%Y-%m-%d").unwrap();
        let to = NaiveDate::parse_from_str("2025-01-31", "%Y-%m-%d").unwrap();

        let summary = recorder.get_summary(from, to);

        assert_eq!(summary.opportunities_detected, 0);
        assert_eq!(summary.trades_opened, 0);
        assert_eq!(summary.profit_realized, Decimal::ZERO);
    }

    #[test]
    fn get_summary_filters_by_date_range() {
        let pool = setup_test_db();

        // Insert stats for three days using the same pool
        {
            let mut conn = pool.get().unwrap();
            for (date, opp_count) in [("2026-01-01", 10), ("2026-01-02", 20), ("2026-01-03", 30)] {
                let row = DailyStatsRow {
                    date: date.to_string(),
                    opportunities_detected: opp_count,
                    ..Default::default()
                };
                diesel::insert_into(daily_stats::table)
                    .values(&row)
                    .execute(&mut conn)
                    .unwrap();
            }
        }

        // Query middle day only using the same pool
        let recorder = SqliteRecorder::new(pool);
        let from = NaiveDate::parse_from_str("2026-01-02", "%Y-%m-%d").unwrap();
        let to = NaiveDate::parse_from_str("2026-01-02", "%Y-%m-%d").unwrap();

        let summary = recorder.get_summary(from, to);

        assert_eq!(summary.opportunities_detected, 20);
    }

    // -------------------------------------------------------------------------
    // Latency and exposure tracking
    // -------------------------------------------------------------------------

    #[test]
    fn record_latency_updates_daily_stats() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool.clone());

        recorder.record_latency(50);
        recorder.record_latency(100);
        recorder.record_latency(75);

        let today = Utc::now().date_naive().to_string();
        let mut conn = pool.get().unwrap();
        let row: DailyStatsRow = daily_stats::table
            .filter(daily_stats::date.eq(&today))
            .first(&mut conn)
            .unwrap();

        assert_eq!(row.latency_count, 3);
        assert_eq!(row.latency_sum_ms, 225);
    }

    #[test]
    fn update_peak_exposure_tracks_maximum() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool.clone());

        recorder.update_peak_exposure(dec!(100));
        recorder.update_peak_exposure(dec!(500));
        recorder.update_peak_exposure(dec!(250));

        let today = Utc::now().date_naive().to_string();
        let mut conn = pool.get().unwrap();
        let row: DailyStatsRow = daily_stats::table
            .filter(daily_stats::date.eq(&today))
            .first(&mut conn)
            .unwrap();

        assert!((row.peak_exposure - 500.0).abs() < 0.01);
    }

    // -------------------------------------------------------------------------
    // Pruning old records
    // -------------------------------------------------------------------------

    #[test]
    fn prune_old_records_deletes_old_opportunities() {
        let pool = setup_test_db();

        // Insert old opportunity first
        {
            let mut conn = pool.get().unwrap();
            let old_opp = NewOpportunityRow {
                strategy: "test".to_string(),
                market_ids: "[]".to_string(),
                edge: 0.05,
                expected_profit: 5.0,
                detected_at: "2020-01-01T00:00:00Z".to_string(), // Old date
                executed: 1,
                rejected_reason: None,
            };
            diesel::insert_into(opportunities::table)
                .values(&old_opp)
                .execute(&mut conn)
                .unwrap();
        }

        // Use recorder for the rest of the operations
        let recorder = SqliteRecorder::new(pool.clone());

        // Insert recent opportunity
        let new_opp = make_opportunity("test", dec!(0.05), true);
        recorder.record_opportunity(&new_opp);

        // Count before prune
        {
            let mut conn = pool.get().unwrap();
            let count_before: i64 = opportunities::table.count().get_result(&mut conn).unwrap();
            assert_eq!(count_before, 2);
        }

        // Prune with 30 day retention
        recorder.prune_old_records(30);

        // Count after prune
        {
            let mut conn = pool.get().unwrap();
            let count_after: i64 = opportunities::table.count().get_result(&mut conn).unwrap();
            assert_eq!(count_after, 1);
        }
    }

    // -------------------------------------------------------------------------
    // CSV export
    // -------------------------------------------------------------------------

    #[test]
    fn export_daily_csv_returns_header_and_data() {
        let pool = setup_test_db();

        // Insert data in a scope so connection is released before export_daily_csv
        {
            let mut conn = pool.get().unwrap();
            let row = DailyStatsRow {
                date: "2026-01-15".to_string(),
                opportunities_detected: 100,
                opportunities_executed: 50,
                trades_opened: 20,
                trades_closed: 18,
                profit_realized: 150.0,
                loss_realized: 25.0,
                win_count: 15,
                loss_count: 3,
                total_volume: 5000.0,
                peak_exposure: 1000.0,
                ..Default::default()
            };
            diesel::insert_into(daily_stats::table)
                .values(&row)
                .execute(&mut conn)
                .unwrap();
        }

        let from = NaiveDate::parse_from_str("2026-01-01", "%Y-%m-%d").unwrap();
        let to = NaiveDate::parse_from_str("2026-01-31", "%Y-%m-%d").unwrap();

        let csv = export_daily_csv(&pool, from, to);

        assert!(csv.contains("date,opportunities_detected"));
        assert!(csv.contains("2026-01-15,100,50,20,18,150.00,25.00"));
    }

    #[test]
    fn export_daily_csv_empty_for_no_data() {
        let pool = setup_test_db();

        let from = NaiveDate::parse_from_str("2026-01-01", "%Y-%m-%d").unwrap();
        let to = NaiveDate::parse_from_str("2026-01-31", "%Y-%m-%d").unwrap();

        let csv = export_daily_csv(&pool, from, to);

        // Should have header only
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("date,opportunities"));
    }

    // -------------------------------------------------------------------------
    // Recent opportunities query
    // -------------------------------------------------------------------------

    #[test]
    fn recent_opportunities_returns_most_recent() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool.clone());

        // Record several opportunities
        for i in 0..5 {
            let opp = RecordedOpportunity {
                strategy: format!("strat_{}", i),
                market_ids: vec!["m1".to_string()],
                edge: dec!(0.01) * Decimal::from(i),
                expected_profit: dec!(1.0),
                executed: true,
                rejected_reason: None,
            };
            recorder.record_opportunity(&opp);
        }

        let recent = recent_opportunities(&pool, 3);

        assert_eq!(recent.len(), 3);
        // Most recent should be first
        assert_eq!(recent[0].strategy, "strat_4");
        assert_eq!(recent[1].strategy, "strat_3");
        assert_eq!(recent[2].strategy, "strat_2");
    }

    #[test]
    fn recent_opportunities_returns_empty_for_no_data() {
        let pool = setup_test_db();

        let recent = recent_opportunities(&pool, 10);

        assert!(recent.is_empty());
    }

    // -------------------------------------------------------------------------
    // Edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn record_opportunity_with_empty_market_ids() {
        let pool = setup_test_db();
        let recorder = SqliteRecorder::new(pool);

        let opp = RecordedOpportunity {
            strategy: "test".to_string(),
            market_ids: vec![],
            edge: dec!(0.05),
            expected_profit: dec!(5.0),
            executed: true,
            rejected_reason: None,
        };

        let id = recorder.record_opportunity(&opp);

        assert!(id.is_some());
    }

    #[test]
    fn summary_from_rows_handles_empty_slice() {
        let summary = summary_from_rows(&[]);

        assert_eq!(summary.opportunities_detected, 0);
        assert_eq!(summary.profit_realized, Decimal::ZERO);
    }

    #[test]
    fn f32_to_decimal_handles_special_values() {
        // Normal value
        let d = f32_to_decimal(123.45);
        assert!(d > dec!(123) && d < dec!(124));

        // Zero
        let d = f32_to_decimal(0.0);
        assert_eq!(d, Decimal::ZERO);

        // Negative
        let d = f32_to_decimal(-50.0);
        assert!(d < Decimal::ZERO);
    }

    #[test]
    fn decimal_to_f32_handles_special_values() {
        assert!((decimal_to_f32(dec!(123.45)) - 123.45).abs() < 0.01);
        assert!((decimal_to_f32(Decimal::ZERO) - 0.0).abs() < 0.01);
        assert!((decimal_to_f32(dec!(-50.0)) - (-50.0)).abs() < 0.01);
    }
}
