use soroban_sdk::{contracttype, symbol_short, Address, Env, Symbol};
use crate::{ContractData, ContractError, DATA_KEY};

pub(crate) const PENDING_OWNER_KEY: Symbol = symbol_short!("PNDOWN");
pub(crate) const PAUSED_KEY: Symbol = symbol_short!("PAUSED");

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
