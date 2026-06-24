use soroban_sdk::{contractevent, contracttype, Address, Env};

use crate::Error;

pub const MIN_UNBONDING_DELAY_LEDGERS: u32 = 10_000;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnbondingRequest {
    pub validator: Address,
    pub amount: i128,
    pub requested_ledger: u32,
    pub release_ledger: u32,
    pub released: bool,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Unbonding(Address),
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnbondingQueued {
    pub validator: Address,
    pub amount: i128,
    pub requested_ledger: u32,
    pub release_ledger: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnbondingReleased {
    pub validator: Address,
    pub amount: i128,
    pub release_ledger: u32,
}

pub fn request_unbonding(
    env: &Env,
    validator: &Address,
    amount: i128,
) -> Result<UnbondingRequest, Error> {
    if amount <= 0 {
        return Err(Error::InvalidStakeAmount);
    }

    validator.require_auth();

    if let Some(existing) = get_unbonding_request(env, validator) {
        if !existing.released {
            return Err(Error::UnbondingAlreadyQueued);
        }
    }

    let requested_ledger = env.ledger().sequence();
    let release_ledger = requested_ledger
        .checked_add(MIN_UNBONDING_DELAY_LEDGERS)
        .ok_or(Error::LedgerSequenceOverflow)?;
    let request = UnbondingRequest {
        validator: validator.clone(),
        amount,
        requested_ledger,
        release_ledger,
        released: false,
    };

    env.storage()
        .persistent()
        .set(&DataKey::Unbonding(validator.clone()), &request);

    UnbondingQueued {
        validator: validator.clone(),
        amount,
        requested_ledger,
        release_ledger,
    }
    .publish(env);

    Ok(request)
}

pub fn release_unbonded_stake(env: &Env, validator: &Address) -> Result<i128, Error> {
    validator.require_auth();

    let key = DataKey::Unbonding(validator.clone());
    let mut request = env
        .storage()
        .persistent()
        .get::<DataKey, UnbondingRequest>(&key)
        .ok_or(Error::UnbondingRequestNotFound)?;

    if request.released {
        return Err(Error::UnbondingAlreadyReleased);
    }

    let current_ledger = env.ledger().sequence();
    if current_ledger < request.release_ledger {
        return Err(Error::UnbondingDelayActive);
    }

    request.released = true;
    env.storage().persistent().set(&key, &request);

    UnbondingReleased {
        validator: validator.clone(),
        amount: request.amount,
        release_ledger: current_ledger,
    }
    .publish(env);

    Ok(request.amount)
}

pub fn get_unbonding_request(env: &Env, validator: &Address) -> Option<UnbondingRequest> {
    env.storage()
        .persistent()
        .get(&DataKey::Unbonding(validator.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, testutils::Address as _, testutils::Ledger};

    #[contract]
    struct TestContract;

    #[contractimpl]
    impl TestContract {}

    fn setup() -> (Env, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(TestContract, ());
        let validator = Address::generate(&env);
        (env, contract_id, validator)
    }

    #[test]
    fn request_queues_unbonding_for_minimum_delay() {
        let (env, contract_id, validator) = setup();
        env.ledger().set_sequence_number(250);

        env.as_contract(&contract_id, || {
            let request = request_unbonding(&env, &validator, 1_500).unwrap();

            assert_eq!(request.amount, 1_500);
            assert_eq!(request.requested_ledger, 250);
            assert_eq!(request.release_ledger, 10_250);
            assert!(!request.released);
            assert_eq!(get_unbonding_request(&env, &validator), Some(request));
        });
    }

    #[test]
    fn release_fails_before_delay_expires() {
        let (env, contract_id, validator) = setup();
        env.ledger().set_sequence_number(1);

        env.as_contract(&contract_id, || {
            request_unbonding(&env, &validator, 900).unwrap();
            env.ledger()
                .set_sequence_number(MIN_UNBONDING_DELAY_LEDGERS);

            assert_eq!(
                release_unbonded_stake(&env, &validator),
                Err(Error::UnbondingDelayActive)
            );
        });
    }

    #[test]
    fn release_succeeds_at_exact_delay_boundary() {
        let (env, contract_id, validator) = setup();
        env.ledger().set_sequence_number(1);

        env.as_contract(&contract_id, || {
            request_unbonding(&env, &validator, 900).unwrap();
            env.ledger()
                .set_sequence_number(1 + MIN_UNBONDING_DELAY_LEDGERS);

            assert_eq!(release_unbonded_stake(&env, &validator), Ok(900));
            let released = get_unbonding_request(&env, &validator).unwrap();
            assert!(released.released);
        });
    }

    #[test]
    fn duplicate_pending_unbonding_is_rejected() {
        let (env, contract_id, validator) = setup();

        env.as_contract(&contract_id, || {
            request_unbonding(&env, &validator, 900).unwrap();

            assert_eq!(
                request_unbonding(&env, &validator, 700),
                Err(Error::UnbondingAlreadyQueued)
            );
        });
    }
}
