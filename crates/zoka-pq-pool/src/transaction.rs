use crate::{
    crypto::Digest,
    model::{coinbase_rho, commit, transfer_rho, Input, Statement, Witness},
    prove_transfer, verify_transfer,
    wallet::{digest, encrypt_note, Account, Address, Envelope, MEMO_BYTES},
    Error, ProofEnvelope,
};
use serde::{Deserialize, Serialize};

pub const MAX_TRANSACTION_BYTES: usize = 16 * 1024 * 1024;

/// What a coinbase output shows instead of a proof: its value (the reward is
/// public anyway) and its hidden owner `H(owner ‖ rcm)`. With the rho of its
/// height, anyone recomputes the commitment; nobody learns the owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opening {
    pub hide: Digest,
    pub value: u64,
}

/// A transfer carries a STARK and no openings; a coinbase (no nullifiers)
/// carries one opening per output and an empty proof.
#[derive(Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub statement: Statement,
    pub height: u32,
    pub envelopes: Vec<Envelope>,
    pub openings: Vec<Opening>,
    pub proof: ProofEnvelope,
}
pub struct Payment {
    pub to: Address,
    pub value: u64,
    pub memo: [u8; MEMO_BYTES],
}
impl Payment {
    pub fn new(to: Address, value: u64) -> Self {
        Self {
            to,
            value,
            memo: [0; MEMO_BYTES],
        }
    }
}

impl Transaction {
    pub fn effects_digest(&self) -> Result<[u8; 32], Error> {
        let mut statement = self.statement.clone();
        statement.binding = [0; 32];
        let effects =
            postcard::to_allocvec(&(statement, self.height, &self.envelopes, &self.openings))
                .map_err(|_| Error::Encoding)?;
        Ok(digest(b"ZokaPQTxEffects2", &[&effects]))
    }
    pub fn txid(&self) -> [u8; 32] {
        self.statement.binding
    }
    pub fn auth_digest(&self) -> [u8; 32] {
        digest(b"ZokaPQTxAuth2___", &[&self.txid(), &self.proof.proof])
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let bytes = postcard::to_allocvec(self).map_err(|_| Error::Encoding)?;
        if bytes.len() > MAX_TRANSACTION_BYTES {
            return Err(Error::Encoding);
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_TRANSACTION_BYTES {
            return Err(Error::Encoding);
        }
        let (tx, rest): (Self, &[u8]) =
            postcard::take_from_bytes(bytes).map_err(|_| Error::Encoding)?;
        if !rest.is_empty() || tx.encode()? != bytes {
            return Err(Error::Encoding);
        }
        tx.check_shape()?;
        Ok(tx)
    }
    pub fn check_shape(&self) -> Result<(), Error> {
        self.statement.validate()?;
        if self.envelopes.len() != self.statement.commitments.len() {
            return Err(Error::Shape);
        }
        for envelope in &self.envelopes {
            envelope.validate()?;
        }
        let coinbase = self.statement.nullifiers.is_empty();
        if coinbase {
            if self.openings.len() != self.statement.commitments.len()
                || self.proof.version != 2
                || !self.proof.proof.is_empty()
            {
                return Err(Error::Shape);
            }
            for opening in &self.openings {
                let _ = opening.hide.fields()?;
            }
        } else if !self.openings.is_empty() {
            return Err(Error::Shape);
        }
        if self.effects_digest()? != self.txid() {
            return Err(Error::Encoding);
        }
        Ok(())
    }
    pub fn verify(&self, network: u8, height: u32) -> Result<(), Error> {
        self.check_shape()?;
        if self.statement.network != network {
            return Err(Error::Network);
        }
        if self.statement.nullifiers.is_empty() {
            if self.height != height {
                return Err(Error::Ledger("coinbase height"));
            }
            return self.verify_coinbase();
        } else if self.height != 0 && self.height < height {
            return Err(Error::Ledger("expired transaction"));
        }
        verify_transfer(&self.statement, &self.proof)
    }

    /// Each commitment is `H(rho of this height and index ‖ value ‖ hide)` and
    /// the values add up to what the coinbase mints.
    fn verify_coinbase(&self) -> Result<(), Error> {
        let s = &self.statement;
        let mut total = 0u64;
        for (index, (cm, opening)) in s.commitments.iter().zip(&self.openings).enumerate() {
            let rho = coinbase_rho(s.network, self.height, index);
            if commit(s.network, rho, opening.value, opening.hide)? != *cm {
                return Err(Error::Ledger("coinbase commitment"));
            }
            total = total
                .checked_add(opening.value)
                .ok_or(Error::Ledger("coinbase overflow"))?;
        }
        if total != s.minted {
            return Err(Error::Ledger("coinbase value"));
        }
        Ok(())
    }
}

pub fn transfer(
    sender: &Account,
    anchor: Digest,
    inputs: Vec<Input>,
    payments: Vec<Payment>,
    fee: u64,
    expiry: u32,
) -> Result<Transaction, Error> {
    let network = sender.network;
    let first = inputs
        .first()
        .ok_or(Error::Shape)?
        .note
        .nullifier(&inputs[0].key, network)?;
    let mut outputs = vec![];
    let mut envelopes = vec![];
    for (index, payment) in payments.into_iter().enumerate() {
        if payment.to.network != network {
            return Err(Error::Network);
        }
        let rho = transfer_rho(network, first, index)?;
        let (note, envelope) =
            encrypt_note(&payment.to, payment.value, &payment.memo, Some(sender), rho)?;
        outputs.push(note);
        envelopes.push(envelope);
    }
    let witness = Witness { inputs, outputs };
    let statement = Statement::from_witness(network, anchor, fee, 0, [0; 32], &witness)?;
    let mut tx = Transaction {
        statement,
        height: expiry,
        envelopes,
        openings: vec![],
        proof: ProofEnvelope {
            version: 2,
            proof: vec![],
        },
    };
    tx.statement.binding = tx.effects_digest()?;
    tx.proof = prove_transfer(&tx.statement, &witness)?;
    Ok(tx)
}

/// A block reward to `to`. No proof: see [`Opening`].
pub fn coinbase(to: &Address, value: u64, height: u32) -> Result<Transaction, Error> {
    let network = to.network;
    let rho = coinbase_rho(network, height, 0);
    let (note, envelope) = encrypt_note(to, value, &[0; MEMO_BYTES], None, rho)?;
    let statement = Statement {
        version: 2,
        network,
        anchor: Digest::ZERO,
        nullifiers: vec![],
        commitments: vec![note.commitment(network)?],
        fee: 0,
        minted: value,
        binding: [0; 32],
    };
    statement.validate()?;
    let mut tx = Transaction {
        statement,
        height,
        envelopes: vec![envelope],
        openings: vec![Opening {
            hide: note.hide(network)?,
            value,
        }],
        proof: ProofEnvelope {
            version: 2,
            proof: vec![],
        },
    };
    tx.statement.binding = tx.effects_digest()?;
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ledger::PREMINE, model::SpendKey};

    #[test]
    fn a_coinbase_needs_no_proof_and_hides_its_owner_but_not_its_value() {
        let miner = Account::from_seed(&[61; 32], 3, 0).unwrap();
        let cb = coinbase(&miner.address(), PREMINE, 1).unwrap();
        let bytes = cb.encode().unwrap();
        assert!(bytes.len() < 4096, "{} bytes", bytes.len());
        assert!(cb.proof.proof.is_empty());
        let cb = Transaction::decode(&bytes).unwrap();
        cb.verify(3, 1).unwrap();
        let (note, _) = miner
            .receive(cb.statement.commitments[0], &cb.envelopes[0])
            .unwrap();
        assert_eq!(note.value, PREMINE);
        assert_eq!(note.rho, coinbase_rho(3, 1, 0));
        // What is public is the value and H(owner ‖ rcm), not the owner.
        assert_eq!(cb.openings[0].value, PREMINE);
        assert_ne!(cb.openings[0].hide, miner.address().owner);
        assert!(cb.verify(3, 2).is_err(), "another height, another rho");
        assert!(cb.verify(2, 1).is_err());

        let resealed = |mut tx: Transaction| {
            tx.statement.binding = tx.effects_digest().unwrap();
            tx
        };
        let mut more = cb.clone();
        more.openings[0].value += 1;
        more.statement.minted += 1;
        assert!(resealed(more).verify(3, 1).is_err(), "value is in the commitment");
        let mut other = cb.clone();
        other.openings[0].hide = Digest([1; crate::crypto::WORDS]);
        assert!(resealed(other).verify(3, 1).is_err());
        let mut short = cb.clone();
        short.statement.minted -= 1;
        assert!(resealed(short).verify(3, 1).is_err());
        let mut proved = cb.clone();
        proved.proof.proof = vec![0];
        assert!(resealed(proved).check_shape().is_err());
        let mut unsealed = cb.clone();
        unsealed.openings[0].value -= 1;
        assert!(unsealed.check_shape().is_err(), "openings are in the txid");
    }

    #[test]
    fn coinbase_and_transfer_outputs_never_share_a_rho() {
        let key = SpendKey::from_seed(&[62; 32], 3, 0).unwrap();
        let note = crate::model::Note {
            owner: key.owner(3),
            rho: coinbase_rho(3, 7, 0),
            rcm: Digest([3; crate::crypto::WORDS]),
            value: 1,
        };
        let first = note.nullifier(&key, 3).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for height in 1..50 {
            assert!(seen.insert(coinbase_rho(3, height, 0)));
        }
        for index in 0..8 {
            assert!(seen.insert(transfer_rho(3, first, index).unwrap()));
        }
        assert_ne!(coinbase_rho(3, 7, 0), coinbase_rho(2, 7, 0));
    }
}
