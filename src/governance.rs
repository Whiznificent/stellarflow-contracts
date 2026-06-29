use soroban_sdk::{contracttype, Address, BytesN, Env, Map, Symbol};
use crate::ContractError;

// Ballot TTL: ~24 hours at 5 s/ledger, matching the consensus validation window.
const BALLOT_TTL_LEDGERS: u32 = 17_280;
const BALLOT_TTL_THRESHOLD: u32 = 5_000;

/// Pending contract upgrade staged for time-locked execution.
#[contracttype]
#[derive(Clone)]
pub struct StagedUpgrade {
    pub wasm_hash: BytesN<32>,
    pub staged_at: u32,
}

/// Return true once the required ledger delay (5 000 ledgers ≈ 7 h at 5 s/ledger) has elapsed.
pub fn verify_staged_delay(staged_at: u32, current_sequence: u32) -> bool {
    const MIN_LEDGER_DELAY: u32 = 5_000;
    current_sequence.saturating_sub(staged_at) >= MIN_LEDGER_DELAY
}

/// Storage key for an ephemeral voting ballot, scoped by proposal identifier.
#[contracttype]
pub enum BallotKey {
    Proposal(Symbol),
}

/// Ephemeral multi-sig voting ballot stored in Temporary storage.
///
/// The ledger garbage-collects this entry automatically once the TTL expires,
/// keeping the ledger state lean after inconclusive or expired consensus rounds.
/// Explicit `close_ballot` calls provide the primary cleanup path once a round
/// concludes so the ledger is reclaimed immediately rather than waiting for TTL.
#[contracttype]
#[derive(Clone)]
pub struct VotingBallot {
    pub target: Address,
    pub replacement: Address,
    pub proposer: Address,
    pub proposed_at: u64,
    pub votes: Map<Address, ()>,
}

/// Write a new ballot to Temporary storage keyed by `proposal_id`.
///
/// Returns `ProposalAlreadyActive` when a ballot for the same id already exists.
pub fn open_ballot(
    env: &Env,
    proposal_id: Symbol,
    target: Address,
    replacement: Address,
    proposer: Address,
) -> Result<(), ContractError> {
    let key = BallotKey::Proposal(proposal_id);
    if env.storage().temporary().has(&key) {
        return Err(ContractError::ProposalAlreadyActive);
    }
    let ballot = VotingBallot {
        target,
        replacement,
        proposer,
        proposed_at: env.ledger().timestamp(),
        votes: Map::new(env),
    };
    env.storage().temporary().set(&key, &ballot);
    env.storage()
        .temporary()
        .extend_ttl(&key, BALLOT_TTL_THRESHOLD, BALLOT_TTL_LEDGERS);
    Ok(())
}

/// Record a vote on an active ballot, refreshing its TTL on each write.
///
/// Returns the updated ballot so callers can inspect the current vote tally.
pub fn cast_vote(
    env: &Env,
    proposal_id: Symbol,
    voter: Address,
) -> Result<VotingBallot, ContractError> {
    let key = BallotKey::Proposal(proposal_id);
    let mut ballot: VotingBallot = env
        .storage()
        .temporary()
        .get(&key)
        .ok_or(ContractError::NoActiveProposal)?;

    if ballot.votes.contains_key(voter.clone()) {
        return Err(ContractError::AlreadyVoted);
    }
    ballot.votes.set(voter, ());
    env.storage().temporary().set(&key, &ballot);
    env.storage()
        .temporary()
        .extend_ttl(&key, BALLOT_TTL_THRESHOLD, BALLOT_TTL_LEDGERS);
    Ok(ballot)
}

/// Read an active ballot from Temporary storage without mutating it.
pub fn get_ballot(env: &Env, proposal_id: Symbol) -> Option<VotingBallot> {
    env.storage()
        .temporary()
        .get(&BallotKey::Proposal(proposal_id))
}

/// Programmatically delete a ballot once the consensus epoch concludes.
///
/// This is the primary cleanup path; the Temporary TTL acts as a safety net
/// for rounds that expire without reaching threshold or an explicit close call.
pub fn close_ballot(env: &Env, proposal_id: Symbol) {
    env.storage()
        .temporary()
        .remove(&BallotKey::Proposal(proposal_id));
}
