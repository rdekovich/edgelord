use std::path::Path;

use crate::adapter::inbound::cli::{operator, output};
use crate::error::{Error, Result};

/// Validate readiness for live trading.
pub fn execute_live<P: AsRef<Path>>(config_path: P) -> Result<()> {
    let config_toml = operator::read_config_toml(config_path.as_ref())?;
    let report = operator::operator().check_live_readiness(&config_toml)?;

    output::section("Live Readiness");
    output::field("Exchange", &report.exchange);
    output::field("Environment", &report.environment);
    output::field("Chain ID", report.chain_id);
    output::field("Dry run", report.dry_run);

    if let Some(reason) = &report.protocol_blocker {
        output::warning(reason);
    }
    if !report.environment_is_mainnet {
        output::warning("Environment is not mainnet (expected mainnet)");
    }
    if !report.chain_is_polygon_mainnet {
        output::warning("Chain ID is not Polygon mainnet (expected 137)");
    }
    if !report.wallet_configured {
        output::warning("Wallet not configured (set WALLET_PRIVATE_KEY or keystore)");
    }
    if report.dry_run {
        output::warning("Dry run is enabled (set dry_run=false for live trading)");
    }

    if report.is_ready() {
        output::success("Ready for live trading");
        Ok(())
    } else {
        output::error("Live readiness check failed");
        Err(Error::Connection("live readiness check failed".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use crate::port::inbound::operator::diagnostic::LiveReadinessReport;

    #[test]
    fn retired_protocol_blocks_otherwise_valid_configuration() {
        let report = LiveReadinessReport {
            protocol_blocker: Some("Protocol retired".into()),
            exchange: "polymarket".into(),
            environment: "mainnet".into(),
            chain_id: 137,
            dry_run: false,
            environment_is_mainnet: true,
            chain_is_polygon_mainnet: true,
            wallet_configured: true,
        };
        assert!(!report.is_ready());
    }

    // Tests for LiveReadinessReport.is_ready()

    #[test]
    fn test_live_readiness_all_conditions_met() {
        let report = LiveReadinessReport {
            protocol_blocker: None,
            exchange: "polymarket".to_string(),
            environment: "mainnet".to_string(),
            chain_id: 137,
            dry_run: false,
            environment_is_mainnet: true,
            chain_is_polygon_mainnet: true,
            wallet_configured: true,
        };
        assert!(report.is_ready());
    }

    #[test]
    fn test_live_readiness_environment_not_mainnet() {
        let report = LiveReadinessReport {
            protocol_blocker: None,
            exchange: "polymarket".to_string(),
            environment: "testnet".to_string(),
            chain_id: 137,
            dry_run: false,
            environment_is_mainnet: false,
            chain_is_polygon_mainnet: true,
            wallet_configured: true,
        };
        assert!(!report.is_ready());
    }

    #[test]
    fn test_live_readiness_wrong_chain_id() {
        let report = LiveReadinessReport {
            protocol_blocker: None,
            exchange: "polymarket".to_string(),
            environment: "mainnet".to_string(),
            chain_id: 80002, // Amoy testnet
            dry_run: false,
            environment_is_mainnet: true,
            chain_is_polygon_mainnet: false,
            wallet_configured: true,
        };
        assert!(!report.is_ready());
    }

    #[test]
    fn test_live_readiness_wallet_not_configured() {
        let report = LiveReadinessReport {
            protocol_blocker: None,
            exchange: "polymarket".to_string(),
            environment: "mainnet".to_string(),
            chain_id: 137,
            dry_run: false,
            environment_is_mainnet: true,
            chain_is_polygon_mainnet: true,
            wallet_configured: false,
        };
        assert!(!report.is_ready());
    }

    #[test]
    fn test_live_readiness_dry_run_enabled() {
        let report = LiveReadinessReport {
            protocol_blocker: None,
            exchange: "polymarket".to_string(),
            environment: "mainnet".to_string(),
            chain_id: 137,
            dry_run: true,
            environment_is_mainnet: true,
            chain_is_polygon_mainnet: true,
            wallet_configured: true,
        };
        assert!(!report.is_ready());
    }

    #[test]
    fn test_live_readiness_multiple_failures() {
        let report = LiveReadinessReport {
            protocol_blocker: None,
            exchange: "polymarket".to_string(),
            environment: "testnet".to_string(),
            chain_id: 80002,
            dry_run: true,
            environment_is_mainnet: false,
            chain_is_polygon_mainnet: false,
            wallet_configured: false,
        };
        assert!(!report.is_ready());
    }

    #[test]
    fn test_live_readiness_only_dry_run_false_rest_failing() {
        let report = LiveReadinessReport {
            protocol_blocker: None,
            exchange: "polymarket".to_string(),
            environment: "testnet".to_string(),
            chain_id: 80002,
            dry_run: false,
            environment_is_mainnet: false,
            chain_is_polygon_mainnet: false,
            wallet_configured: false,
        };
        assert!(!report.is_ready());
    }

    // Tests for LiveReadinessReport clone and debug

    #[test]
    fn test_live_readiness_report_clone() {
        let report = LiveReadinessReport {
            protocol_blocker: None,
            exchange: "polymarket".to_string(),
            environment: "mainnet".to_string(),
            chain_id: 137,
            dry_run: false,
            environment_is_mainnet: true,
            chain_is_polygon_mainnet: true,
            wallet_configured: true,
        };
        let cloned = report.clone();
        assert_eq!(report.exchange, cloned.exchange);
        assert_eq!(report.environment, cloned.environment);
        assert_eq!(report.chain_id, cloned.chain_id);
        assert_eq!(report.dry_run, cloned.dry_run);
        assert_eq!(report.environment_is_mainnet, cloned.environment_is_mainnet);
        assert_eq!(
            report.chain_is_polygon_mainnet,
            cloned.chain_is_polygon_mainnet
        );
        assert_eq!(report.wallet_configured, cloned.wallet_configured);
    }

    #[test]
    fn test_live_readiness_report_debug() {
        let report = LiveReadinessReport {
            protocol_blocker: None,
            exchange: "polymarket".to_string(),
            environment: "mainnet".to_string(),
            chain_id: 137,
            dry_run: false,
            environment_is_mainnet: true,
            chain_is_polygon_mainnet: true,
            wallet_configured: true,
        };
        let debug_str = format!("{:?}", report);
        assert!(debug_str.contains("LiveReadinessReport"));
        assert!(debug_str.contains("polymarket"));
        assert!(debug_str.contains("mainnet"));
        assert!(debug_str.contains("137"));
    }

    // Tests for field values

    #[test]
    fn test_live_readiness_chain_id_values() {
        // Test with Polygon mainnet
        let mainnet = LiveReadinessReport {
            protocol_blocker: None,
            exchange: "polymarket".to_string(),
            environment: "mainnet".to_string(),
            chain_id: 137,
            dry_run: false,
            environment_is_mainnet: true,
            chain_is_polygon_mainnet: true,
            wallet_configured: true,
        };
        assert!(mainnet.is_ready());

        // Test with Amoy testnet
        let testnet = LiveReadinessReport {
            protocol_blocker: None,
            exchange: "polymarket".to_string(),
            environment: "testnet".to_string(),
            chain_id: 80002,
            dry_run: false,
            environment_is_mainnet: false,
            chain_is_polygon_mainnet: false,
            wallet_configured: true,
        };
        assert!(!testnet.is_ready());
    }

    #[test]
    fn test_live_readiness_empty_exchange() {
        let report = LiveReadinessReport {
            protocol_blocker: None,
            exchange: "".to_string(),
            environment: "mainnet".to_string(),
            chain_id: 137,
            dry_run: false,
            environment_is_mainnet: true,
            chain_is_polygon_mainnet: true,
            wallet_configured: true,
        };
        // Empty exchange should still be "ready" if all conditions met
        // (validation is done elsewhere)
        assert!(report.is_ready());
    }

    #[test]
    fn test_live_readiness_empty_environment() {
        let report = LiveReadinessReport {
            protocol_blocker: None,
            exchange: "polymarket".to_string(),
            environment: "".to_string(),
            chain_id: 137,
            dry_run: false,
            environment_is_mainnet: true,
            chain_is_polygon_mainnet: true,
            wallet_configured: true,
        };
        // Empty environment string doesn't affect is_ready if flags are set correctly
        assert!(report.is_ready());
    }

    // Tests for is_ready boundary conditions

    #[test]
    fn test_is_ready_just_one_condition_false() {
        // Test each condition being false individually
        let scenarios = [
            (false, true, true, false), // environment_is_mainnet = false
            (true, false, true, false), // chain_is_polygon_mainnet = false
            (true, true, false, false), // wallet_configured = false
        ];

        for (env_mainnet, chain_mainnet, wallet, dry_run) in scenarios {
            let report = LiveReadinessReport {
                protocol_blocker: None,
                exchange: "test".to_string(),
                environment: "test".to_string(),
                chain_id: 137,
                dry_run,
                environment_is_mainnet: env_mainnet,
                chain_is_polygon_mainnet: chain_mainnet,
                wallet_configured: wallet,
            };
            assert!(
                !report.is_ready(),
                "Report should not be ready when a condition is false"
            );
        }
    }
}
