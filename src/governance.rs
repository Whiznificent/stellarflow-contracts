use soroban_sdk::contracttype;

const MIN_LEDGER_DELAY: u32 = 5000;

#[contracttype]
pub struct StagedUpgrade {
    pub wasm_hash: soroban_sdk::BytesN<32>,
    pub staged_at: u32,
}

pub fn verify_staged_delay(staged_at: u32, current_ledger: u32) -> bool {
    current_ledger.saturating_sub(staged_at) >= MIN_LEDGER_DELAY
}

/// Verify that any incoming parameter modification maps to a target execution block height
/// strictly greater than the current active configuration index.
pub fn verify_block_height(target_height: u32, active_index: u32) -> bool {
    target_height > active_index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_block_height() {
        // Strictly greater target height should be valid
        assert!(verify_block_height(101, 100));
        // Equal target height should be invalid
        assert!(!verify_block_height(100, 100));
        // Less than target height should be invalid
        assert!(!verify_block_height(99, 100));
    }
}

