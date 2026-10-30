//! What a view key needs of the chain, and the balance it finds there.
//!
//! A node serves every output (commitment and envelope) without the proofs, in
//! pages of blocks; whoever holds a [`ViewKey`] opens them where the key is, so
//! the node never learns whose wallet is being read. Every output of a
//! transfer is also sealed with its sender's outgoing key, so the key also
//! recognises the wallet's own transfers: what they spent is their outputs
//! plus their fee, and the balance is exact without the spending key.
use crate::{
    crypto::Digest,
    ledger::maturity,
    transaction::Transaction,
    wallet::{Envelope, ViewKey, MEMO_BYTES},
    Error,
};
use serde::{Deserialize, Serialize};

/// The most blocks one page carries.
pub const MAX_SCAN_BLOCKS: u32 = 100;
/// The most bytes a scanner accepts for one page.
pub const MAX_SCAN_PAGE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanOutput {
    pub commitment: Digest,
    pub envelope: Envelope,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanTransaction {
    pub txid: [u8; 32],
    pub coinbase: bool,
    pub inputs: u32,
    pub fee: u64,
    pub outputs: Vec<ScanOutput>,
}
impl ScanTransaction {
    pub fn of(tx: &Transaction) -> Self {
        Self {
            txid: tx.txid(),
            coinbase: tx.statement.nullifiers.is_empty(),
            inputs: tx.statement.nullifiers.len() as u32,
            fee: tx.statement.fee,
            outputs: tx
                .statement
                .commitments
                .iter()
                .zip(&tx.envelopes)
                .map(|(commitment, envelope)| ScanOutput {
                    commitment: *commitment,
                    envelope: envelope.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanBlock {
    pub height: u32,
    pub hash: String,
    pub timestamp: u64,
    pub transactions: Vec<ScanTransaction>,
}

/// Consecutive canonical blocks from `from`, and the tip when they were read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanPage {
    pub network: u8,
    pub tip: u32,
    pub from: u32,
    pub blocks: Vec<ScanBlock>,
}
impl ScanPage {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        postcard::to_allocvec(self).map_err(|_| Error::Encoding)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_SCAN_PAGE_BYTES {
            return Err(Error::Encoding);
        }
        let (page, rest): (Self, _) =
            postcard::take_from_bytes(bytes).map_err(|_| Error::Encoding)?;
        if !rest.is_empty() || page.blocks.len() > MAX_SCAN_BLOCKS as usize {
            return Err(Error::Encoding);
        }
        Ok(page)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Received {
    pub height: u32,
    #[serde(with = "hex32")]
    pub txid: [u8; 32],
    pub coinbase: bool,
    pub value: u64,
    pub memo: String,
}

/// One of the wallet's own transfers: the notes it consumed were worth
/// `spent`, everything it created (`paid` to others, `change` back) plus `fee`.
#[derive(Clone, Debug, Serialize)]
pub struct Sent {
    pub height: u32,
    #[serde(with = "hex32")]
    pub txid: [u8; 32],
    pub spent: u64,
    pub paid: u64,
    pub change: u64,
    pub fee: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Balance {
    pub height: u32,
    pub balance_atoms: u64,
    pub spendable_atoms: u64,
    pub immature_atoms: u64,
    pub received_atoms: u64,
    pub sent_atoms: u64,
    pub received: Vec<Received>,
    pub sent: Vec<Sent>,
}

/// A wallet's history as one view key reads it, block after block.
pub struct Viewer {
    key: ViewKey,
    next: u32,
    received: Vec<Received>,
    sent: Vec<Sent>,
}
impl Viewer {
    pub fn new(key: ViewKey) -> Self {
        Self {
            key,
            next: 1,
            received: Vec::new(),
            sent: Vec::new(),
        }
    }
    pub fn key(&self) -> &ViewKey {
        &self.key
    }
    /// The next height it expects.
    pub fn next(&self) -> u32 {
        self.next
    }
    /// Read a page; it has to start where the previous one ended.
    pub fn add_page(&mut self, page: &ScanPage) -> Result<(), Error> {
        if page.network != self.key.network || page.from != self.next {
            return Err(Error::Network);
        }
        for block in &page.blocks {
            self.add(block)?;
        }
        Ok(())
    }
    pub fn add(&mut self, block: &ScanBlock) -> Result<(), Error> {
        if block.height != self.next {
            return Err(Error::Shape);
        }
        for tx in &block.transactions {
            let mut change = 0u64;
            let mut paid = 0u64;
            let mut ours = false;
            for output in &tx.outputs {
                let received = self.key.receive(output.commitment, &output.envelope).ok();
                if let Some((note, memo)) = &received {
                    self.received.push(Received {
                        height: block.height,
                        txid: tx.txid,
                        coinbase: tx.coinbase,
                        value: note.value,
                        memo: memo_text(memo),
                    });
                }
                if tx.coinbase {
                    continue;
                }
                if let Ok((note, _)) = self.key.recover_sent(output.commitment, &output.envelope) {
                    ours = true;
                    let total = if received.is_some() {
                        &mut change
                    } else {
                        &mut paid
                    };
                    *total = total.saturating_add(note.value);
                }
            }
            if ours {
                self.sent.push(Sent {
                    height: block.height,
                    txid: tx.txid,
                    spent: paid.saturating_add(change).saturating_add(tx.fee),
                    paid,
                    change,
                    fee: tx.fee,
                });
            }
        }
        self.next = block.height.checked_add(1).ok_or(Error::Shape)?;
        Ok(())
    }
    /// The balance at the last block read; `tip` is that block's height.
    pub fn balance(&self) -> Balance {
        let tip = self.next.saturating_sub(1);
        let wait = maturity(self.key.network);
        let received = self
            .received
            .iter()
            .fold(0u64, |sum, r| sum.saturating_add(r.value));
        let sent = self
            .sent
            .iter()
            .fold(0u64, |sum, s| sum.saturating_add(s.spent));
        // Block 1 enters the note tree at once; later coinbases `wait` blocks
        // after their own.
        let immature = self
            .received
            .iter()
            .filter(|r| r.coinbase && r.height > 1 && r.height.saturating_add(wait) > tip)
            .fold(0u64, |sum, r| sum.saturating_add(r.value));
        let balance = received.saturating_sub(sent);
        Balance {
            height: tip,
            balance_atoms: balance,
            spendable_atoms: balance.saturating_sub(immature),
            immature_atoms: immature,
            received_atoms: received,
            sent_atoms: sent,
            received: self.received.clone(),
            sent: self.sent.clone(),
        }
    }
}

/// The memo up to its first zero byte, as text (lossy: it is whatever the
/// sender typed).
fn memo_text(memo: &[u8; MEMO_BYTES]) -> String {
    let end = memo.iter().position(|&b| b == 0).unwrap_or(MEMO_BYTES);
    String::from_utf8_lossy(&memo[..end]).into_owned()
}

mod hex32 {
    pub fn serialize<S: serde::Serializer>(bytes: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let text: String = bytes
            .iter()
            .flat_map(|b| {
                [
                    DIGITS[(b >> 4) as usize] as char,
                    DIGITS[(b & 15) as usize] as char,
                ]
            })
            .collect();
        s.serialize_str(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crypto::WORDS,
        wallet::{encrypt_note, Account},
    };

    fn output(to: &Account, value: u64, from: Option<&Account>, memo: &[u8]) -> ScanOutput {
        let mut text = [0; MEMO_BYTES];
        text[..memo.len()].copy_from_slice(memo);
        let (note, envelope) = encrypt_note(
            &to.address(),
            value,
            &text,
            from,
            Digest([value as u32 % 9; WORDS]),
        )
        .unwrap();
        ScanOutput {
            commitment: note.commitment(to.network).unwrap(),
            envelope,
        }
    }

    fn block(height: u32, transactions: Vec<ScanTransaction>) -> ScanBlock {
        ScanBlock {
            height,
            hash: format!("{height:064x}"),
            timestamp: 1_000 + u64::from(height) * 120,
            transactions,
        }
    }

    fn tx(id: u8, coinbase: bool, fee: u64, outputs: Vec<ScanOutput>) -> ScanTransaction {
        ScanTransaction {
            txid: [id; 32],
            coinbase,
            inputs: u32::from(!coinbase),
            fee,
            outputs,
        }
    }

    #[test]
    fn a_view_key_finds_the_exact_balance() {
        let alice = Account::from_seed(&[1; 32], 2, 0).unwrap();
        let bob = Account::from_seed(&[2; 32], 2, 0).unwrap();
        let carol = Account::from_seed(&[3; 32], 2, 0).unwrap();
        let blocks = vec![
            // Alice mines 12, bob mines 12.
            block(1, vec![tx(1, true, 0, vec![output(&alice, 12, None, b"")])]),
            block(2, vec![tx(2, true, 0, vec![output(&bob, 12, None, b"")])]),
            // Bob pays alice 5 (fee 1, change 6).
            block(
                3,
                vec![
                    tx(3, true, 0, vec![output(&carol, 12, None, b"")]),
                    tx(
                        4,
                        false,
                        1,
                        vec![
                            output(&alice, 5, Some(&bob), b"thanks"),
                            output(&bob, 6, Some(&bob), b""),
                        ],
                    ),
                ],
            ),
            // Alice pays carol 7 out of her 17 (fee 1, change 9).
            block(
                4,
                vec![tx(
                    5,
                    false,
                    1,
                    vec![
                        output(&carol, 7, Some(&alice), b""),
                        output(&alice, 9, Some(&alice), b""),
                    ],
                )],
            ),
        ];
        let page = ScanPage {
            network: 2,
            tip: 4,
            from: 1,
            blocks,
        };
        let page = ScanPage::decode(&page.encode().unwrap()).unwrap();

        let mut viewer = Viewer::new(alice.view_key().unwrap());
        viewer.add_page(&page).unwrap();
        let seen = viewer.balance();
        assert_eq!(seen.received_atoms, 12 + 5 + 9);
        assert_eq!(seen.sent_atoms, 17);
        assert_eq!(seen.balance_atoms, 9);
        assert_eq!(seen.sent.len(), 1);
        assert_eq!(
            (seen.sent[0].paid, seen.sent[0].change, seen.sent[0].fee),
            (7, 9, 1)
        );
        assert_eq!(seen.received[1].memo, "thanks");
        // Block 1 counts at once.
        assert_eq!(seen.immature_atoms, 0);

        let mut viewer = Viewer::new(bob.view_key().unwrap());
        viewer.add_page(&page).unwrap();
        let seen = viewer.balance();
        assert_eq!(seen.balance_atoms, 6);
        // Its coinbase of block 2 waits until block 62.
        assert_eq!(seen.immature_atoms, 12);
        assert_eq!(seen.spendable_atoms, 0);

        // Carol never sent: what she got is what she has.
        let mut viewer = Viewer::new(carol.view_key().unwrap());
        viewer.add_page(&page).unwrap();
        assert_eq!(viewer.balance().balance_atoms, 19);
        assert!(viewer.balance().sent.is_empty());

        // A page that does not follow, or of another network, is refused.
        let mut viewer = Viewer::new(alice.view_key().unwrap());
        let mut gap = page.clone();
        gap.from = 2;
        assert!(viewer.add_page(&gap).is_err());
        let mut other = page.clone();
        other.network = 3;
        assert!(viewer.add_page(&other).is_err());
        assert!(ScanPage::decode(&[page.encode().unwrap(), vec![0]].concat()).is_err());
    }
}
