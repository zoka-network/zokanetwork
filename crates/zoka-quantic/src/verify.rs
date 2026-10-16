//! Proof and signature verification of Quantic transactions.

use orchard::bundle::BatchValidator;
use orchard::circuit::{ProvingKey, VerifyingKey};
use rand_core::{CryptoRng, Rng};

use crate::tx::{bundle_version, QuanticTransaction};

/// Builds the Ironwood proving key (2.8 s on the PC, about 40 s on the test
/// phone; `halo2_proofs` 0.4 cannot store it). Its verifying key comes along.
pub fn proving_key() -> ProvingKey {
    ProvingKey::build(bundle_version().circuit_version())
}

/// Builds only the verifying key (1.9 s on the PC).
pub fn verifying_key() -> VerifyingKey {
    VerifyingKey::build(bundle_version().circuit_version())
}

/// Checks the proofs, the spend signatures and the binding signatures of
/// `transactions` as one batch, each against its own txid. `false` if any of
/// them fails; it does not say which.
pub fn verify_transactions<'a, R: Rng + CryptoRng>(
    vk: &VerifyingKey,
    transactions: impl IntoIterator<Item = &'a QuanticTransaction>,
    rng: R,
) -> bool {
    let mut batch = BatchValidator::new(vk);
    for tx in transactions {
        let bundle = tx.to_orchard_bundle();
        if batch.add_bundle(&bundle, tx.effects().sighash()).is_err() {
            return false;
        }
    }
    batch.validate(rng)
}
