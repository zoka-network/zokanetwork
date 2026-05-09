#![no_main]

use libfuzzer_sys::fuzz_target;
use zoka::private_wallet::{PrivateTransaction, PrivateWalletManager};

fuzz_target!(|data: &[u8]| {
    if let Ok(tx) = serde_json::from_slice::<PrivateTransaction>(data) {
        let mut manager = PrivateWalletManager::new();
        let _ = manager.validate_inbound_private_transaction(&tx);
    }
});
