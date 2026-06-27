# TODO

## Goal

Make `src/lib.rs` (and any dependent modules) compile and pass the existing test suite.

## Step 1 — Baseline diagnostics

- [x] Run `cargo test -q` to get the full compile/test error list.

## Step 2 — Resolve hard compile blockers in `src/lib.rs`

- [ ] Remove duplicate `pub mod validation;`
- [ ] Remove duplicate `update_validator_profile` and duplicate `_resolve_feed_metrics` definitions.
- [ ] Fix `ContractError` discriminant duplication (`DivisionByZero` / `StaleSequence`).
- [ ] Fix `symbol_to_asset_id` implementation to correctly iterate `soroban_sdk::Symbol`.
- [ ] Fix mismatched types between contract APIs and tests (decide whether heartbeat keys use `Symbol` or `AssetId=u32`, and update all signatures + call sites consistently).
- [ ] Fix upgrade/storage inconsistencies (`PendingUpgrade` vs `StagedUpgrade`, undefined constants/keys, undefined `sequence`).

## Step 3 — Resolve module boundary issues

- [ ] Update `src/admin.rs` so it does not call `lib.rs` private helpers (`_get_signers`, `_revocation_threshold`). Either:
  - move those helpers into a shared module, or
  - re-implement the required logic inside `admin.rs`.
- [ ] Ensure all `ContractError` variants referenced by `admin.rs`, `validation.rs`, and tests exist.

## Step 4 — Bring API/tests into alignment

- [ ] Fix `set_value` argument list to match tests (`set_value`/`try_set_value` arity and semantics).
- [ ] Fix `get_upgrade_timelock_remaining` return type mismatch (u64 vs u32) expected by tests.

## Step 5 — Re-run tests iteratively

- [ ] Run `cargo test -q` again.
- [ ] Repeat edit → test cycles until green.
