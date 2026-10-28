use crate::{
    crypto::Digest,
    model::{Input, Statement, Witness},
    prove_transfer, verify_transfer,
    wallet::{digest, encrypt_note, Account, Address, Envelope, MEMO_BYTES},
    Error, ProofEnvelope,
};
use serde::{Deserialize, Serialize};

pub const MAX_TRANSACTION_BYTES: usize = 16 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub statement: Statement,
    pub height: u32,
    pub envelopes: Vec<Envelope>,
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
        let effects = postcard::to_allocvec(&(statement, self.height, &self.envelopes))
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
        } else if self.height != 0 && self.height < height {
            return Err(Error::Ledger("expired transaction"));
        }
        verify_transfer(&self.statement, &self.proof)
    }
}

fn build(
    network: u8,
    anchor: Digest,
    inputs: Vec<Input>,
    payments: Vec<Payment>,
    sender: Option<&Account>,
    fee: u64,
    minted: u64,
    height: u32,
) -> Result<Transaction, Error> {
    let mut outputs = vec![];
    let mut envelopes = vec![];
    for payment in payments {
        if payment.to.network != network {
            return Err(Error::Network);
        }
        let (note, envelope) = encrypt_note(&payment.to, payment.value, &payment.memo, sender)?;
        outputs.push(note);
        envelopes.push(envelope);
    }
    let witness = Witness { inputs, outputs };
    let statement = Statement::from_witness(network, anchor, fee, minted, [0; 32], &witness)?;
    let mut tx = Transaction {
        statement,
        height,
        envelopes,
        proof: ProofEnvelope {
            version: 2,
            proof: vec![],
        },
    };
    tx.statement.binding = tx.effects_digest()?;
    tx.proof = prove_transfer(&tx.statement, &witness)?;
    Ok(tx)
}
pub fn transfer(
    sender: &Account,
    anchor: Digest,
    inputs: Vec<Input>,
    payments: Vec<Payment>,
    fee: u64,
    expiry: u32,
) -> Result<Transaction, Error> {
    if inputs.is_empty() {
        return Err(Error::Shape);
    }
    build(
        sender.network,
        anchor,
        inputs,
        payments,
        Some(sender),
        fee,
        0,
        expiry,
    )
}
pub fn coinbase(to: &Address, value: u64, height: u32) -> Result<Transaction, Error> {
    build(
        to.network,
        Digest::ZERO,
        vec![],
        vec![Payment::new(to.clone(), value)],
        None,
        0,
        value,
        height,
    )
}
