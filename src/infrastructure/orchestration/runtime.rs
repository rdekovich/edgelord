//! Orchestrator runtime lifecycle.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{info, warn};

use super::cluster;
use super::context::EventProcessingContext;
use super::inference;
use super::orchestrator::{process_market_event, Orchestrator};
use super::startup;
use super::stream;
use crate::application::position::manager::PositionManager;
use crate::application::risk::manager::RiskManager;
use crate::application::state::AppState;
use crate::error::Result;
use crate::infrastructure::bootstrap::{
    build_cluster_cache, build_inferrer, build_llm_client, build_notifier_registry,
    build_strategy_registry, init_stats_recorder,
};
use crate::infrastructure::config::settings::Config;
#[cfg(feature = "telegram")]
use crate::port::inbound::runtime::RuntimeClusterView;
use crate::port::outbound::inference::RelationInferrer;

impl Orchestrator {
    /// Run the main application loop.
    pub async fn run(config: Config) -> Result<()> {
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        super::runtime::run_with_shutdown(config, shutdown_rx).await
    }

    /// Run with externally controlled shutdown signal.
    pub async fn run_with_shutdown(config: Config, shutdown: watch::Receiver<bool>) -> Result<()> {
        super::runtime::run_with_shutdown(config, shutdown).await
    }
}

/// Runtime loop entrypoint used by [`Orchestrator`].
pub async fn run_with_shutdown(config: Config, mut shutdown: watch::Receiver<bool>) -> Result<()> {
    info!(exchange = ?config.exchange, dry_run = config.dry_run, "Starting edgelord");

    if !config.dry_run && config.wallet.private_key.is_some() {
        let enabled = |name: &str| {
            config
                .strategies
                .enabled
                .iter()
                .any(|s| s.trim().to_lowercase().replace('-', "_") == name)
        };
        if enabled("market_rebalancing") && config.strategies.exhaustive_groups.is_empty() {
            return Err(crate::error::Error::Parse("Live rebalancing requires strategies.exhaustive_groups verified against resolution rules".into()));
        }
        if enabled("combinatorial") && config.strategies.verified_relations.is_empty() {
            return Err(crate::error::Error::Parse("Live combinatorial trading requires strategies.verified_relations; LLM guesses are insufficient".into()));
        }
    }

    let state = if config.dry_run {
        Arc::new(AppState::new(config.risk.clone().into()))
    } else {
        let path = std::path::PathBuf::from(format!("{}.execution.jsonl", config.database));
        let journal =
            Arc::new(crate::adapter::outbound::journal::FileExecutionJournal::open(&path)?);
        Arc::new(AppState::with_journal(config.risk.clone().into(), journal)?)
    };
    let stats_recorder = init_stats_recorder(&config)?;
    // Repair reporting if settlement was durable before the SQL close committed.
    for position in state.positions().all() {
        if let (Some(trade_id), crate::domain::position::PositionStatus::Closed { pnl }) =
            (position.trade_id(), position.status())
        {
            stats_recorder.record_trade_close(&crate::domain::stats::TradeCloseEvent {
                trade_id,
                realized_profit: *pnl,
                reason: "settlement_replay".into(),
            });
        }
    }

    let position_manager = Arc::new(PositionManager::new(Arc::clone(&stats_recorder)));
    info!(database = %config.database, "Database initialized");

    let risk_manager = Arc::new(RiskManager::new(state.clone()));

    #[allow(unused_variables)]
    let (notifiers, runtime_stats) =
        build_notifier_registry(&config, Arc::clone(&state), Arc::clone(&stats_recorder));
    let notifiers = Arc::new(notifiers);
    info!(notifiers = notifiers.len(), "Notifiers initialized");

    let executor = if config.dry_run {
        None
    } else {
        crate::infrastructure::exchange::factory::ExchangeFactory::create_arbitrage_executor(
            &config,
        )
        .await?
    };
    if let Some(executor) = &executor {
        let identity = executor
            .account_identity()
            .ok_or_else(|| crate::error::Error::Parse("Executor has no account identity".into()))?;
        state.bind_account(&identity)?;
        let positions: Vec<_> = state.positions().all().cloned().collect();
        executor.verify_inventory(&positions).await?;
        if state.is_circuit_breaker_active() {
            return Err(crate::error::Error::Parse(
                "Execution ledger requires reconciliation; live trading disabled".into(),
            ));
        }
    }

    let cluster_cache = build_cluster_cache(&config);
    let llm_client = build_llm_client(&config);
    let inferrer: Option<Arc<dyn RelationInferrer>> =
        llm_client.map(|llm| build_inferrer(&config, llm));
    if inferrer.is_some() {
        info!("Inference service enabled");
    }

    #[cfg(feature = "telegram")]
    if let Some(ref stats) = runtime_stats {
        stats.set_cluster_cache(Arc::clone(&cluster_cache) as Arc<dyn RuntimeClusterView>);
    }

    let strategies = build_strategy_registry(&config, Arc::clone(&cluster_cache));
    let Some(prepared) = startup::prepare_markets(&config, strategies).await? else {
        return Ok(());
    };

    #[cfg(feature = "telegram")]
    if let Some(ref stats) = runtime_stats {
        stats.update_market_counts(prepared.registry.len(), prepared.token_ids.len());
    }

    inference::run_startup_inference(
        &config,
        inferrer.as_ref(),
        &prepared.market_summaries,
        cluster_cache.as_ref(),
        &notifiers,
    )
    .await;

    let (cache, cluster_handle) = cluster::setup_cluster_detection(
        &config,
        Arc::clone(&prepared.registry),
        Arc::clone(&cluster_cache),
        Arc::clone(&notifiers),
    );

    let _inference_handle = inference::start_continuous_inference(
        &config,
        inferrer,
        Arc::clone(&cluster_cache),
        Arc::new(prepared.market_summaries.clone()),
    );

    let mut data_stream = stream::create_connected_stream(&config, &prepared.token_ids).await?;
    info!("Listening for market events...");

    let dry_run = config.dry_run || executor.is_none();
    let stats_interval_secs = config.telegram.stats_interval_secs;
    let mut stats_interval =
        tokio::time::interval(std::time::Duration::from_secs(stats_interval_secs));
    stats_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut settlement_interval = tokio::time::interval(std::time::Duration::from_secs(60));
    settlement_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = settlement_interval.tick(), if executor.is_some() => {
                if let Some(executor) = &executor {
                    let positions: Vec<_> = state.positions().all().cloned().collect();
                    match tokio::time::timeout(std::time::Duration::from_secs(30), executor.settlement_payouts(&positions)).await {
                        Ok(Ok(payouts)) if !payouts.is_empty() => {
                            match state.settle_tokens(&payouts) {
                                Ok(closes) => for (trade_id, pnl) in closes {
                                    if let Some(trade_id) = trade_id { stats_recorder.record_trade_close(&crate::domain::stats::TradeCloseEvent { trade_id, realized_profit: pnl, reason: "resolution (redemption may remain pending)".into() }); }
                                },
                                Err(error) => state.require_reconciliation(format!("Settlement persistence failed: {error}")),
                            }
                        }
                        Ok(Ok(_)) => {},
                        _ => warn!("Settlement poll failed; inventory remains reserved"),
                    }
                }
            }
            result = shutdown.changed() => {
                match result {
                    Ok(_) => {
                        if *shutdown.borrow() {
                            info!("Shutdown signal received");
                            break;
                        }
                    }
                    Err(_) => {
                        info!("Shutdown channel closed");
                        break;
                    }
                }
            }
            _ = stats_interval.tick() => {
                #[cfg(feature = "telegram")]
                if let Some(ref stats) = runtime_stats {
                    if let Some(pool_stats) = data_stream.pool_stats() {
                        stats.update_pool_stats(pool_stats);
                    }
                }
            }
            event = data_stream.next_event() => {
                let Some(event) = event else {
                    warn!("Market data stream ended");
                    break;
                };
                process_market_event(
                    event,
                    EventProcessingContext {
                        cache: &cache,
                        registry: &prepared.registry,
                        strategies: prepared.strategies.as_ref(),
                        executor: executor.clone(),
                        risk_manager: &risk_manager,
                        notifiers: &notifiers,
                        state: &state,
                        stats: &stats_recorder,
                        position_manager: &position_manager,
                        dry_run,
                    },
                );
            }
        }
    }

    if let Some(handle) = cluster_handle {
        handle.shutdown().await;
    }

    Ok(())
}
