# SubRail Contracts

Soroban smart contracts for SubRail — an open recurring-payments protocol on Stellar. This repo contains the on-chain rulebook only. See the [subrail-protocol](https://github.com/subrail-protocol) org for the full picture, [`subrail-api`](https://github.com/subrail-protocol/subrail-api) for the indexer/keeper/backend, and [`subrail-web`](https://github.com/subrail-protocol/subrail-web) for the frontend.

## How it works

SubRail uses a prepaid-balance model. The subscriber funds a per-subscription balance held by the contract and withdrawable at any time. Each elapsed billing period, `charge` moves at most one period's price from that balance to the merchant — never more, never early.
merchant ──[create_plan]──> Plan (immutable price/interval/grace)
subscriber ──[subscribe: cap + optional lifetime ceiling + deposit]──> Subscription
keeper ──[charge, permissionless]──> price moves balance → merchant, once per period
subscriber ──[deposit / withdraw / pause / resume / cancel]──> full control

### Subscription state machine

                 +---------+

subscribe ------> | Active | <---- resume / successful charge
+---------+
| | |
charge failed --- | | --- pause
v | v
+---------+ +--------+
| PastDue | | Paused |
+---------+ +--------+
|
grace elapsed ------+------> Expired (terminal, residual refunded)
cancel (from Active / PastDue / Paused) ------> Cancelled (terminal, balance refunded)

A failed charge does not revert — it transitions the subscription to `PastDue` and, once the plan's grace window elapses without recovery, to `Expired`. Every transition emits a typed event carrying `prev_status`/`new_status` so indexers can rebuild the state machine without reading contract storage.

## Contract API

| Function                                                                                                                                         | Auth       | Description                                                      |
| ------------------------------------------------------------------------------------------------------------------------------------------------ | ---------- | ---------------------------------------------------------------- |
| `initialize(admin)`                                                                                                                              | —          | One-time setup.                                                  |
| `create_plan(merchant, token, amount, interval, grace_period, name)`                                                                             | merchant   | Create an immutable billing plan.                                |
| `set_plan_active(plan_id, active)`                                                                                                               | merchant   | Open/close a plan to _new_ subscriptions.                        |
| `subscribe(subscriber, plan_id, max_amount, spend_ceiling, initial_deposit)`                                                                     | subscriber | Authorize + fund; first period settles immediately.              |
| `deposit(sub_id, amount)`                                                                                                                        | subscriber | Top up the prepaid balance.                                      |
| `withdraw(sub_id, amount)`                                                                                                                       | subscriber | Pull unused funds out at any time.                               |
| `pause(sub_id)` / `resume(sub_id)`                                                                                                               | subscriber | Freeze billing; schedule shifts by the paused duration.          |
| `cancel(sub_id)`                                                                                                                                 | subscriber | Terminal; refunds the full remaining balance.                    |
| `charge(sub_id)`                                                                                                                                 | **none**   | Keeper entry point; settles a due period or records the failure. |
| `get_plan`, `get_subscription`, `get_merchant_plans`, `get_subscriber_subscriptions`, `get_balance`, `is_charge_due`, `get_admin`, `get_version` | —          | Read-only queries.                                               |

## Development

Prerequisites: Rust (stable), the `wasm32v1-none` target, and the [Stellar CLI](https://developers.stellar.org/docs/tools/cli).

```bash
# run the test suite
cargo test

# lint exactly as CI does
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings

# build the deployable artifact
cargo build --target wasm32v1-none --release
```

## Deploying to testnet

```bash
# one-time: create and fund a testnet identity
stellar keys generate deployer --network testnet --fund

# deploy
stellar contract deploy \
  --wasm target/wasm32v1-none/release/subrail.wasm \
  --source deployer --network testnet

# initialize (use the contract id printed by the deploy)
stellar contract invoke --id <CONTRACT_ID> --source deployer --network testnet \
  -- initialize --admin $(stellar keys address deployer)

# sanity check
stellar contract invoke --id <CONTRACT_ID> --source deployer --network testnet \
  -- get_version
```

## Design notes & v1 boundaries

- **No retroactive back-billing:** recovery from `PastDue` re-anchors the schedule at the recovery time rather than charging missed periods, so a lapsed subscriber can never be hit with a burst of catch-up charges.
- **Intervals are bounded** (1 hour – 366 days) to guard against accidental per-second billing.
- **Plan deactivation is prospective only:** existing subscriptions keep billing; it only blocks new sign-ups.
- Protocol fees, merchant-initiated plan migration, multi-token plans, and auto-cancel of subscriptions on archived plans are intentionally out of scope for v1 — they are tracked as issues.

## License

[MIT](LICENSE)
