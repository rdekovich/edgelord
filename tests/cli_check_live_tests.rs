use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn write_temp_config(contents: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("edgelord-check-live-{nanos}.toml"));
    fs::write(&path, contents).expect("write temp config");
    path
}

#[test]
fn check_live_warns_on_missing_wallet_or_mainnet() {
    let _guard = ENV_LOCK.lock().expect("env lock poisoned");

    let template = include_str!("../config.toml.example");
    let testnet = template
        .replace("chain_id = 137", "chain_id = 80002")
        .replace("environment = \"mainnet\"", "environment = \"testnet\"");
    let path = write_temp_config(&testnet);

    std::env::remove_var("WALLET_PRIVATE_KEY");
    std::env::remove_var("EDGELORD_KEYSTORE_PASSWORD");
    std::env::remove_var("EDGELORD_KEYSTORE_PASSWORD_FILE");

    let output = Command::new(env!("CARGO_BIN_EXE_edgelord"))
        .args(["check", "live", "--config"])
        .arg(&path)
        .output()
        .expect("run edgelord");

    let _ = fs::remove_file(&path);

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Check for warning content (ignoring ANSI escape codes around emoji)
    assert!(
        stdout.contains("Wallet not configured"),
        "expected wallet warning in stdout: {stdout}"
    );
    assert!(
        stdout.contains("Chain ID is not Polygon mainnet"),
        "expected chain id warning in stdout: {stdout}"
    );
    assert!(
        !output.status.success(),
        "expected nonzero exit when not ready"
    );
}

#[test]
fn check_live_fails_when_environment_is_testnet_even_with_mainnet_chain_id() {
    let _guard = ENV_LOCK.lock().expect("env lock poisoned");

    let template = include_str!("../config.toml.example");
    let testnet_chain137 = template
        .replace("environment = \"mainnet\"", "environment = \"testnet\"")
        .replace("dry_run = true", "dry_run = false");
    let path = write_temp_config(&testnet_chain137);

    std::env::set_var(
        "WALLET_PRIVATE_KEY",
        "1111111111111111111111111111111111111111111111111111111111111111",
    );
    std::env::remove_var("EDGELORD_KEYSTORE_PASSWORD");
    std::env::remove_var("EDGELORD_KEYSTORE_PASSWORD_FILE");

    let output = Command::new(env!("CARGO_BIN_EXE_edgelord"))
        .args(["check", "live", "--config"])
        .arg(&path)
        .output()
        .expect("run edgelord");

    let _ = fs::remove_file(&path);
    std::env::remove_var("WALLET_PRIVATE_KEY");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Environment is not mainnet"),
        "expected environment warning in stdout: {stdout}"
    );
    assert!(
        !output.status.success(),
        "expected nonzero exit for testnet environment"
    );
}
