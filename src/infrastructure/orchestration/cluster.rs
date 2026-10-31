//! Cluster detection runtime wiring.

use std::sync::Arc;

use tracing::info;

use crate::application::cache::book::BookCache;
use crate::application::cache::cluster::ClusterCache;
use crate::application::cluster::service::{ClusterDetectionHandle, ClusterDetectionService};
use crate::domain::market::MarketRegistry;
use crate::infrastructure::bootstrap::build_projection_solver;
use crate::infrastructure::config::settings::Config;
use crate::port::outbound::notifier::{Event, NotifierRegistry, OpportunityEvent};

/// Build book cache and optionally start cluster detection service.
pub(crate) fn setup_cluster_detection(
    config: &Config,
    registry: Arc<MarketRegistry>,
    cluster_cache: Arc<ClusterCache>,
    notifiers: Arc<NotifierRegistry>,
) -> (Arc<BookCache>, Option<ClusterDetectionHandle>) {
    if config.cluster_detection.enabled {
        let (cache, update_rx) =
            BookCache::with_notifications(config.cluster_detection.channel_capacity);
        let cache = Arc::new(cache);

        let service = ClusterDetectionService::new(
            config.cluster_detection.to_core_config(),
            Arc::clone(&cache),
            Arc::clone(&cluster_cache),
            Arc::clone(&registry),
            build_projection_solver(),
        );
        let (handle, mut opp_rx) = service.start(update_rx);

        tokio::spawn(async move {
            while let Some(opp) = opp_rx.recv().await {
                info!(
                    cluster = %opp.cluster_id,
                    gap = %opp.gap,
                    markets = ?opp.markets.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
                    "Cluster opportunity detected"
                );
                let mut candidate = OpportunityEvent::from(&opp.opportunity);
                candidate.question = format!(
                    "Unverified relationship candidate (not executable): {}",
                    candidate.question
                );
                let event = Event::OpportunityDetected(candidate);
                notifiers.notify_all(event);
            }
        });

        info!("Cluster detection service started");
        (cache, Some(handle))
    } else {
        (Arc::new(BookCache::new()), None)
    }
}
