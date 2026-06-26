use soroban_sdk::{contracttype, symbol_short, Address, Env, Map, Symbol};
use crate::{ContractData, ContractError, DATA_KEY, SIGNERS_KEY, REVOKED_SIGNER_KEY};


pub(crate) const PENDING_OWNER_KEY: Symbol = symbol_short!("PNDOWN");

// ── Emergency key revocation ─────────────────────────────────────────────

pub(crate) const EMERGENCY_REVOCATION_KEY: Symbol = symbol_short!("EMERREV");

#[contracttype]
#[derive(Clone)]
pub struct EmergencyRevocationProposal {
    /// Address that must be blocked.
    pub target: Address,
    /// Address that will be used as “admin replacement” in this emergency flow.
    /// (Used only to keep existing contract behavior; revocation blocking is the main effect.)
    pub replacement: Address,
    pub proposer: Address,
    pub proposed_at: u64,
    pub votes: Map<Address, ()>,
}


#[contracttype]
#[derive(Clone)]
pub struct PendingOwner {
    pub nominee: Address,
    pub proposed_by: Address,
}

/// Phase 1: current admin nominates a new owner.
/// Stores the nominee under PNDOWN; does not transfer ownership yet.
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
