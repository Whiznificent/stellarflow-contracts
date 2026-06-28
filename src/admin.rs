use soroban_sdk::{contracttype, symbol_short, Address, Env, Symbol};
use crate::{ContractData, ContractError, DATA_KEY, SIGNERS_KEY, REVOKED_SIGNER_KEY};
use crate::storage::{SignerKey, RevokedSignerKey};

pub(crate) const PENDING_OWNER_KEY: Symbol = symbol_short!("PNDOWN");
pub(crate) const PAUSED_KEY: Symbol = symbol_short!("PAUSED");

// ── Emergency key revocation ─────────────────────────────────────────────

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
    /// Using Vec<Address> instead of Map for gas optimization.
    pub votes: Vec<Address>,
}

// ── Pending ownership transfer ────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub struct PendingOwner {
    pub nominee: Address,
    pub proposed_by: Address,
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
    let is_signer = _is_signer(env, &proposer);
    if data.admin != proposer && !is_signer {
        return Err(ContractError::Unauthorized);
    }
    proposer.require_auth();

    // Guard: only one active emergency proposal at a time.
    if env.storage().instance().has(&EMERGENCY_REVOCATION_KEY) {
        return Err(ContractError::EmergencyRevocationAlreadyActive);
    }

    // The target must currently be a signer or the admin.
    let target_is_signer = _is_signer(env, &target);
    if data.admin != target && !target_is_signer {
        return Err(ContractError::TargetNotAdmin);
    }

    let mut votes: Vec<Address> = Vec::new(env);
    // The proposer's opening of the proposal counts as their vote.
    votes.push_back(proposer.clone());

    let proposal = EmergencyRevocationProposal {
        target,
        replacement,
        proposer: proposer.clone(),
        proposed_at: env.ledger().timestamp(),
        votes,
    };

    env.storage()
        .instance()
        .set(&EMERGENCY_REVOCATION_KEY, &proposal);

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
    let is_signer = _is_signer(env, &voter);
    if data.admin != voter && !is_signer {
        return Err(ContractError::Unauthorized);
    }

    let mut proposal: EmergencyRevocationProposal = env
        .storage()
        .instance()
        .get(&EMERGENCY_REVOCATION_KEY)
        .ok_or(ContractError::NoActiveEmergencyRevocation)?;

    // Prevent double-voting.
    for i in 0..proposal.votes.len() {
        if proposal.votes.get(i).unwrap() == voter {
            return Err(ContractError::AlreadyVoted);
        }
    }

    // The compromised key must never be allowed to vote on its own revocation.
    if voter == proposal.target {
        return Err(ContractError::Unauthorized);
    }

    proposal.votes.push_back(voter);

    let threshold = _revocation_threshold(env);

    if proposal.votes.len() >= threshold {
        // ── Threshold reached: execute revocation immediately ────────────

        // 1. Stamp the target as revoked in persistent storage.
        //    This is the flag that `assert_not_revoked` checks before every
        //    sensitive operation.
        let revoked_key = RevokedSignerKey(proposal.target.clone());
        env.storage().instance().set(&revoked_key, &true);

        // 2. Remove the target from the active signer set.
        let signer_key = SignerKey(proposal.target.clone());
        env.storage().instance().remove(&signer_key);

        // 3. Promote the replacement into the signer set (unless it is the
        //    target itself, which would be a no-op replacement).
        if proposal.replacement != proposal.target {
            let replacement_key = SignerKey(proposal.replacement.clone());
            env.storage().instance().set(&replacement_key, &true);
        }

        // 4. If the compromised key was the admin, transfer admin rights.
        let mut contract_data = data;
        if contract_data.admin == proposal.target {
            contract_data.admin = proposal.replacement.clone();
            env.storage().instance().set(&DATA_KEY, &contract_data);
        }

        // 5. Wipe the proposal so a fresh one can be raised if needed.
        env.storage()
            .instance()
            .remove(&EMERGENCY_REVOCATION_KEY);
    } else {
        // Threshold not yet reached — persist the updated vote tally.
        env.storage()
            .instance()
            .set(&EMERGENCY_REVOCATION_KEY, &proposal);
    }

    Ok(())
}

// ── Emergency revocation — query ─────────────────────────────────────────

/// Returns the active emergency revocation proposal, if one exists.
pub fn get_emergency_revocation_proposal(
    env: &Env,
) -> Option<EmergencyRevocationProposal> {
    env.storage().instance().get(&EMERGENCY_REVOCATION_KEY)
}

/// Returns `true` if `addr` has been stamped as revoked.
///
/// This is intentionally a pure read — callers that need to *enforce* the
/// check should call `assert_not_revoked` instead.
pub fn is_revoked(env: &Env, addr: &Address) -> bool {
    let revoked_key = RevokedSignerKey(addr.clone());
    env.storage().instance().has(&revoked_key)
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

/// Helper function to check if an address is a registered signer.
fn _is_signer(env: &Env, addr: &Address) -> bool {
    let signer_key = SignerKey(addr.clone());
    env.storage().instance().has(&signer_key)
}

/// Helper function to calculate the revocation threshold.
fn _revocation_threshold(env: &Env) -> u32 {
    let signer_count: u32 = env.storage().instance().get(&SIGNERS_KEY).unwrap_or(0u32);
    if signer_count == 0 { 1 } else { signer_count / 2 + 1 }
}
