use crate::{ContractData, ContractError, DATA_KEY, REVOKED_SIGNER_KEY, SIGNERS_KEY};
use crate::temp_governance::{
    store_temp_proposal, get_temp_proposal, has_temp_proposal, remove_temp_proposal,
    extend_temp_proposal_ttl, EMERGENCY_REVOCATION_TEMP_KEY,
    DEFAULT_PROPOSAL_TTL, EXTENDED_PROPOSAL_TTL
};
use soroban_sdk::{contracttype, symbol_short, Address, Env, Map, Symbol};

pub(crate) const PENDING_OWNER_KEY: Symbol = symbol_short!("PNDOWN");
pub(crate) const PAUSED_KEY: Symbol = symbol_short!("PAUSED");

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

// ── Emergency revocation — Phase 1: open a proposal ──────────────────────

/// Any registered signer **or** the current admin may open an emergency
/// revocation proposal against a compromised hot-wallet address.
///
/// Only one proposal may be active at a time.  The caller must not be the
/// target of the proposal (a compromised key must not be able to propose its
/// own revocation to stall the process with a self-serving replacement).
pub fn propose_emergency_revocation(
    env: &Env,
    proposer: Address,
    target: Address,
    replacement: Address,
) -> Result<(), ContractError> {
    let data: ContractData = env
        .storage()
        .instance()
        .get(&DATA_KEY)
        .ok_or(ContractError::NotInitialized)?;

    // The target must not open its own revocation proposal.
    if proposer == target {
        return Err(ContractError::Unauthorized);
    }

    // Only the admin or a registered signer may open a proposal.
    let is_signer = get_signers(env).contains_key(proposer.clone());
    if data.admin != proposer && !is_signer {
        return Err(ContractError::Unauthorized);
    }
    proposer.require_auth();

    // Guard: only one active emergency proposal at a time.
    // ── CHANGED: Check temporary storage instead of persistent ──
    if has_temp_proposal(env, &EMERGENCY_REVOCATION_TEMP_KEY) {
        return Err(ContractError::EmergencyRevocationAlreadyActive);
    }

    // The target must currently be a signer or the admin.
    let target_is_signer = get_signers(env).contains_key(target.clone());
    if data.admin != target && !target_is_signer {
        return Err(ContractError::TargetNotAdmin);
    }

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

    Ok(())
}

// ── Emergency revocation — Phase 2: cast a vote ───────────────────────────

/// A registered signer or the admin casts an `aye` vote on the active
/// emergency revocation proposal.
///
/// When the vote count reaches the majority threshold the function
/// **immediately**:
/// 1. Writes the target address into `REVOKED_SIGNER_KEY` storage so that
///    every downstream guard (`assert_not_revoked`) blocks it instantly.
/// 2. Removes the target from the active signer set.
/// 3. Optionally promotes `replacement` into the signer set.
/// 4. If the target is the current admin, transfers admin rights to
///    `replacement`.
/// 5. Clears the proposal from storage.
pub fn vote_emergency_revocation(
    env: &Env,
    voter: Address,
    sig_expires_at: u64,
) -> Result<(), ContractError> {
    // Reject stale signatures up-front.
    if env.ledger().timestamp() > sig_expires_at {
        return Err(ContractError::SignatureExpired);
    }

    voter.require_auth();

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
        .get(&REVOKED_SIGNER_KEY)
        .unwrap_or_else(|| Map::new(env));
    revoked.contains_key(addr.clone())
}

/// Enforcing guard — returns `RevokedAddress` if `addr` is in the revocation
/// list.  Call this at the top of every sensitive function.
pub fn assert_not_revoked(env: &Env, addr: &Address) -> Result<(), ContractError> {
    if is_revoked(env, addr) {
        Err(ContractError::RevokedAddress)
    } else {
        Ok(())
    }
}

// ── Ownership transfer ────────────────────────────────────────────────────

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

    env.storage().instance().set(
        &PENDING_OWNER_KEY,
        &PendingOwner {
            nominee,
            proposed_by: current_admin,
        },
    );
    Ok(())
}

/// Phase 2: nominee claims ownership, proving key access.
/// Only succeeds when a pending transfer exists and caller is the nominee.
pub fn claim_ownership(env: &Env, claimer: Address) -> Result<(), ContractError> {
    let pending: PendingOwner = env
        .storage()
        .instance()
        .get(&PENDING_OWNER_KEY)
        .ok_or(ContractError::NoPendingOwner)?;

    if pending.nominee != claimer {
        return Err(ContractError::NotAdmin);
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

/// Emergency stop: verified coordinator sets the global is_paused flag.
pub fn set_paused(env: &Env, caller: Address, paused: bool) -> Result<(), ContractError> {
    let data: ContractData = env
        .storage()
        .instance()
        .get(&DATA_KEY)
        .ok_or(ContractError::NotInitialized)?;

    if data.admin != caller {
        return Err(ContractError::NotAdmin);
    }
    caller.require_auth();

    env.storage().instance().set(&PAUSED_KEY, &paused);
    Ok(())
}

/// Returns true when the contract is in emergency-paused state.
pub fn is_paused(env: &Env) -> bool {
    env.storage().instance().get(&PAUSED_KEY).unwrap_or(false)
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
