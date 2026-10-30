# Configuration Reference

Primary configuration file: `config.toml`.

Start from `config.toml.example` and override only what is required for your environment.

## Configuration Priority

1. Built-in defaults
2. `config.toml`
3. CLI flags
4. Environment variables (typically for secrets)

## Core Top-Level Settings

```toml
profile = "local"      # local | production | custom
dry_run = true
database = "edgelord.db"
exchange = "polymarket"
```

## Exchange Configuration

```toml
[exchange_config]
type = "polymarket"
environment = "mainnet"     # Public feed; use dry_run=true for observation
chain_id = 137               # Live execution supports Polygon only
ws_url = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
api_url = "https://clob.polymarket.com"
```

Key nested groups:

- `[exchange_config.http]`: REST timeout + retry policy
- `[exchange_config.connections]`: connection lifecycle and fanout
- `[exchange_config.market_filter]`: market universe and quality thresholds
- `[exchange_config.scoring.*]`: subscription prioritization heuristics
- `[exchange_config.dedup]`: market-event dedup strategy

### Market Filter

Controls which markets are tracked based on volume and liquidity thresholds.

```toml
[exchange_config.market_filter]
max_markets = 500              # Maximum markets to track
max_subscriptions = 2000       # Maximum token subscriptions
min_volume_24h = 1000.0        # Minimum 24h volume (USD)
min_liquidity = 500.0          # Minimum liquidity (USD)
max_spread_pct = 0.10          # Maximum bid-ask spread (10%)
include_binary = true          # Include 2-outcome markets
include_multi_outcome = true   # Include multi-outcome markets
max_outcomes = 20              # Maximum outcomes per market
```

CLI overrides: `--max-markets`, `--min-volume`, `--min-liquidity`

### Deduplication

Controls how duplicate WebSocket messages are filtered.

```toml
[exchange_config.dedup]
enabled = true
strategy = "hash"              # Primary: hash | timestamp | content
fallback = "timestamp"         # Fallback if primary fails
cache_ttl_secs = 5             # How long to remember seen messages
max_cache_entries = 100000     # Maximum cache entries
```

## Connection Pool

WebSocket connection management for high-volume subscriptions.

```toml
[connection_pool]
max_connections = 10           # Maximum WebSocket connections
subscriptions_per_connection = 500  # Tokens per connection before rotation
connection_ttl_secs = 120      # Connection lifetime before refresh
preemptive_reconnect_secs = 30 # Start reconnect before TTL expires
health_check_interval_secs = 30  # Check connection health interval
max_silent_secs = 60           # Max seconds without data before reconnect
channel_capacity = 10000       # Event buffer size
```

CLI overrides: `--max-connections`, `--subs-per-connection`, `--connection-ttl`

## Strategies

The safe example enables only single-condition detection with `dry_run = true`. To enable all three, replace the strategy section with the following and substitute real condition IDs whose resolution rules you have reviewed:

```toml
[strategies]
enabled = ["single_condition", "market_rebalancing", "combinatorial"]
exhaustive_groups = [["condition-a", "condition-b", "condition-c"]]

[[strategies.verified_relations]]
type = "implies"
if_yes = "condition-d"
then_yes = "condition-e"

[strategies.single_condition]
min_edge = 0.05
min_profit = 0.50

[strategies.market_rebalancing]
min_edge = 0.03
min_profit = 1.00
max_outcomes = 10

[strategies.combinatorial]
gap_threshold = 0.02
```

`exhaustive_groups` must contain at least three distinct binary conditions covering every possible resolution, with exactly one YES winner. At-most-one is insufficient. All members must survive discovery and filtering or startup fails. These groups create synthetic event markets while retaining individual binary markets.

`verified_relations` supports the serialized `RelationKind` variants (`implies`, `mutually_exclusive`, `exactly_one`, `linear`). Constraints must describe settlement outcomes, not price correlations. Combinatorial execution enumerates 2–8 binary markets; missing books, contradictory constraints and larger clusters do not produce trades. LLM candidates are kept outside the execution relation cache. `max_iterations` and `tolerance` are retained for research API compatibility and do not tune basket execution.

Live startup refuses enabled rebalancing without groups or combinatorial trading without verified relations. The software cannot establish that an operator's asserted relationship is true; review the actual resolution text, deadlines, cancellation rules and payout denomination.

CLI overrides: `--strategies`, `--min-edge`, `--min-profit`.

## Risk Management

```toml
[risk]
max_position_per_market = 100.0   # Maximum position size per market (USD)
max_total_exposure = 500.0        # Maximum total exposure (USD)
min_profit_threshold = 0.50       # Minimum profit to execute (USD)
max_slippage = 0.02               # Maximum slippage (0.02 = 2%)
execution_timeout_secs = 30       # Trade execution timeout (seconds)
```

CLI overrides: `--max-position`, `--max-exposure`, `--min-profit`, `--max-slippage`, `--execution-timeout`

## Telegram Integration

Telegram is enabled by default. Disable with `--no-default-features` if not needed.

```toml
[telegram]
enabled = true
notify_opportunities = false      # Send opportunity alerts (noisy)
notify_executions = true          # Send execution alerts
notify_risk_rejections = true     # Send risk rejection alerts
stats_interval_secs = 30          # Stats polling interval
position_display_limit = 10       # Max positions shown in /positions
```

CLI overrides: `--telegram-enabled`, `--stats-interval`

Runtime bot commands are accepted only from `TELEGRAM_CHAT_ID` and include:

- `/status`, `/health`, `/positions`, `/stats`, `/pool`, `/markets`, `/version`
- `/pause`, `/resume`
- `/set_risk <field> <value>` where `field` is `min_profit`, `max_slippage`, `max_position`, or `max_exposure`

## Governor (Adaptive Scaling)

Controls adaptive subscription management based on latency metrics.

```toml
[governor]
enabled = true

[governor.latency]
target_p50_ms = 10             # Target median latency
target_p95_ms = 50             # Target 95th percentile
target_p99_ms = 100            # Target 99th percentile
max_p99_ms = 200               # Maximum acceptable p99

[governor.scaling]
check_interval_secs = 10       # How often to check metrics
expand_threshold = 0.70        # Add subs when utilization below this
contract_threshold = 1.20      # Remove subs when utilization above this
expand_step = 50               # Subscriptions to add per cycle
contract_step = 100            # Subscriptions to remove per cycle
cooldown_secs = 60             # Minimum time between scaling actions
```

## Reconnection

WebSocket reconnection behavior with exponential backoff.

```toml
[reconnection]
initial_delay_ms = 1000        # Initial delay (1 second)
max_delay_ms = 60000           # Maximum delay (60 seconds)
backoff_multiplier = 2.0       # Double delay each failure
max_consecutive_failures = 10  # Trip circuit breaker after failures
circuit_breaker_cooldown_ms = 300000  # 5 minute cooldown
```

## Optional Inference and Cluster Detection

Enable when using combinatorial relation-based detection.

```toml
[inference]
enabled = false
min_confidence = 0.7
ttl_seconds = 3600
batch_size = 30

[cluster_detection]
enabled = false
debounce_ms = 100
min_gap = 0.02
max_clusters_per_cycle = 10
channel_capacity = 1000
```

## Secrets and Environment Variables

Do not commit secrets to `config.toml`. Use [dugout](https://crates.io/crates/dugout) for secrets management.

### Dugout Setup (Recommended)

```console
$ dugout init
$ dugout set WALLET_PRIVATE_KEY
$ dugout set TELEGRAM_BOT_TOKEN
$ dugout set TELEGRAM_CHAT_ID
$ git add .dugout.toml && git commit -m "chore: add secrets vault"
```

Run commands with secrets injected:

```console
$ dugout run -- edgelord run --config config.toml
```

### Required Secrets

| Variable | Description | Required |
|----------|-------------|----------|
| `WALLET_PRIVATE_KEY` | Trading wallet private key | Yes |
| `TELEGRAM_BOT_TOKEN` | Telegram bot token | If telegram enabled |
| `TELEGRAM_CHAT_ID` | Telegram chat ID | If telegram enabled |
| `ANTHROPIC_API_KEY` | Anthropic API key | If using LLM inference |
| `OPENAI_API_KEY` | OpenAI API key | If using LLM inference |

### Legacy Variables

These are used by the provisioning system:

- `EDGELORD_PRIVATE_KEY`
- `EDGELORD_KEYSTORE_PASSWORD`
- `EDGELORD_KEYSTORE_PASSWORD_FILE`

## CLI Override Summary

All config values can be overridden via CLI flags. Here's a quick reference:

| Config Setting | CLI Flag |
|----------------|----------|
| `exchange_config.chain_id` | `--chain-id`, `--mainnet`, `--testnet` |
| `exchange_config.market_filter.max_markets` | `--max-markets` |
| `exchange_config.market_filter.min_volume_24h` | `--min-volume` |
| `exchange_config.market_filter.min_liquidity` | `--min-liquidity` |
| `connection_pool.max_connections` | `--max-connections` |
| `connection_pool.subscriptions_per_connection` | `--subs-per-connection` |
| `connection_pool.connection_ttl_secs` | `--connection-ttl` |
| `risk.max_slippage` | `--max-slippage` |
| `risk.execution_timeout_secs` | `--execution-timeout` |
| `risk.max_position_per_market` | `--max-position` |
| `risk.max_total_exposure` | `--max-exposure` |
| `telegram.stats_interval_secs` | `--stats-interval` |
| `database` | `--database` |

## Validation Workflow

```console
$ edgelord check config --config config.toml
$ edgelord check health --config config.toml
$ edgelord config show --config config.toml
$ dugout run -- edgelord check live --config config.toml
```

See [CLI Reference](cli-reference.md) for full command options.
