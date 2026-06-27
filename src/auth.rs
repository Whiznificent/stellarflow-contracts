use soroban_sdk::{Address, Env, Map, Vec};
use crate::{ContractData, ContractError, DATA_KEY, SIGNERS_KEY};

/// Rigid multi-signature confirmation barrier for parameter shift actions.
/// Requires a supermajority of 4 out of 5 validated administrative signatures
/// before approving changes to system boundary configurations.
pub fn require_multisig(env: &Env, signers: &Vec<Address>) -> Result<(), ContractError> {
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

    let mut valid_count = 0;
    let mut verified: Map<Address, ()> = Map::new(env);

    for i in 0..signers.len() {
        let signer = signers.get(i).unwrap();
        let is_authorized = authorized_signers.contains_key(signer.clone()) || data.admin == signer;
        
        if is_authorized && !verified.contains_key(signer.clone()) {
            signer.require_auth();
            verified.set(signer.clone(), ());
            valid_count += 1;
        }
    }

    // Require a supermajority of 4 out of 5 validated administrative signatures
    if valid_count < 4 {
        return Err(ContractError::ThresholdNotReached);
    }

    Ok(())
}
