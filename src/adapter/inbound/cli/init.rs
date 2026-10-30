//! Interactive setup wizard.
//!
//! A comprehensive wizard that guides users through complete edgelord setup
//! including network selection, wallet creation, strategies, and risk limits.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use alloy_signer_local::PrivateKeySigner;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Password, Select};
use rand::rngs::OsRng;

use crate::adapter::inbound::cli::{output, paths};
use crate::error::{ConfigError, Result};

/// Default config template used by the setup wizard.
const CONFIG_TEMPLATE: &str = include_str!("../../../../config.toml.example");

/// Run the interactive setup wizard.
pub fn execute(path: PathBuf, force: bool) -> Result<()> {
    if output::is_json() {
        return Err(ConfigError::InvalidValue {
            field: "json",
            reason: "`edgelord init` is interactive; use `edgelord config init` for scripted setup"
                .to_string(),
        }
        .into());
    }

    output::header(env!("CARGO_PKG_VERSION"));
    println!();
    output::note("Welcome to edgelord. Let's get you set up.");
    println!();

    let theme = ColorfulTheme::default();

    // ─────────────────────────────────────────────────────────────────────────
    // Network
    // ─────────────────────────────────────────────────────────────────────────

    output::section("Network");

    let networks = &["Testnet (recommended for first run)", "Mainnet"];
    let network = Select::with_theme(&theme)
        .with_prompt("Select network")
        .items(networks)
        .default(0)
        .interact()?;

    let environment = if network == 0 { "testnet" } else { "mainnet" };
    let is_mainnet = network == 1;

    if is_mainnet {
        output::warning("Mainnet selected - real funds will be at risk");
        let confirm = Confirm::with_theme(&theme)
            .with_prompt("Continue with mainnet?")
            .default(false)
            .interact()?;
        if !confirm {
            output::note("Switching to testnet.");
            return execute(path, force);
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Wallet Setup
    // ─────────────────────────────────────────────────────────────────────────

    output::section("Wallet");

    let keystore_path = paths::keystore_dir().join("polymarket.json");

    let wallet_exists = keystore_path.exists();
    let (signer, keystore_created) =
        if wallet_exists {
            output::note(&format!(
                "Existing keystore found at {}",
                keystore_path.display()
            ));
            let use_existing = Confirm::with_theme(&theme)
                .with_prompt("Use existing wallet?")
                .default(true)
                .interact()?;

            if use_existing {
                let password = Password::with_theme(&theme)
                    .with_prompt("Keystore password")
                    .interact()?;

                let spinner = output::spinner("Unlocking wallet...");
                let signer = PrivateKeySigner::decrypt_keystore(&keystore_path, &password)
                    .map_err(|e| ConfigError::InvalidValue {
                        field: "keystore",
                        reason: e.to_string(),
                    })?;
                output::spinner_success(&spinner, "Wallet unlocked");

                (signer, false)
            } else {
                output::warning("Creating a new wallet will overwrite the existing keystore.");
                let confirm = Confirm::with_theme(&theme)
                    .with_prompt("Continue?")
                    .default(false)
                    .interact()?;
                if !confirm {
                    output::note("Setup aborted.");
                    return Ok(());
                }
                create_or_import_wallet(&theme, &keystore_path)?
            }
        } else {
            create_or_import_wallet(&theme, &keystore_path)?
        };

    output::field("Address", format!("{}", signer.address()));
    output::field("Keystore", keystore_path.display());

    if keystore_created {
        println!();
        output::warning("Save your keystore password securely. You'll need it to start edgelord.");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Strategies
    // ─────────────────────────────────────────────────────────────────────────

    output::section("Strategies");

    let single_condition = Confirm::with_theme(&theme)
        .with_prompt("single_condition: Buy YES + NO when sum < $1")
        .default(true)
        .interact()?;

    let rebalancing = Confirm::with_theme(&theme)
        .with_prompt("market_rebalancing: Requires reviewed exhaustive_groups")
        .default(false)
        .interact()?;

    let combinatorial = Confirm::with_theme(&theme)
        .with_prompt("combinatorial: Requires reviewed verified_relations")
        .default(false)
        .interact()?;

    let mut enabled_strategies = Vec::new();
    if single_condition {
        enabled_strategies.push("single_condition");
    }
    if rebalancing {
        enabled_strategies.push("market_rebalancing");
    }
    if combinatorial {
        enabled_strategies.push("combinatorial");
    }

    if enabled_strategies.is_empty() {
        output::warning("No strategies selected.");
        let proceed = Confirm::with_theme(&theme)
            .with_prompt("Continue anyway?")
            .default(false)
            .interact()?;
        if !proceed {
            output::note("Setup aborted.");
            return Ok(());
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Risk Limits
    // ─────────────────────────────────────────────────────────────────────────

    output::section("Risk Limits");

    let max_exposure: f64 = Input::with_theme(&theme)
        .with_prompt("Maximum total exposure ($)")
        .default(if is_mainnet { 100.0 } else { 500.0 })
        .interact()?;

    let max_position: f64 = Input::with_theme(&theme)
        .with_prompt("Maximum position per market ($)")
        .default(if is_mainnet { 25.0 } else { 100.0 })
        .interact()?;

    // ─────────────────────────────────────────────────────────────────────────
    // Optional Features
    // ─────────────────────────────────────────────────────────────────────────

    output::section("Notifications");

    let telegram = Confirm::with_theme(&theme)
        .with_prompt("Enable Telegram notifications?")
        .default(false)
        .interact()?;

    if telegram {
        output::note("Set TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID environment variables.");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Generate & Write Config
    // ─────────────────────────────────────────────────────────────────────────

    println!();
    let spinner = output::spinner("Writing configuration...");

    // Check if config file exists.
    if path.exists() && !force {
        output::spinner_fail(&spinner, "Config already exists");
        let overwrite = Confirm::with_theme(&theme)
            .with_prompt(format!("{} already exists. Overwrite?", path.display()))
            .default(false)
            .interact()?;
        if !overwrite {
            output::note("Setup aborted. Your wallet was still created.");
            return Ok(());
        }
    }

    let config = generate_config(
        environment,
        &enabled_strategies,
        max_exposure,
        max_position,
        combinatorial,
        telegram,
        &keystore_path,
    )?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, config)?;

    output::spinner_success(&spinner, "Configuration saved");

    // ─────────────────────────────────────────────────────────────────────────
    // Summary
    // ─────────────────────────────────────────────────────────────────────────

    output::section("Ready");

    output::success(&format!("Config    {}", path.display()));
    output::success(&format!("Keystore  {}", keystore_path.display()));
    output::success(&format!("Address   {}", signer.address()));
    output::success(&format!(
        "Network   {} (chain {})",
        environment,
        if is_mainnet { 137 } else { 80002 }
    ));

    println!();
    output::section("Next Steps");

    output::note("Setup always starts in dry-run mode; no orders will be submitted.");
    output::note(
        "Review docs/configuration.md and docs/deployment/operations.md before enabling execution.",
    );
    output::note("Multi-market strategies require verified groups/relations in the config file.");
    output::note("Live execution supports only Polygon chain 137; testnet is detection-only.");
    output::note("Start with: edgelord run --dry-run");

    Ok(())
}

/// Create or import a wallet, returning the signer and whether a new keystore was created.
fn create_or_import_wallet(
    theme: &ColorfulTheme,
    keystore_path: &Path,
) -> Result<(PrivateKeySigner, bool)> {
    let wallet_options = &["Generate a new wallet", "Import an existing private key"];
    let wallet_choice = Select::with_theme(theme)
        .with_prompt("Wallet setup")
        .items(wallet_options)
        .default(0)
        .interact()?;

    let password = Password::with_theme(theme)
        .with_prompt("Create keystore password")
        .with_confirmation("Confirm password", "Passwords don't match")
        .interact()?;

    if password.len() < 8 {
        output::warning("Password should be at least 8 characters for security.");
    }

    let parent = keystore_path
        .parent()
        .ok_or_else(|| ConfigError::InvalidValue {
            field: "keystore_path",
            reason: "missing parent directory".to_string(),
        })?;
    fs::create_dir_all(parent)?;

    let name = keystore_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| ConfigError::InvalidValue {
            field: "keystore_path",
            reason: "invalid file name".to_string(),
        })?;

    let signer = if wallet_choice == 0 {
        // Generate new wallet
        let spinner = output::spinner("Generating wallet...");

        let mut rng = OsRng;
        let (signer, _uuid) =
            PrivateKeySigner::new_keystore(parent, &mut rng, &password, Some(name)).map_err(
                |e| ConfigError::InvalidValue {
                    field: "keystore",
                    reason: e.to_string(),
                },
            )?;

        output::spinner_success(&spinner, "Wallet created");
        signer
    } else {
        // Import existing key
        let private_key: String = Input::with_theme(theme)
            .with_prompt("Private key (hex, with or without 0x)")
            .interact()?;

        let key = private_key.trim().trim_start_matches("0x");
        let signer = PrivateKeySigner::from_str(key).map_err(|e| ConfigError::InvalidValue {
            field: "private_key",
            reason: e.to_string(),
        })?;

        let spinner = output::spinner("Encrypting keystore...");

        let mut rng = OsRng;
        let (signer, _uuid) = PrivateKeySigner::encrypt_keystore(
            parent,
            &mut rng,
            signer.to_bytes(),
            &password,
            Some(name),
        )
        .map_err(|e| ConfigError::InvalidValue {
            field: "keystore",
            reason: e.to_string(),
        })?;

        output::spinner_success(&spinner, "Wallet imported");
        signer
    };

    Ok((signer, true))
}

fn generate_config(
    environment: &str,
    strategies: &[&str],
    max_exposure: f64,
    max_position: f64,
    enable_combinatorial: bool,
    enable_telegram: bool,
    keystore_path: &Path,
) -> Result<String> {
    let mut config: toml::Value = toml::from_str(CONFIG_TEMPLATE).map_err(ConfigError::Parse)?;
    let table = config.as_table_mut().ok_or_else(|| {
        ConfigError::Other("config template root must be a TOML table".to_string())
    })?;

    // General settings
    table.insert("dry_run".to_string(), toml::Value::Boolean(true));

    // Exchange config
    if let Some(exchange_config) = table
        .get_mut("exchange_config")
        .and_then(toml::Value::as_table_mut)
    {
        exchange_config.insert(
            "environment".to_string(),
            toml::Value::String(environment.to_string()),
        );
        exchange_config.insert(
            "chain_id".to_string(),
            toml::Value::Integer(if environment == "mainnet" { 137 } else { 80002 }),
        );
    }

    // Strategies
    if let Some(strategies_table) = table
        .get_mut("strategies")
        .and_then(toml::Value::as_table_mut)
    {
        strategies_table.insert(
            "enabled".to_string(),
            toml::Value::Array(
                strategies
                    .iter()
                    .map(|name| toml::Value::String((*name).to_string()))
                    .collect(),
            ),
        );
    }

    if let Some(combinatorial_table) = table
        .get_mut("strategies")
        .and_then(toml::Value::as_table_mut)
        .and_then(|strategies_table| {
            strategies_table
                .get_mut("combinatorial")
                .and_then(toml::Value::as_table_mut)
        })
    {
        combinatorial_table.insert(
            "enabled".to_string(),
            toml::Value::Boolean(enable_combinatorial),
        );
    }

    // Risk limits
    if let Some(risk_table) = table.get_mut("risk").and_then(toml::Value::as_table_mut) {
        risk_table.insert(
            "max_total_exposure".to_string(),
            toml::Value::Float(max_exposure),
        );
        risk_table.insert(
            "max_position_per_market".to_string(),
            toml::Value::Float(max_position),
        );
    }

    // Wallet keystore path
    let wallet = table
        .entry("wallet")
        .or_insert_with(|| toml::Value::Table(Default::default()));
    if let Some(wallet_table) = wallet.as_table_mut() {
        wallet_table.insert(
            "keystore_path".to_string(),
            toml::Value::String(keystore_path.to_string_lossy().to_string()),
        );
    }

    // Telegram
    if let Some(telegram_table) = table
        .get_mut("telegram")
        .and_then(toml::Value::as_table_mut)
    {
        telegram_table.insert("enabled".to_string(), toml::Value::Boolean(enable_telegram));
    }

    toml::to_string_pretty(&config).map_err(|error| ConfigError::Other(error.to_string()).into())
}

#[cfg(test)]
mod safety_tests {
    use super::*;

    #[test]
    fn mainnet_wizard_does_not_enable_order_submission() {
        let text = generate_config(
            "mainnet",
            &["single_condition"],
            100.0,
            10.0,
            false,
            false,
            Path::new("wallet.json"),
        )
        .unwrap();
        let config: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(config["dry_run"].as_bool(), Some(true));
        assert_eq!(
            config["exchange_config"]["chain_id"].as_integer(),
            Some(137)
        );
    }
}
