use soroban_sdk::{contracttype, Address, Env, Symbol};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Subscription(Address),
    AssetPrice(Symbol),
}

pub const RENT_THRESHOLD: u32 = 259_200;
pub const RENT_EXTEND_TO: u32 = 518_400;

pub const ASSET_TTL_THRESHOLD: u32 = 5_000;
pub const ASSET_TTL_EXTEND_TO: u32 = 100_000;

pub fn extend_subscription_rent(env: &Env, consumer_id: Address) {
    let key = DataKey::Subscription(consumer_id);
    env.storage().persistent().extend_ttl(&key, RENT_THRESHOLD, RENT_EXTEND_TO);
}

pub fn check_subscription(env: &Env, consumer_id: Address) -> bool {
    let key = DataKey::Subscription(consumer_id.clone());
    if env.storage().persistent().has(&key) {
        extend_subscription_rent(env, consumer_id);
        true
    } else {
        false
    }
}

pub fn extend_asset_rent(env: &Env, asset: Symbol) -> bool {
    let key = DataKey::AssetPrice(asset);
    if env.storage().persistent().has(&key) {
        env.storage().persistent().extend_ttl(&key, ASSET_TTL_THRESHOLD, ASSET_TTL_EXTEND_TO);
        true
    } else {
        false
    }
}

pub fn preflight_rent_check(env: &Env) {
    env.storage().instance().extend_ttl(0, ASSET_TTL_THRESHOLD);
}
