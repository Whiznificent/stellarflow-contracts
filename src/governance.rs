use soroban_sdk::{contracttype, contracterror, Env};
use ledger_time_helper::current_ledger_sequence;

pub const EXPIRATION_WINDOW_LEDGERS: u32 = 20_000;

// --- kept from the original governance.rs so existing lib.rs imports compile ---
pub use staged::StagedUpgrade;
pub use staged::MIN_LEDGER_DELAY;
pub use staged::verify_staged_delay;

mod staged {
    use soroban_sdk::contracttype;

    pub const MIN_LEDGER_DELAY: u32 = 5000;

    #[contracttype]
    #[derive(Clone, Debug, PartialEq)]
    pub struct StagedUpgrade {
        pub wasm_hash: soroban_sdk::BytesN<32>,
        pub staged_at: u32,
    }

    pub fn verify_staged_delay(staged_at: u32, current_ledger: u32) -> bool {
        current_ledger.saturating_sub(staged_at) >= MIN_LEDGER_DELAY
    }
}

// ---- Governance proposal types ----

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum GovernanceError {
    ProposalNotFound = 1,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum ProposalStatus {
    Active,
    Passed,
    Rejected,
    Defunct,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub proposal_id: u64,
    pub status: ProposalStatus,
    pub created_at_ledger: u32,
    pub vote_count: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Proposal(u64),
    ProposalVotes(u64),
}

/// Checks whether `proposal_id` has exceeded the 20,000-ledger expiration
/// window and, if so, transitions it to `Defunct` and removes both storage
/// slots.  Operates in strict O(1): at most 2 reads, 1 write, 2 removes.
pub fn expire_proposal(env: Env, proposal_id: u64) -> Result<ProposalStatus, GovernanceError> {
    let key = DataKey::Proposal(proposal_id);
    let proposal: Proposal = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(GovernanceError::ProposalNotFound)?;

    // Already in a terminal (non-Active) state — return as-is.
    if proposal.status != ProposalStatus::Active {
        return Ok(proposal.status);
    }

    let elapsed = current_ledger_sequence(&env).saturating_sub(proposal.created_at_ledger);
    if elapsed < EXPIRATION_WINDOW_LEDGERS {
        return Ok(ProposalStatus::Active);
    }

    // Transition and clean up both storage slots.
    env.storage().persistent().remove(&key);
    env.storage()
        .persistent()
        .remove(&DataKey::ProposalVotes(proposal_id));

    Ok(ProposalStatus::Defunct)
}
