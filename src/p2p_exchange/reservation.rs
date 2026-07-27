//! Choosing which private notes back an offer.
//!
//! Publishing an offer must lock the ZKA behind it. Without that a seller can
//! advertise coins they do not have — and worse, advertise the same coins in
//! several offers at once, so whichever buyer pays second cannot be paid at
//! all.
//!
//! This module only *decides*: it picks notes and refuses when they do not
//! cover the amount. Marking them reserved belongs to the wallet, which owns
//! that state.

use serde::{Deserialize, Serialize};

/// A spendable note, as far as reservation is concerned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendableNote {
    pub id: String,
    pub amount_atoms: u64,
    /// Already reserved by another offer or a pending transaction.
    pub reserved: bool,
    /// Coinbase notes stay locked until they mature.
    #[serde(default)]
    pub unlock_height: u64,
}

/// Why an offer could not be backed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReservationError {
    /// Free notes do not add up to the offer.
    InsufficientFunds { needed_atoms: u64, available_atoms: u64 },
    /// The offer asks for nothing.
    ZeroAmount,
}

/// Notes chosen to back an offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reservation {
    pub note_ids: Vec<String>,
    /// Total of the chosen notes. At least the offer amount; the excess
    /// returns as change when the notes are spent.
    pub total_atoms: u64,
}

/// Picks notes covering `needed_atoms` at `chain_height`.
///
/// Skips notes already reserved and coinbase notes that have not matured —
/// both are unspendable, and counting them would let an offer be published
/// against ZKA that cannot actually be handed over.
///
/// Prefers the largest notes first, so an offer locks as few as possible and
/// leaves the wallet with usable change instead of fragmenting it.
pub fn select_notes(
    notes: &[SpendableNote],
    needed_atoms: u64,
    chain_height: u64,
) -> Result<Reservation, ReservationError> {
    if needed_atoms == 0 {
        return Err(ReservationError::ZeroAmount);
    }

    let mut usable: Vec<&SpendableNote> = notes
        .iter()
        .filter(|note| {
            !note.reserved && note.amount_atoms > 0 && note.unlock_height <= chain_height
        })
        .collect();

    // Largest first. Ties break on id so two nodes given the same wallet
    // always choose the same notes, which keeps the decision reproducible.
    usable.sort_by(|a, b| {
        b.amount_atoms
            .cmp(&a.amount_atoms)
            .then_with(|| a.id.cmp(&b.id))
    });

    let available: u64 = usable.iter().map(|n| n.amount_atoms).sum();
    if available < needed_atoms {
        return Err(ReservationError::InsufficientFunds {
            needed_atoms,
            available_atoms: available,
        });
    }

    let mut chosen = Vec::new();
    let mut total: u64 = 0;
    for note in usable {
        chosen.push(note.id.clone());
        total = total.saturating_add(note.amount_atoms);
        if total >= needed_atoms {
            break;
        }
    }

    Ok(Reservation { note_ids: chosen, total_atoms: total })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, atoms: u64) -> SpendableNote {
        SpendableNote {
            id: id.to_string(),
            amount_atoms: atoms,
            reserved: false,
            unlock_height: 0,
        }
    }

    #[test]
    fn it_covers_the_amount_with_the_fewest_notes() {
        let notes = vec![note("a", 10), note("b", 100), note("c", 50)];
        let picked = select_notes(&notes, 120, 0).unwrap();

        // Largest first: 100 + 50 clears 120 without touching the small note.
        assert_eq!(picked.note_ids, vec!["b".to_string(), "c".to_string()]);
        assert_eq!(picked.total_atoms, 150);
    }

    #[test]
    fn an_offer_the_wallet_cannot_back_is_refused() {
        // The whole point: no offer reaches the network unbacked.
        let notes = vec![note("a", 10), note("b", 20)];
        assert_eq!(
            select_notes(&notes, 100, 0),
            Err(ReservationError::InsufficientFunds {
                needed_atoms: 100,
                available_atoms: 30
            })
        );
    }

    #[test]
    fn notes_already_backing_another_offer_are_not_reused() {
        // Reusing them would let two offers promise the same coins, leaving
        // whichever buyer pays second unpayable.
        let mut taken = note("a", 100);
        taken.reserved = true;
        let notes = vec![taken, note("b", 30)];

        assert_eq!(
            select_notes(&notes, 100, 0),
            Err(ReservationError::InsufficientFunds {
                needed_atoms: 100,
                available_atoms: 30
            })
        );
    }

    #[test]
    fn immature_coinbase_notes_do_not_count() {
        // They cannot be spent yet, so an offer backed by them could not be
        // honoured even though the balance looks sufficient.
        let mut locked = note("reward", 1_000);
        locked.unlock_height = 500;
        let notes = vec![locked, note("b", 40)];

        assert!(matches!(
            select_notes(&notes, 100, 499),
            Err(ReservationError::InsufficientFunds { .. })
        ));
        // Once matured it counts.
        assert!(select_notes(&notes, 100, 500).is_ok());
    }

    #[test]
    fn an_exact_match_uses_one_note() {
        let notes = vec![note("a", 100), note("b", 100)];
        let picked = select_notes(&notes, 100, 0).unwrap();
        assert_eq!(picked.note_ids.len(), 1);
        assert_eq!(picked.total_atoms, 100);
    }

    #[test]
    fn selection_is_reproducible_across_equal_notes() {
        // Same wallet, same answer — ties break on id rather than on the
        // order the wallet happened to return.
        let ascending = vec![note("a", 50), note("b", 50)];
        let descending = vec![note("b", 50), note("a", 50)];

        assert_eq!(
            select_notes(&ascending, 50, 0).unwrap().note_ids,
            select_notes(&descending, 50, 0).unwrap().note_ids
        );
    }

    #[test]
    fn a_zero_amount_offer_is_refused() {
        assert_eq!(
            select_notes(&[note("a", 100)], 0, 0),
            Err(ReservationError::ZeroAmount)
        );
    }

    #[test]
    fn an_empty_wallet_is_refused_rather_than_returning_nothing() {
        assert_eq!(
            select_notes(&[], 100, 0),
            Err(ReservationError::InsufficientFunds {
                needed_atoms: 100,
                available_atoms: 0
            })
        );
    }

    #[test]
    fn dust_notes_are_ignored() {
        let notes = vec![note("zero", 0), note("real", 100)];
        let picked = select_notes(&notes, 100, 0).unwrap();
        assert_eq!(picked.note_ids, vec!["real".to_string()]);
    }
}
