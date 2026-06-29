use crate::{ContractData, ContractError, DATA_KEY, REVOKED_SIGNER_KEY, SIGNERS_KEY};
use crate::temp_governance::{
    store_temp_proposal, get_temp_proposal, has_temp_proposal, remove_temp_proposal,
    extend_temp_proposal_ttl, EMERGENCY_REVOCATION_TEMP_KEY,
    DEFAULT_PROPOSAL_TTL, EXTENDED_PROPOSAL_TTL
};
use soroban_sdk::{contracttype, symbol_short, Address, Env, Map, Symbol};
use crate::{ContractData, ContractError, DATA_KEY, SIGNERS_KEY};

pub(crate) const PENDING_OWNER_KEY: Symbol = symbol_short!("PNDOWN");
pub(crate) const PENDING_ADMIN_KEY: Symbol = symbol_short!("PADMIN");

// ── Emergency key revocation ─────────────────────────────────────────────
// NOTE: Emergency revocation proposals are now stored in temporary storage
// via EMERGENCY_REVOCATION_TEMP_KEY. The persistent EMERGENCY_REVOCATION_KEY
// is kept for backward compatibility but is no longer used for new proposals.

pub(crate) const EMERGENCY_REVOCATION_KEY: Symbol = symbol_short!("EMERREV");

/// Proposal raised by the multi-sig coordinator group to revoke a hot-wallet key.
///
/// Once a majority of registered signers (plus the admin) cast votes, the
/// `target` address is written to `REVOKED_SIGNER_KEY` storage, instantly
/// blocking it from signing or modifying configurations.
#[contracttype]
#[derive(Clone)]
pub struct EmergencyRevocationProposal {
    /// Address whose signing rights must be stripped.
    pub target: Address,
    /// Optional replacement address used to keep the signer set healthy after
    /// revocation.  Pass the target itself if no replacement is needed.
    pub replacement: Address,
    /// Coordinator who opened the proposal.
    pub proposer: Address,
    /// Ledger timestamp at proposal time (informational / audit trail).
    pub proposed_at: u64,
    /// Set of addresses that have already voted `aye` on this proposal.
    pub votes: Map<Address, ()>,
}

// ── Pending ownership transfer ────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub struct PendingOwner {
    pub nominee: Address,
    pub proposed_by: Address,
}

fn get_signers(env: &Env) -> Map<Address, ()> {
    env.storage()
        .instance()
        .get(&SIGNERS_KEY)
        .unwrap_or_else(|| Map::new(env))
}

fn revocation_threshold(env: &Env) -> u32 {
    let n = get_signers(env).len();
    if n == 0 {
        1
    } else {
        n / 2 + 1
    }
}

fn execute_emergency_revocation(
    env: &Env,
    data: ContractData,
    proposal: EmergencyRevocationProposal,
) {
    let mut revoked: Map<Address, ()> = env
        .storage()
        .instance()
        .get(&REVOKED_SIGNER_KEY)
        .unwrap_or_else(|| Map::new(env));
    revoked.set(proposal.target.clone(), ());
    env.storage().instance().set(&REVOKED_SIGNER_KEY, &revoked);

    let mut signers = get_signers(env);
    signers.remove(proposal.target.clone());
    if proposal.replacement != proposal.target {
        signers.set(proposal.replacement.clone(), ());
    }
    env.storage().instance().set(&SIGNERS_KEY, &signers);

    let mut contract_data = data;
    if contract_data.admin == proposal.target {
        contract_data.admin = proposal.replacement.clone();
        env.storage().instance().set(&DATA_KEY, &contract_data);
    }

    // ── CHANGED: Remove from temporary storage instead of persistent ──
    remove_temp_proposal(env, &EMERGENCY_REVOCATION_TEMP_KEY);
}

// ─── Issue #429: Two-phase ownership transfer ────────────────────────────────

/// Phase 1: current admin nominates a new owner.
/// Stores the nominee under `PNDOWN`; does not transfer ownership yet.
pub fn propose_ownership_transfer(
    env: &Env,
    current_admin: Address,
    nominee: Address,
) -> Result<(), ContractError> {
    let data: ContractData = env
        .storage()
        .instance()
        .get(&DATA_KEY)
        .ok_or(ContractError::NotInitialized)?;

    if data.admin != current_admin {
        return Err(ContractError::NotAdmin);
    }
    current_admin.require_auth();

    if env.storage().instance().has(&PENDING_OWNER_KEY) {
        return Err(ContractError::TransferAlreadyPending);
    }

    // Guard: only one active emergency proposal at a time.
    // ── CHANGED: Check temporary storage instead of persistent ──
    if has_temp_proposal(env, &EMERGENCY_REVOCATION_TEMP_KEY) {
        return Err(ContractError::EmergencyRevocationAlreadyActive);
    }

/// Phase 2: nominee claims ownership, proving key access.
/// Only succeeds when a pending transfer exists and caller is the nominee.
pub fn claim_ownership(env: &Env, claimer: Address) -> Result<(), ContractError> {
    let pending: PendingOwner = env
        .storage()
        .instance()
        .get(&PENDING_OWNER_KEY)
        .ok_or(ContractError::NoPendingOwner)?;

    let mut votes: Map<Address, ()> = Map::new(env);
    // The proposer's opening of the proposal counts as their vote.
    votes.set(proposer.clone(), ());

    let proposal = EmergencyRevocationProposal {
        target,
        replacement,
        proposer: proposer.clone(),
        proposed_at: env.ledger().timestamp(),
        votes,
    };

    if proposal.votes.len() >= revocation_threshold(env) {
        execute_emergency_revocation(env, data, proposal);
    } else {
        // ── CHANGED: Store in temporary storage instead of persistent ──
        store_temp_proposal(env, &EMERGENCY_REVOCATION_TEMP_KEY, &proposal, DEFAULT_PROPOSAL_TTL);
    }
    claimer.require_auth();

    let mut data: ContractData = env
        .storage()
        .instance()
        .get(&DATA_KEY)
        .ok_or(ContractError::NotInitialized)?;

    data.admin = claimer;
    env.storage().instance().set(&DATA_KEY, &data);
    env.storage().instance().remove(&PENDING_OWNER_KEY);
    Ok(())
}

// ─── Issue #493: Two-phase admin key revocation ──────────────────────────────
//
// Prevents instant admin key substitution from a single compromised key.
// An admin key change requires EITHER:
//   (a) A secondary independent verification signature from a registered cosigner, OR
//   (b) A 24-hour timelock period to elapse before the change becomes active.
//
// This gives the network window to detect and respond to a compromised key
// before the damage is done.

/// Phase 1: current admin proposes a new admin key.
/// The change is not active until it passes through one of the two verification paths.
pub fn propose_admin_change(
    env: &Env,
    current_admin: Address,
    new_admin: Address,
) -> Result<(), ContractError> {
    let data: ContractData = env
        .storage()
        .instance()
        .get(&DATA_KEY)
        .ok_or(ContractError::NotInitialized)?;

    // Only the admin or a registered signer may vote.
    let is_signer = get_signers(env).contains_key(voter.clone());
    if data.admin != voter && !is_signer {
        return Err(ContractError::Unauthorized);
    }

    // ── CHANGED: Retrieve from temporary storage instead of persistent ──
    let mut proposal: EmergencyRevocationProposal = get_temp_proposal(env, &EMERGENCY_REVOCATION_TEMP_KEY)
        .ok_or(ContractError::NoActiveEmergencyRevocation)?;

    // Prevent double-voting.
    if proposal.votes.contains_key(voter.clone()) {
        return Err(ContractError::AlreadyVoted);
    }

    // The compromised key must never be allowed to vote on its own revocation.
    if voter == proposal.target {
        return Err(ContractError::Unauthorized);
    }

    proposal.votes.set(voter, ());

    let threshold = revocation_threshold(env);

    if proposal.votes.len() >= threshold {
        // ── Threshold reached: execute revocation immediately ────────────

        // 1. Stamp the target as revoked in persistent storage.
        //    This is the flag that `assert_not_revoked` checks before every
        //    sensitive operation.
        let mut revoked: Map<Address, ()> = env
            .storage()
            .instance()
            .get(&REVOKED_SIGNER_KEY)
            .unwrap_or_else(|| Map::new(env));
        revoked.set(proposal.target.clone(), ());
        env.storage().instance().set(&REVOKED_SIGNER_KEY, &revoked);

        // 2. Remove the target from the active signer set.
        let mut signers = get_signers(env);
        signers.remove(proposal.target.clone());

        // 3. Promote the replacement into the signer set (unless it is the
        //    target itself, which would be a no-op replacement).
        if proposal.replacement != proposal.target {
            signers.set(proposal.replacement.clone(), ());
        }
        env.storage().instance().set(&SIGNERS_KEY, &signers);

        // 4. If the compromised key was the admin, transfer admin rights.
        let mut contract_data = data;
        if contract_data.admin == proposal.target {
            contract_data.admin = proposal.replacement.clone();
            env.storage().instance().set(&DATA_KEY, &contract_data);
        }

        // 5. ── CHANGED: Wipe from temporary storage instead of persistent ──
        remove_temp_proposal(env, &EMERGENCY_REVOCATION_TEMP_KEY);
    } else {
        // Threshold not yet reached — persist the updated vote tally.
        // ── CHANGED: Store in temporary storage with extended TTL ──
        store_temp_proposal(env, &EMERGENCY_REVOCATION_TEMP_KEY, &proposal, EXTENDED_PROPOSAL_TTL);
    }
    current_admin.require_auth();

    env.storage().instance().set(
        &PENDING_ADMIN_KEY,
        &AdminChangeProposal {
            new_admin,
            proposer: current_admin,
            proposed_at: env.ledger().timestamp(),
        },
    );
    Ok(())
}

// ── Emergency revocation — query ─────────────────────────────────────────

/// Returns the active emergency revocation proposal, if one exists.
/// ── NOTE: Proposals are now stored in temporary storage and will auto-purge after TTL ──
pub fn get_emergency_revocation_proposal(env: &Env) -> Option<EmergencyRevocationProposal> {
    get_temp_proposal(env, &EMERGENCY_REVOCATION_TEMP_KEY)
}

/// Returns `true` if `addr` has been stamped as revoked.
///
/// This is intentionally a pure read — callers that need to *enforce* the
/// check should call `assert_not_revoked` instead.
pub fn is_revoked(env: &Env, addr: &Address) -> bool {
    let revoked: Map<Address, ()> = env
        .storage()
        .instance()
        .get(&PENDING_ADMIN_KEY)
        .ok_or(ContractError::NoAdminChangePending)?;

    if proposal.proposer == cosigner {
        return Err(ContractError::CosignerCannotBeProposer);
    }

    let authorized_signers: Map<Address, ()> = env
        .storage()
        .instance()
        .get(&SIGNERS_KEY)
        .unwrap_or_else(|| Map::new(env));
    let data: ContractData = env
        .storage()
        .instance()
        .get(&DATA_KEY)
        .ok_or(ContractError::NotInitialized)?;

    let is_authorized =
        authorized_signers.contains_key(cosigner.clone()) || data.admin == cosigner;
    if !is_authorized {
        return Err(ContractError::Unauthorized);
    }
    cosigner.require_auth();

    let mut contract_data = data;
    contract_data.admin = proposal.new_admin;
    env.storage().instance().set(&DATA_KEY, &contract_data);
    env.storage().instance().remove(&PENDING_ADMIN_KEY);
    Ok(())
}

/// Phase 2 — path B: execute the admin change after the 24-hour timelock has elapsed.
/// No secondary signature required; the delay itself acts as the verification window.
pub fn execute_admin_change_by_timelock(
    env: &Env,
    executor: Address,
) -> Result<(), ContractError> {
    let proposal: AdminChangeProposal = env
        .storage()
        .instance()
        .get(&PENDING_ADMIN_KEY)
        .ok_or(ContractError::NoAdminChangePending)?;

    let elapsed = env.ledger().timestamp().saturating_sub(proposal.proposed_at);
    if elapsed < ADMIN_CHANGE_TIMELOCK_SECONDS {
        return Err(ContractError::AdminChangeTimelockNotSatisfied);
    }

    let data: ContractData = env
        .storage()
        .instance()
        .get(&DATA_KEY)
        .ok_or(ContractError::NotInitialized)?;

    if executor != proposal.proposer && executor != data.admin {
        return Err(ContractError::Unauthorized);
    }
    executor.require_auth();

    let mut contract_data = data;
    contract_data.admin = proposal.new_admin;
    env.storage().instance().set(&DATA_KEY, &contract_data);
    env.storage().instance().remove(&PENDING_ADMIN_KEY);
    Ok(())
}

/// Cancel a pending admin change. Only the current admin can cancel.
/// Provides an emergency stop if the proposer's key was compromised.
pub fn cancel_admin_change(
    env: &Env,
    canceller: Address,
) -> Result<(), ContractError> {
    let _proposal: AdminChangeProposal = env
        .storage()
        .instance()
        .get(&PENDING_ADMIN_KEY)
        .ok_or(ContractError::NoAdminChangePending)?;

    let data: ContractData = env
        .storage()
        .instance()
        .get(&DATA_KEY)
        .ok_or(ContractError::NotInitialized)?;

    if data.admin != canceller {
        return Err(ContractError::NotAdmin);
    }
    canceller.require_auth();

    env.storage().instance().remove(&PENDING_ADMIN_KEY);
    Ok(())
}

/// Query the currently pending admin change proposal, if any.
pub fn get_pending_admin_change(env: &Env) -> Option<AdminChangeProposal> {
    env.storage().instance().get(&PENDING_ADMIN_KEY)
}

// ── Proposal cleanup and lifecycle management ────────────────────────────

/// Explicitly purge an expired or stale emergency revocation proposal from temporary storage.
///
/// This function allows the admin or any authorized party to proactively clean up
/// proposals that have failed to reach quorum or have become stale. While the Soroban
/// network will eventually auto-purge these via TTL expiration, explicit removal:
/// - Frees storage resources sooner
/// - Allows reinitiating a new proposal immediately
/// - Provides audit trail of cleanup actions
///
/// The proposal must exist and no threshold check is performed — this is purely
/// a cleanup operation for failed or expired voting attempts.
pub fn purge_emergency_revocation_proposal(env: &Env) -> Result<(), ContractError> {
    // Verify the contract is initialized
    let _data: ContractData = env
        .storage()
        .instance()
        .get(&DATA_KEY)
        .ok_or(ContractError::NotInitialized)?;

    // Purge the proposal from temporary storage if it exists
    if has_temp_proposal(env, &EMERGENCY_REVOCATION_TEMP_KEY) {
        remove_temp_proposal(env, &EMERGENCY_REVOCATION_TEMP_KEY);
    }

    Ok(())
}

/// Check if an emergency revocation proposal is currently active (stored in temp storage).
///
/// Returns true only if the proposal exists in temporary storage and hasn't expired
/// according to Soroban's TTL mechanism.
pub fn has_active_emergency_revocation(env: &Env) -> bool {
    has_temp_proposal(env, &EMERGENCY_REVOCATION_TEMP_KEY)
}
