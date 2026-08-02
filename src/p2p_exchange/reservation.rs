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
/// Prefers the **tightest single note** that covers the amount. Picking the
/// largest instead locks far more than the offer is worth: with a wallet full
/// of 12 ZKA rewards plus one note of 61, selling 10 immobilised the whole 61.
/// The surplus only returns as change once the trade closes, so until then it
/// is money the seller cannot touch.
///
/// When no single note covers it, falls back to largest-first so the offer is
/// backed by as few notes as possible.
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

    // Si una sola nota alcanza, se toma la MAS CHICA que alcance en vez de la
    // mas grande. Elegir la mayor minimiza la cantidad de notas, pero inmoviliza
    // de mas: con notas de 12 y una de 61, vender 10 bloqueaba los 61 enteros y
    // el vendedor veia desaparecer cinco veces lo que estaba vendiendo. El
    // excedente vuelve como vuelto recien al cerrarse la operacion, asi que
    // mientras tanto es plata que no puede usar.
    //
    // Se recorre al reves porque `usable` esta ordenado de mayor a menor: el
    // ultimo que todavia cubre el importe es el mas ajustado.
    if let Some(fitting) = usable
        .iter()
        .rev()
        .find(|note| note.amount_atoms >= needed_atoms)
    {
        return Ok(Reservation {
            note_ids: vec![fitting.id.clone()],
            total_atoms: fitting.amount_atoms,
        });
    }
    // Ninguna nota sola alcanza: hay que combinar, y ahi conviene la mayor
    // primero para usar la menor cantidad posible.
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

/// Adapts the wallet's notes into the form [`select_notes`] takes.
///
/// Drops spent notes here rather than in the selector: `SpendableNote` is
/// meant to describe something spendable, so letting a spent one through would
/// make every caller responsible for a filter they could forget.
///
/// `unlock_heights` supplies coinbase maturity, which `PrivateNote` does not
/// carry — the wallet tracks locked rewards separately. Anything missing from
/// the map is treated as mature, matching a note that was never a coinbase.
pub fn spendable_from_wallet_notes(
    notes: &[crate::private_wallet::PrivateNote],
    unlock_heights: &std::collections::HashMap<String, u64>,
) -> Vec<SpendableNote> {
    notes
        .iter()
        .filter(|note| !note.spent)
        .map(|note| SpendableNote {
            id: note.note_id.clone(),
            amount_atoms: note.amount,
            reserved: note.reserved,
            unlock_height: unlock_heights.get(&note.note_id).copied().unwrap_or(0),
        })
        .collect()
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
    fn a_small_offer_locks_the_tightest_note_not_the_biggest() {
        // Cartera tipica de un minero: muchas recompensas iguales y alguna
        // nota grande de un vuelto anterior.
        let notes = vec![
            note("reward-a", 12),
            note("reward-b", 12),
            note("big", 61),
            note("huge", 98),
        ];
        let picked = select_notes(&notes, 10, 0).unwrap();

        assert_eq!(picked.total_atoms, 12, "debe tomar una de 12, no la de 61");
        assert_eq!(picked.note_ids.len(), 1);
        assert!(picked.note_ids[0].starts_with("reward-"));
    }

    #[test]
    fn when_no_single_note_covers_it_combines_the_largest() {
        // Aca si conviene la mayor primero: minimiza cuantas notas se traban.
        let notes = vec![note("a", 12), note("b", 12), note("c", 12)];
        let picked = select_notes(&notes, 30, 0).unwrap();

        assert_eq!(picked.note_ids.len(), 3);
        assert_eq!(picked.total_atoms, 36);
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
    fn backing_an_offer_locks_more_than_it_sells_when_no_note_fits_exactly() {
        // Una nota privada es indivisible: para respaldar 20 hay que inmovilizar
        // la de 28 entera. El excedente vuelve como vuelto al cerrarse, pero
        // hasta entonces no se puede gastar.
        //
        // Esto no es un defecto que haya que corregir aqui: es la consecuencia
        // de no partir notas. Se fija en un test porque la interfaz tiene que
        // mostrar los dos numeros -- lo vendido y lo bloqueado -- y sin eso el
        // usuario ve bajar su saldo mas de lo que vendio y cree que perdio algo.
        let notes = vec![note("grande", 28), note("chica", 5)];
        let picked = select_notes(&notes, 20, 0).unwrap();

        assert_eq!(picked.note_ids, vec!["grande".to_string()]);
        assert_eq!(picked.total_atoms, 28);
        assert!(
            picked.total_atoms > 20,
            "se inmoviliza de mas, y la interfaz debe decirlo"
        );
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

    // ── Wallet adapter ───────────────────────────────────────────────────

    fn wallet_note(id: &str, amount: u64) -> crate::private_wallet::PrivateNote {
        // Only the fields the adapter reads matter; the cryptographic ones are
        // irrelevant to note selection.
        crate::private_wallet::PrivateNote {
            note_id: id.to_string(),
            asset_tag: "ZKA".to_string(),
            source_tx_hash: String::new(),
            recipient_commitment: String::new(),
            stealth_address: String::new(),
            ephemeral_pubkey: Vec::new(),
            commitment: String::new(),
            amount_commitment: String::new(),
            range_proof: Vec::new(),
            one_time_public_key: String::new(),
            commitment_blind: Vec::new(),
            amount,
            is_change: false,
            spent: false,
            created_at: 0,
            spent_at: None,
            reserved: false,
            reserved_by_tx: None,
        }
    }

    #[test]
    fn spent_notes_never_reach_the_selector() {
        // Dropped in the adapter rather than the selector, so no caller can
        // forget the filter and back an offer with coins already gone.
        let mut spent = wallet_note("gone", 500);
        spent.spent = true;
        let notes = vec![spent, wallet_note("live", 100)];

        let spendable = spendable_from_wallet_notes(&notes, &std::collections::HashMap::new());
        assert_eq!(spendable.len(), 1);
        assert_eq!(spendable[0].id, "live");
    }

    #[test]
    fn the_wallets_reserved_flag_is_carried_through() {
        let mut held = wallet_note("held", 100);
        held.reserved = true;
        let spendable =
            spendable_from_wallet_notes(&[held], &std::collections::HashMap::new());
        assert!(spendable[0].reserved);
    }

    #[test]
    fn coinbase_maturity_comes_from_the_supplied_map() {
        // PrivateNote carries no unlock height; the wallet tracks locked
        // rewards separately, so it has to be supplied alongside.
        let notes = vec![wallet_note("reward", 1_000), wallet_note("normal", 100)];
        let mut heights = std::collections::HashMap::new();
        heights.insert("reward".to_string(), 900u64);

        let spendable = spendable_from_wallet_notes(&notes, &heights);
        let reward = spendable.iter().find(|n| n.id == "reward").unwrap();
        let normal = spendable.iter().find(|n| n.id == "normal").unwrap();

        assert_eq!(reward.unlock_height, 900);
        // Absent from the map means it was never a coinbase, so it is mature.
        assert_eq!(normal.unlock_height, 0);

        // Below the unlock height the reward cannot back an offer.
        assert!(matches!(
            select_notes(&spendable, 1_000, 899),
            Err(ReservationError::InsufficientFunds { .. })
        ));
        assert!(select_notes(&spendable, 1_000, 900).is_ok());
    }
}
