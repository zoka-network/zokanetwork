//! What a fuzzer calls, and the valid inputs it starts from.
//!
//! Every function here takes bytes that someone else chose (a peer, a node
//! serving pages, whoever pasted an address) and states what must hold for
//! all of them: the code under it never panics, and what it accepts has one
//! encoding. The same functions run under libFuzzer (`fuzz/` at the root of
//! the repository) and under the deterministic mutation tests of this module,
//! which need no special toolchain and run with every `cargo test`.
//!
//! Compiled for tests and with the `fuzzing` feature only: nothing of this
//! reaches a node or a wallet.
pub mod mutate;
pub mod tree;

#[cfg(test)]
mod tests;

use crate::{
    commitment::{accumulate, block_hash, state_root, Committed},
    crypto::{Digest, WORDS},
    ledger::{subsidy, Ledger, ATOMS},
    light::LightWallet,
    model::{Input, MerklePath, Note, SpendKey, Statement, Witness},
    prove_transfer,
    scan::{IncomingViewer, ScanBlock2, ScanPage, ScanPage2, ScanTransaction2, Viewer},
    transaction::{self, Transaction},
    verify_transfer,
    wallet::{
        Account, Address, Envelope, IncomingViewKey, ShortAddress, ViewKey, SEALED_NOTE_BYTES,
    },
    Error, ProofEnvelope,
};
use std::collections::BTreeSet;
use zoka_pq::X_WING_CIPHERTEXT_BYTES;

/// Every seed is of the regtest network.
pub const NETWORK: u8 = 3;

/// The wallet the page, envelope and state targets read with. Its seed is
/// public: it only ever holds what [`Corpus::build`] mines for it.
pub fn reader() -> Account {
    Account::from_seed(&[0xa1; 32], NETWORK, 0).expect("a fixed seed is an account")
}
/// Whom the seed payment pays.
pub fn payee() -> Account {
    Account::from_seed(&[0xb2; 32], NETWORK, 0).expect("a fixed seed is an account")
}

/// A transaction off the wire: what a node decodes before anything else.
pub fn transaction(data: &[u8]) {
    let Ok(tx) = Transaction::decode(data) else {
        return;
    };
    assert_eq!(
        tx.encode().expect("a decoded transaction encodes"),
        data,
        "one transaction was accepted in a second encoding"
    );
    tx.check_shape()
        .expect("a decoded transaction has its shape");
    assert_eq!(tx.effects_digest().expect("and its effects"), tx.txid());
    let _ = tx.auth_digest();
    let scanned = ScanTransaction2::of(&tx);
    assert_eq!(scanned.coinbase(), tx.statement.nullifiers.is_empty());
    let _ = scanned.view();
    // A proof or a coinbase opening: either verdict, never a panic.
    let _ = tx.verify(tx.statement.network, tx.height);
}

/// One real input and one padding input, two outputs, nothing random:
/// every process computes the same statement, so a proof made in one is a
/// seed for a fuzzer in another.
pub fn fixture() -> (Statement, Witness) {
    let key = SpendKey::from_seed(&[1; 32], NETWORK, 0).expect("a fixed seed is a key");
    let other = SpendKey::from_seed(&[2; 32], NETWORK, 0).expect("a fixed seed is a key");
    let note = Note {
        owner: key.owner(NETWORK),
        rho: Digest([12; WORDS]),
        rcm: Digest([13; WORDS]),
        value: 1000,
    };
    let first = note.nullifier(&key, NETWORK).expect("canonical digests");
    let rho = |index| crate::model::transfer_rho(NETWORK, first, index).expect("a rho");
    let path = MerklePath {
        index: 17,
        siblings: std::array::from_fn(|level| Digest([level as u32 + 100; WORDS])),
    };
    let anchor = path
        .root(NETWORK, note.commitment(NETWORK).expect("a commitment"))
        .expect("a root");
    let outputs = vec![
        Note {
            owner: other.owner(NETWORK),
            rho: rho(0),
            rcm: Digest([21; WORDS]),
            value: 600,
        },
        Note {
            owner: key.owner(NETWORK),
            rho: rho(1),
            rcm: Digest([22; WORDS]),
            value: 395,
        },
    ];
    // The second input pads the shape: worth nothing, in no tree.
    let padding_key = SpendKey::from_seed(&[3; 32], NETWORK, 0).expect("a fixed seed is a key");
    let padding = Input {
        note: Note {
            owner: padding_key.owner(NETWORK),
            rho: Digest([14; WORDS]),
            rcm: Digest([15; WORDS]),
            value: 0,
        },
        key: padding_key,
        path: MerklePath {
            index: 0,
            siblings: [Digest::ZERO; crate::model::DEPTH],
        },
    };
    let witness = Witness {
        inputs: vec![Input { key, note, path }, padding],
        outputs,
    };
    let statement = Statement::from_witness(NETWORK, anchor, 5, 0, [7; 32], &witness)
        .expect("the fixture is a valid statement");
    (statement, witness)
}

/// The bytes of a proof for [`fixture`]'s statement: whether the verifier
/// takes them. It must answer for any bytes at all.
pub fn proof(data: &[u8]) -> bool {
    let (statement, _) = fixture();
    proof_of(&statement, data)
}

/// The same, against any statement.
pub fn proof_of(statement: &Statement, data: &[u8]) -> bool {
    let envelope = ProofEnvelope {
        version: crate::model::VERSION,
        proof: data.to_vec(),
    };
    verify_transfer(statement, &envelope).is_ok()
}

/// A page of the chain as a node serves it to wallets: read by a view key and
/// by a light wallet, the two things that open pages they did not make.
pub fn scan_page(data: &[u8]) {
    let reader = reader();
    if let Ok(page) = ScanPage::decode(data) {
        let mut viewer = Viewer::new(reader.view_key().expect("a view key"));
        let _ = viewer.add_page(&page);
        let exact = viewer.balance();
        let mut incoming =
            IncomingViewer::new(reader.incoming_view_key().expect("an incoming view key"));
        let _ = incoming.add_page(&page);
        let collected = incoming.receipts();
        // The two keys open the same notes: the incoming one only sorts
        // them into what was collected and what came back as change.
        assert_eq!(collected.received.len(), exact.received.len());
        assert_eq!(
            collected
                .received_atoms
                .saturating_add(collected.change_atoms),
            exact.received_atoms
        );
    }
    if let Ok(page) = ScanPage2::decode(data) {
        let _ = page.view();
        let mut wallet = LightWallet::new(NETWORK, 1).expect("a wallet");
        let _ = wallet.add_page(&reader, &page);
        exercise(&wallet, &reader);
        // Whatever a page left in the wallet is a state it can store.
        let state = wallet.encode().expect("a wallet state encodes");
        let again = LightWallet::decode(&state).expect("and decodes");
        assert_eq!(again.encode().expect("twice"), state);
    }
}

fn exercise(wallet: &LightWallet, reader: &Account) {
    let _ = wallet.balance();
    let _ = (
        wallet.height(),
        wallet.next(),
        wallet.tip(),
        wallet.in_sync(),
    );
    let _ = (
        wallet.anchor(),
        wallet.verified(),
        wallet.pending().is_some(),
    );
    let _ = wallet.history().len();
    let _ = wallet.quote(1);
    let _ = wallet.plan(reader, payee().address(), 1, b"fuzz");
}

/// A commitment and an envelope as the `envelope` target takes them.
pub const ENVELOPE_INPUT_BYTES: usize =
    4 * WORDS + X_WING_CIPHERTEXT_BYTES + 12 + SEALED_NOTE_BYTES + 12 + SEALED_NOTE_BYTES;

/// The parts of an `envelope` input, in order, with their lengths.
pub const ENVELOPE_PARTS: [(&str, usize); 6] = [
    ("commitment", 4 * WORDS),
    ("kem", X_WING_CIPHERTEXT_BYTES),
    ("nonce", 12),
    ("encrypted", SEALED_NOTE_BYTES),
    ("out_nonce", 12),
    ("outgoing", SEALED_NOTE_BYTES),
];

pub fn envelope_input(commitment: Digest, envelope: &Envelope) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(ENVELOPE_INPUT_BYTES);
    for word in commitment.0 {
        bytes.extend(word.to_le_bytes());
    }
    bytes.extend(&envelope.kem);
    bytes.extend(envelope.nonce);
    bytes.extend(&envelope.encrypted);
    bytes.extend(envelope.out_nonce);
    bytes.extend(&envelope.outgoing);
    bytes
}

/// One output of the chain, opened by [`reader`]: whether it reads as received
/// and whether it reads as sent. A wallet and its view key always agree.
pub fn envelope(data: &[u8]) -> (bool, bool) {
    if data.len() < ENVELOPE_INPUT_BYTES {
        return (false, false);
    }
    let mut rest = data;
    let mut take = |len: usize| {
        let (part, tail) = rest.split_at(len);
        rest = tail;
        part
    };
    let commitment = take(4 * WORDS);
    let commitment = Digest(std::array::from_fn(|i| {
        u32::from_le_bytes(commitment[i * 4..i * 4 + 4].try_into().expect("four bytes"))
    }));
    let envelope = Envelope {
        kem: take(X_WING_CIPHERTEXT_BYTES).to_vec(),
        nonce: take(12).try_into().expect("twelve bytes"),
        encrypted: take(SEALED_NOTE_BYTES).to_vec(),
        out_nonce: take(12).try_into().expect("twelve bytes"),
        outgoing: take(SEALED_NOTE_BYTES).to_vec(),
    };
    let reader = reader();
    let view = reader.view_key().expect("a view key");
    let received = reader.receive(commitment, &envelope).is_ok();
    let sent = reader.recover_sent(commitment, &envelope).is_ok();
    assert_eq!(
        received,
        view.receive(commitment, &envelope).is_ok(),
        "a wallet and its view key disagree on what was received"
    );
    assert_eq!(
        sent,
        view.recover_sent(commitment, &envelope).is_ok(),
        "a wallet and its view key disagree on what was sent"
    );
    let incoming = reader.incoming_view_key().expect("an incoming view key");
    assert_eq!(
        received,
        incoming.receive(commitment, &envelope).is_ok(),
        "a wallet and its incoming view key disagree on what was received"
    );
    (received, sent)
}

/// Text someone pasted where an address or a view key goes.
pub fn text(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    // Networks 0 and 4 do not exist: they have to be refused, not indexed.
    for network in 0..=4 {
        if let Ok(address) = Address::decode(&text, network) {
            assert_eq!(
                address.encode().expect("a decoded address encodes"),
                text,
                "one address was accepted as a second text"
            );
            let short = address.short_address().expect("and has a short form");
            assert!(short.matches(&address));
            assert_eq!(
                ShortAddress::decode(&short.encode(), network).expect("which decodes"),
                short
            );
        }
        if let Ok(short) = ShortAddress::decode(&text, network) {
            assert_eq!(
                short.encode(),
                text,
                "one short address was accepted as a second text"
            );
        }
        if let Ok(key) = ViewKey::decode(&text, network) {
            assert_eq!(
                key.encode(),
                text.trim(),
                "one view key was accepted as a second text"
            );
            key.address()
                .validate()
                .expect("a view key names a valid address");
        }
        if let Ok(key) = IncomingViewKey::decode(&text, network) {
            assert_eq!(
                key.encode(),
                text.trim(),
                "one incoming view key was accepted as a second text"
            );
            key.address()
                .validate()
                .expect("an incoming view key names a valid address");
        }
    }
}

/// A light wallet's stored state: a file that a crash or a bad disk can
/// leave in any shape. A state that decodes has to be usable or refused.
pub fn wallet_state(data: &[u8]) {
    let Ok(mut wallet) = LightWallet::decode(data) else {
        return;
    };
    let reader = reader();
    exercise(&wallet, &reader);
    let _ = wallet.confirm_checkpoint(1, "00");
    let _ = wallet.encode();
}

/// A chain kept by a full ledger and served as pages with real headers.
struct Chain {
    ledger: Ledger,
    blocks: Vec<ScanBlock2>,
    accumulator: [u8; 32],
}
impl Chain {
    fn new() -> Result<Self, Error> {
        Ok(Self {
            ledger: Ledger::new(NETWORK)?,
            blocks: Vec::new(),
            accumulator: [0; 32],
        })
    }
    /// The block's coinbase.
    fn mine(&mut self, miner: &Account, transfers: Vec<Transaction>) -> Result<Transaction, Error> {
        let height = self.ledger.height + 1;
        let fees: u64 = transfers.iter().map(|tx| tx.statement.fee).sum();
        let coinbase = transaction::coinbase(&miner.address(), subsidy(height) + fees, height)?;
        self.ledger.apply(&coinbase, &transfers)?;
        let prev_hash = self
            .blocks
            .last()
            .map_or_else(|| "0".repeat(64), |block| block.hash.clone());
        let spent: BTreeSet<Digest> = transfers
            .iter()
            .flat_map(|tx| tx.statement.nullifiers.iter().copied())
            .collect();
        self.accumulator = accumulate(&self.accumulator, &spent);
        let state_root = state_root(&Committed {
            network: NETWORK,
            height,
            issued: self.ledger.issued,
            notes: self.ledger.tree.len(),
            root: self.ledger.tree.root(),
            nullifiers: self.accumulator,
        });
        let merkle_root = format!("{height:064x}");
        let timestamp = 1_000 + u64::from(height) * 120;
        let hash = block_hash(
            &prev_hash,
            &merkle_root,
            &state_root,
            timestamp,
            0,
            u64::from(height),
        );
        self.blocks.push(ScanBlock2 {
            height,
            hash,
            prev_hash,
            merkle_root,
            state_root,
            nonce: 0,
            timestamp,
            transactions: std::iter::once(&coinbase)
                .chain(&transfers)
                .map(ScanTransaction2::of)
                .collect(),
        });
        Ok(coinbase)
    }
    fn page(&self, from: u32) -> ScanPage2 {
        ScanPage2 {
            network: NETWORK,
            tip: self.ledger.height,
            from,
            blocks: self.blocks[from as usize - 1..].to_vec(),
        }
    }
}

/// One valid input per target, made the way a node and a wallet make them.
/// Building it proves two transfers: seconds, once per process.
pub struct Corpus {
    /// A block reward: a transaction without a proof.
    pub coinbase: Vec<u8>,
    /// A payment with its proof.
    pub transfer: Vec<u8>,
    /// A proof of [`fixture`]'s statement.
    pub proof: Vec<u8>,
    /// Four blocks as a view key reads them, and as a light wallet does.
    pub page: Vec<u8>,
    pub page2: Vec<u8>,
    /// [`reader`]'s light wallet after those four blocks.
    pub wallet: Vec<u8>,
    /// The change of the payment: [`reader`] both received and sent it.
    pub envelope: Vec<u8>,
    /// An address, its short form, a view key and an incoming view key.
    pub texts: Vec<String>,
}
impl Corpus {
    pub fn build() -> Result<Self, Error> {
        let (reader, payee) = (reader(), payee());
        let mut chain = Chain::new()?;
        let coinbase = chain.mine(&reader, vec![])?;
        chain.mine(&payee, vec![])?;
        chain.mine(&payee, vec![])?;
        let mut wallet = LightWallet::new(NETWORK, 1)?;
        wallet.add_page(&reader, &chain.page(1))?;
        let transfer = wallet
            .plan(&reader, payee.address(), 42 * ATOMS, b"a seed for fuzzers")?
            .prove(&reader)?;
        wallet.reserve(&transfer)?;
        chain.mine(&payee, vec![transfer.clone()])?;
        wallet.add_page(&reader, &chain.page(4))?;
        let page2 = chain.page(1);
        let (statement, witness) = fixture();
        let change = transfer
            .statement
            .commitments
            .get(1)
            .zip(transfer.envelopes.get(1))
            .ok_or(Error::Shape)?;
        Ok(Self {
            coinbase: coinbase.encode()?,
            transfer: transfer.encode()?,
            proof: prove_transfer(&statement, &witness)?.proof,
            page: page2.view().encode()?,
            page2: page2.encode()?,
            wallet: wallet.encode()?,
            envelope: envelope_input(*change.0, change.1),
            texts: vec![
                reader.address().encode()?,
                reader.address().short()?,
                reader.view_key()?.encode(),
                reader.incoming_view_key()?.encode(),
            ],
        })
    }

    /// Each seed with the name of the fuzz target that takes it.
    pub fn seeds(&self) -> Vec<(&'static str, Vec<u8>)> {
        let mut seeds = vec![
            ("transaction", self.coinbase.clone()),
            ("transaction", self.transfer.clone()),
            ("proof", self.proof.clone()),
            ("scan_page", self.page.clone()),
            ("scan_page", self.page2.clone()),
            ("wallet_state", self.wallet.clone()),
            ("envelope", self.envelope.clone()),
        ];
        seeds.extend(
            self.texts
                .iter()
                .map(|text| ("text", text.clone().into_bytes())),
        );
        seeds
    }
}
