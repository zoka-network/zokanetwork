//! Synthetic encrypted outputs for the browser's real-WASM tests. All keys
//! come from public, fixed test seeds; never use these accounts for funds.
//! Run with the output JSON path as the first argument.
use zoka_pq_pool::{
    crypto::{Digest, WORDS},
    scan::{ScanBlock, ScanOutput, ScanPage, ScanTransaction},
    wallet::{change_memo, encrypt_note, Account, MEMO_BYTES},
};

fn output(to: &Account, from: Option<&Account>, value: u64, change: bool) -> ScanOutput {
    let rho = Digest([value as u32 % 9; WORDS]);
    let mut memo = [0; MEMO_BYTES];
    if change {
        // Public test vector: BLAKE2b-256(person="ZokaPQViewKey2__",
        // seed=[1;32] || network=2 || index=0 little-endian).
        let view_seed = [
            30, 234, 84, 53, 196, 77, 211, 11, 138, 244, 1, 185, 169, 255, 49, 167, 86, 47, 152,
            183, 63, 98, 205, 241, 127, 135, 226, 236, 177, 105, 235, 125,
        ];
        memo = change_memo(&view_seed, rho);
    } else if from.is_some_and(|from| from.address() != to.address()) {
        memo[..7].copy_from_slice(b"invoice");
    }
    let (note, envelope) = encrypt_note(&to.address(), value, &memo, from, rho).unwrap();
    assert_eq!(
        to.incoming_view_key().unwrap().is_change(&note, &memo),
        change
    );
    ScanOutput {
        commitment: note.commitment(to.network).unwrap(),
        envelope,
    }
}

fn main() {
    let alice = Account::from_seed(&[1; 32], 2, 0).unwrap();
    let bob = Account::from_seed(&[2; 32], 2, 0).unwrap();
    // Alice receives 100, sends 30 + a fee of 1, then sends 10 + a fee of 1.
    // First change is marked; the second is deliberately legacy/unmarked.
    let transfers = vec![
        (true, 0, vec![output(&alice, None, 100, false)]),
        (
            false,
            1,
            vec![
                output(&bob, Some(&alice), 30, false),
                output(&alice, Some(&alice), 69, true),
            ],
        ),
        (
            false,
            1,
            vec![
                output(&bob, Some(&alice), 10, false),
                output(&alice, Some(&alice), 58, false),
            ],
        ),
        (false, 1, vec![output(&alice, Some(&bob), 5, false)]),
        (true, 0, vec![output(&alice, None, 12, false)]),
    ];
    let page = ScanPage {
        network: 2,
        from: 1,
        tip: transfers.len() as u32,
        blocks: transfers
            .into_iter()
            .enumerate()
            .map(|(index, (coinbase, fee, outputs))| {
                let height = index as u32 + 1;
                ScanBlock {
                    height,
                    hash: format!("{height:064x}"),
                    timestamp: 1_000 + u64::from(height) * 120,
                    transactions: vec![ScanTransaction {
                        txid: [height as u8; 32],
                        coinbase,
                        inputs: u32::from(!coinbase),
                        fee,
                        outputs,
                    }],
                }
            })
            .collect(),
    };
    let json = serde_json::json!({
        "description": "Synthetic public seeds [1;32] and [2;32]. No real wallet data.",
        "address": alice.address().short().unwrap(),
        "full": alice.view_key().unwrap().encode(),
        "incoming": alice.incoming_view_key().unwrap().encode(),
        "page": page.encode().unwrap(),
    });
    let path = std::env::args().nth(1).expect("output JSON path");
    std::fs::write(path, serde_json::to_vec(&json).unwrap()).unwrap();
}
