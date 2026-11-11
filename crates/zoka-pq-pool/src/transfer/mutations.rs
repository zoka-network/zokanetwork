//! Every register of the transfer circuit, changed one at a time.
//!
//! A change the AIR accepts must still describe a legitimate transfer: some
//! are (another recipient, another `rcm`, another note of another tree), and
//! a missing constraint is exactly a change it accepts that is not. So each
//! register is changed three ways, the whole hash trace is recomputed and the
//! public values are rebound to what the forged registers compute, as a
//! prover that wants to cheat would do. Whenever the AIR accepts the result,
//! [`intended`] checks it against the model's own functions, not the AIR's
//! jobs: the owner is the key's, the note is in the anchor's tree, the output
//! `rho` comes from the first nullifier and the amounts balance as integers.
//!
//! A soundness bug in this circuit would be inflation nobody sees: the
//! balance lives only inside the proof. These tests are the systematic
//! version of the hand-written forgeries in `transfer::tests`, and the last
//! one checks that they would notice a constraint that went missing.
use super::*;
use crate::{crypto::Digest, ledger::NoteTree};
use p3_field::PrimeField32;

/// A valid transfer of `inputs` notes, `real` of them notes of the tree and
/// the rest padding (worth nothing, in no tree), and the statement it
/// proves. Some unrelated notes come first, so the paths are not those of an
/// empty tree. Two outputs, as every transfer has.
fn shape(real: usize, inputs: usize) -> (Statement, Witness) {
    let (n, m, shape) = (real, TRANSFER_OUTPUTS, inputs);
    let network = 3;
    let key = SpendKey::from_seed(&[7; 32], network, 0).unwrap();
    let mut tree = NoteTree::new(network).unwrap();
    for other in 0..5 {
        tree.append(Digest([900 + other; WORDS])).unwrap();
    }
    let notes: Vec<Note> = (0..n)
        .map(|i| Note {
            owner: key.owner(network),
            rho: Digest([40 + i as u32; WORDS]),
            rcm: Digest([60 + i as u32; WORDS]),
            value: 1_000 + i as u64,
        })
        .collect();
    let indices: Vec<u32> = notes
        .iter()
        .map(|note| tree.append(note.commitment(network).unwrap()).unwrap())
        .collect();
    let first = notes[0].nullifier(&key, network).unwrap();
    let fee = 7;
    let mut left = notes.iter().map(|note| note.value).sum::<u64>() - fee;
    let outputs = (0..m)
        .map(|j| {
            let value = if j + 1 == m { left } else { left / 2 };
            left -= value;
            Note {
                owner: SpendKey::from_seed(&[9 + j as u8; 32], network, 0)
                    .unwrap()
                    .owner(network),
                rho: transfer_rho(network, first, j).unwrap(),
                rcm: Digest([80 + j as u32; WORDS]),
                value,
            }
        })
        .collect();
    let mut inputs: Vec<Input> = notes
        .into_iter()
        .zip(indices)
        .map(|(note, index)| Input {
            key: key.clone(),
            note,
            path: tree.path(index).unwrap(),
        })
        .collect();
    for pad in inputs.len()..shape {
        let key = SpendKey::from_seed(&[100 + pad as u8; 32], network, 0).unwrap();
        inputs.push(Input {
            note: Note {
                owner: key.owner(network),
                rho: Digest([200 + pad as u32; WORDS]),
                rcm: Digest([300 + pad as u32; WORDS]),
                value: 0,
            },
            key,
            path: MerklePath {
                index: pad as u32,
                siblings: std::array::from_fn(|depth| Digest([400 + depth as u32; WORDS])),
            },
        });
    }
    let witness = Witness { inputs, outputs };
    let statement =
        Statement::from_witness(network, tree.root(), fee, 0, [3; 32], &witness).unwrap();
    (statement, witness)
}

/// The transfer the registers describe, as the model defines it. It reads
/// only the registers a prover chooses and the public values; the ones the
/// circuit derives (hides, commitments, nullifiers, the Merkle levels) are
/// recomputed here with `crypto` and `model`, never taken from the trace.
fn intended(program: &Program, r: &[F], public: &[F]) -> Result<(), String> {
    let network = public[1].as_canonical_u32() as u8;
    let digest = |at: usize| Digest::from_fields(std::array::from_fn(|i| r[at + i]));
    let published = |at: usize| Digest::from_fields(std::array::from_fn(|i| public[at + i]));
    let bits = |at: usize, count: usize| -> Result<u64, String> {
        (0..count).try_fold(0u64, |value, i| match r[at + i].as_canonical_u32() {
            0 => Ok(value),
            1 => Ok(value | 1 << i),
            other => Err(format!("register {} holds {other}, not a bit", at + i)),
        })
    };
    let limbs = |at: usize| -> u128 {
        (0..4)
            .map(|i| u128::from(public[at + i].as_canonical_u32()) << (16 * i))
            .sum()
    };
    let note = |layout: &NoteLayout| -> Result<Note, String> {
        Ok(Note {
            owner: digest(layout.owner),
            rho: digest(layout.rho),
            rcm: digest(layout.rcm),
            value: bits(layout.bits, 64)?,
        })
    };
    let n = program.inputs.len();
    let mut incoming = limbs(6);
    let mut first = None;
    for (i, input) in program.inputs.iter().enumerate() {
        let key: [F; WORDS] = std::array::from_fn(|w| r[input.key + w]);
        let note = note(&input.note)?;
        if note.owner != crypto::hash(crypto::OWNER, network, &key) {
            return Err(format!("input {i}: the owner is not its key's"));
        }
        let commitment = note.commitment(network).map_err(|e| e.to_string())?;
        let mut fields = key.to_vec();
        fields.extend(note.rho.fields().map_err(|e| e.to_string())?);
        let nullifier = crypto::hash(crypto::NULLIFIER, network, &fields);
        if nullifier != published(26 + i * WORDS) {
            return Err(format!(
                "input {i}: the published nullifier is not its note's"
            ));
        }
        first.get_or_insert(nullifier);
        let path = MerklePath {
            index: bits(input.index, DEPTH)? as u32,
            siblings: std::array::from_fn(|depth| digest(input.siblings + depth * WORDS)),
        };
        // A note worth nothing needs no tree: it only pads the shape.
        if note.value != 0
            && path.root(network, commitment).map_err(|e| e.to_string())? != published(10)
        {
            return Err(format!("input {i}: the note is not in the anchor's tree"));
        }
        incoming += u128::from(note.value);
    }
    let first = first.ok_or("a transfer without inputs")?;
    let mut outgoing = limbs(2);
    for (j, layout) in program.outputs.iter().enumerate() {
        let note = note(layout)?;
        if note.rho != transfer_rho(network, first, j).map_err(|e| e.to_string())? {
            return Err(format!("output {j}: its rho was chosen"));
        }
        let commitment = note.commitment(network).map_err(|e| e.to_string())?;
        if commitment != published(26 + (n + j) * WORDS) {
            return Err(format!(
                "output {j}: the published commitment is not its note's"
            ));
        }
        outgoing += u128::from(note.value);
    }
    if incoming != outgoing {
        return Err(format!("{incoming} atoms in, {outgoing} out"));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum Change {
    PlusOne,
    Zero,
    MinusOne,
    Flip,
    Two,
}
impl Change {
    fn of(&self, value: F) -> F {
        match self {
            Self::PlusOne => value + F::ONE,
            Self::Zero => F::ZERO,
            Self::MinusOne => F::ZERO - F::ONE,
            Self::Flip => F::ONE - value,
            Self::Two => F::from_u32(2),
        }
    }
}

#[derive(Default)]
struct Outcome {
    tried: usize,
    accepted: usize,
    rejected: usize,
    /// What the AIR accepted that is not a transfer: a missing constraint.
    unsound: Vec<String>,
}

/// The registers to change: all of them, but two words of each sibling and
/// of each Merkle level, the bulk of the trace (thirty-two times sixteen
/// words per input) whose words all play the same role.
fn registers(program: &Program) -> Vec<usize> {
    let merkle_levels: Vec<(usize, usize)> = program
        .jobs
        .iter()
        .filter(|job| matches!(job.message.first(), Some(Source::Constant(domain)) if *domain == crypto::TREE))
        .map(|job| (job.result, job.result + WORDS))
        .collect();
    let edge = |at: usize, start: usize| matches!((at - start) % WORDS, 0 | 15);
    (0..program.registers)
        .filter(|&at| {
            for input in &program.inputs {
                let siblings = input.siblings..input.siblings + DEPTH * WORDS;
                if siblings.contains(&at) {
                    return edge(at, input.siblings);
                }
            }
            match merkle_levels
                .iter()
                .find(|(start, end)| (*start..*end).contains(&at))
            {
                Some((start, _)) => edge(at, *start),
                None => true,
            }
        })
        .collect()
}

/// One change of one register. `None` if it changed nothing.
fn probe(
    air: &TransferAir,
    base: &[F],
    public: &[F],
    at: usize,
    change: Change,
) -> Option<Result<bool, String>> {
    let mut forged = base.to_vec();
    forged[at] = change.of(forged[at]);
    if forged[at] == base[at] {
        return None;
    }
    let (mut trace, mut registers) = air.program.hash_trace(forged.clone(), public);
    // The register is a hash result the recomputation restored: put the
    // changed value back, so the AIR is asked about a wrong result.
    if registers[at] != forged[at] {
        registers[at] = forged[at];
        for row in trace.values.chunks_exact_mut(trace.width) {
            row[HASH_COLS + at] = forged[at];
        }
    }
    let mut published = public.to_vec();
    for &(register, index) in &air.program.bindings {
        published[index] = registers[register];
    }
    let accepted = p3_air::check_all_constraints(air, &trace, &published, Some(1)).is_ok();
    if accepted {
        if let Err(why) = intended(&air.program, &registers, &published) {
            return Some(Err(format!("register {at}, {change:?}: {why}")));
        }
    }
    Some(Ok(accepted))
}

/// Every change of every register in `at`, spread over the machine's cores.
fn sweep(air: &TransferAir, statement: &Statement, witness: &Witness, at: &[usize]) -> Outcome {
    let public = statement.public_values().unwrap();
    let trace = air.program.trace(statement, witness).unwrap();
    assert!(p3_air::check_all_constraints(air, &trace, &public, Some(1)).is_ok());
    let base = trace.values[HASH_COLS..trace.width].to_vec();
    intended(&air.program, &base, &public).expect("the honest transfer is one");
    let bits: std::collections::BTreeSet<usize> = air.program.bits.iter().copied().collect();
    let cases: Vec<(usize, Change)> = at
        .iter()
        .flat_map(|&register| {
            let changes: &[Change] = if bits.contains(&register) {
                &[Change::Flip, Change::Two, Change::MinusOne]
            } else {
                &[Change::PlusOne, Change::Zero, Change::MinusOne]
            };
            changes.iter().map(move |&change| (register, change))
        })
        .collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = cases.len().div_ceil(threads).max(1);
    let results: Vec<Option<Result<bool, String>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = cases
            .chunks(chunk)
            .map(|part| {
                let (base, public) = (&base, &public);
                scope.spawn(move || {
                    part.iter()
                        .map(|&(register, change)| probe(air, base, public, register, change))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    let mut outcome = Outcome::default();
    for result in results.into_iter().flatten() {
        outcome.tried += 1;
        match result {
            Ok(true) => outcome.accepted += 1,
            Ok(false) => outcome.rejected += 1,
            Err(why) => {
                outcome.accepted += 1;
                outcome.unsound.push(why);
            }
        }
    }
    outcome
}

#[test]
fn every_change_the_circuit_accepts_is_still_a_legitimate_transfer() {
    // One real note padded to two, two real notes, and three padded to four.
    for (real, n) in [(1, 2), (2, 2), (3, 4)] {
        let m = TRANSFER_OUTPUTS;
        let (statement, witness) = shape(real, n);
        let air = TransferAir::new(n, m);
        let at = registers(&air.program);
        let started = std::time::Instant::now();
        let outcome = sweep(&air, &statement, &witness, &at);
        eprintln!(
            "{n}x{m} ({real} real): {} registers, {} changes, {} accepted (all legitimate), {} rejected, {:?}",
            at.len(),
            outcome.tried,
            outcome.accepted,
            outcome.rejected,
            started.elapsed()
        );
        assert!(
            outcome.unsound.is_empty(),
            "{n}x{m}: the circuit accepted what is not a transfer:\n{}",
            outcome.unsound.join("\n")
        );
        // Both kinds happen: the sweep reaches legitimate freedom (recipients,
        // rcm, other trees, anything of a padding input) and the constraints
        // that refuse the rest.
        assert!(
            outcome.accepted > 100 && outcome.rejected > 1_000,
            "{n}x{m}"
        );
    }
}

/// The sweep has to notice a constraint that went missing. Three circuits,
/// each one rule short, and the registers that rule is about.
#[test]
fn the_sweep_finds_a_constraint_that_went_missing() {
    let (statement, witness) = shape(1, 2);
    let weakened = |drop: &dyn Fn(&mut Program)| {
        let mut air = TransferAir::new(2, 2);
        drop(&mut air.program);
        air
    };

    // Without the job that derives the first output's rho, the sender picks
    // it: two notes could share a nullifier (Faerie Gold).
    let air = weakened(&|p| {
        let rho = p.outputs[0].rho;
        p.jobs.retain(|job| job.result != rho);
    });
    let rho = air.program.outputs[0].rho;
    let outcome = sweep(
        &air,
        &statement,
        &witness,
        &(rho..rho + WORDS).collect::<Vec<_>>(),
    );
    assert!(
        outcome
            .unsound
            .iter()
            .any(|why| why.contains("rho was chosen")),
        "the missing rho derivation went unnoticed"
    );

    // Without the owner derivation, any key spends the note.
    let air = weakened(&|p| {
        let owner = p.inputs[0].note.owner;
        p.jobs.retain(|job| job.result != owner);
    });
    let key = air.program.inputs[0].key;
    let outcome = sweep(
        &air,
        &statement,
        &witness,
        &(key..key + WORDS).collect::<Vec<_>>(),
    );
    assert!(
        outcome
            .unsound
            .iter()
            .any(|why| why.contains("owner is not its key's")),
        "the missing owner derivation went unnoticed"
    );

    // Without the anchor constraint, the note can be in any tree.
    let air = weakened(&|p| p.anchors.clear());
    let siblings = air.program.inputs[0].siblings;
    let outcome = sweep(&air, &statement, &witness, &[siblings, siblings + 15]);
    assert!(
        outcome
            .unsound
            .iter()
            .any(|why| why.contains("anchor's tree")),
        "the missing anchor binding went unnoticed"
    );
}
