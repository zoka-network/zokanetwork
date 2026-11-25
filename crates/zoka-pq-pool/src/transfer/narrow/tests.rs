use super::*;
use crate::fuzzing::fixture;
use std::collections::BTreeMap;

fn shapes() -> Vec<(usize, usize)> {
    TRANSFER_INPUTS
        .into_iter()
        .map(|n| (n, TRANSFER_OUTPUTS))
        .collect()
}

fn rejects(air: &NarrowAir, trace: &RowMajorMatrix<F>, public: &[F]) -> bool {
    !p3_air::check_all_constraints(air, trace, public, Some(1)).is_ok()
}

/// In every shape: a register lives from the row after the job that writes
/// it (or from its first reading row, if the witness loads it) to its last
/// reading row; no two values share a column at once; and the width stays
/// about the hash's whatever the number of inputs.
#[test]
fn every_value_lives_where_it_is_read() {
    for (n, m) in shapes() {
        let air = NarrowAir::new(n, m);
        let (program, layout) = (&air.program, &air.layout);
        let mut spans: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
        for interval in layout.place.iter().flatten().chain(&layout.worth) {
            assert!(interval.from <= interval.to);
            assert!(interval.to < layout.rows);
            spans
                .entry(interval.column)
                .or_default()
                .push((interval.from, interval.to));
        }
        for column in spans.values_mut() {
            column.sort();
            for pair in column.windows(2) {
                assert!(pair[0].1 < pair[1].0, "{n}x{m}: two values share a column");
            }
        }
        for (j, job) in program.jobs.iter().enumerate() {
            let (first, last) = layout.jobs[j];
            for (k, chunk) in job.message.chunks_exact(RATE).enumerate() {
                let at = first + k.saturating_sub(1);
                for source in chunk {
                    for r in source.registers() {
                        let lives = layout.place[r].unwrap();
                        assert!(lives.from <= at && at <= lives.to, "{n}x{m}: register {r}");
                    }
                }
            }
            for word in 0..WORDS {
                if let Some(lives) = layout.place[job.result + word] {
                    assert_eq!(lives.from, last + 1, "{n}x{m}: a result starts after its job");
                }
            }
            assert!(last < layout.last);
        }
        for r in program.carries..program.carries + 15 {
            let lives = layout.place[r].unwrap();
            assert_eq!((lives.from, lives.to), (layout.last, layout.last));
        }
        eprintln!(
            "{n}x{m}: {} rows, {} columns ({} in version 3), {} periodic",
            layout.rows,
            layout.width(),
            HASH_COLS + program.registers,
            air.num_periodic_columns()
        );
        assert!(layout.width() < HASH_COLS + 256, "{n}x{m}: {}", layout.width());
    }
}

#[test]
fn a_valid_payment_satisfies_the_narrow_air_in_every_shape() {
    for (real, inputs) in [(1, 2), (2, 2), (3, 4), (5, 8)] {
        let (s, w) = super::super::mutations::shape(real, inputs);
        let air = NarrowAir::new(inputs, TRANSFER_OUTPUTS);
        let public = s.public_values().unwrap();
        let trace = air.trace(&s, &w).unwrap();
        let checked = p3_air::check_all_constraints(&air, &trace, &public, Some(8));
        assert!(checked.is_ok(), "{real} of {inputs}: {checked:?}");
    }
}

/// Every cell where a value lives, changed alone, is refused: the column
/// keeps it from the row before, or a constraint reads it on that row.
#[test]
fn every_live_cell_is_held() {
    let (s, w) = fixture();
    let air = NarrowAir::new(s.nullifiers.len(), s.commitments.len());
    let public = s.public_values().unwrap();
    let trace = air.trace(&s, &w).unwrap();
    assert!(!rejects(&air, &trace, &public));
    let width = trace.width;
    let mut cells: Vec<(usize, usize)> = Vec::new();
    for interval in air.layout.place.iter().flatten().chain(&air.layout.worth) {
        cells.push((interval.column, interval.from));
        if interval.to > interval.from {
            cells.push((interval.column, interval.to));
        }
    }
    for l in 0..LIMBS {
        cells.push((Layout::SUMS + l, air.layout.last));
        cells.push((Layout::SUMS + l, air.layout.limbs[1] + 1));
    }
    for (column, row) in cells {
        let mut forged = trace.clone();
        forged.values[row * width + column] += F::ONE;
        assert!(rejects(&air, &forged, &public), "column {column}, row {row}");
    }
}

/// The forgeries version 3's tests try, on the narrow trace: with every
/// permutation and every public commitment recomputed, more money out than
/// in, a value bit that is not a bit and a key that is not the owner's are
/// all refused.
#[test]
fn forged_values_are_refused_even_with_every_hash_recomputed() {
    let (s, w) = fixture();
    let air = NarrowAir::new(2, 2);
    let public = s.public_values().unwrap();
    let (registers, _) = air.program.witness(&s, &w).unwrap();
    let forge = |change: &dyn Fn(&mut Vec<F>)| -> bool {
        let mut registers = registers.clone();
        change(&mut registers);
        let mut inputs = air.program.run_jobs(&mut registers, &public);
        let trace = air.lay_out(&registers, &mut inputs);
        let mut public = public.clone();
        for &(r, p) in &air.program.bindings {
            public[p] = registers[r];
        }
        rejects(&air, &trace, &public)
    };
    let output = air.program.outputs[0].bits;
    // 600 -> 601: one more atom out than in.
    assert!(forge(&|r| r[output] = F::ONE));
    // A "bit" worth 2.
    assert!(forge(&|r| r[output] = F::from_u32(2)));
    // Another key: its owner is not the note's.
    let key = air.program.inputs[0].key;
    assert!(forge(&|r| r[key] += F::ONE));
    // A sibling of another tree: the root is not the anchor.
    let sibling = air.program.inputs[0].siblings;
    assert!(forge(&|r| r[sibling] += F::ONE));
    // A carry that hides a missing atom.
    let carry = air.program.carries;
    assert!(forge(&|r| r[carry] = F::ONE - r[carry]));
    // Nothing changed: accepted, so the harness itself is sound.
    assert!(!forge(&|_| {}));
}

/// Version 3's largest case on the narrow circuit: eight notes of nearly
/// 2^62 each, so the running sums and the carries cross every limb.
#[test]
fn the_largest_shape_with_carries_in_every_limb_proves_and_verifies() {
    use crate::ledger::NoteTree;
    let key = SpendKey::from_seed(&[42; 32], 3, 0).unwrap();
    let mut tree = NoteTree::new(3).unwrap();
    let notes: Vec<_> = (0..MAX_INPUTS)
        .map(|i| Note {
            owner: key.owner(3),
            rho: crate::crypto::Digest([100 + i as u32; WORDS]),
            rcm: crate::crypto::Digest([150 + i as u32; WORDS]),
            value: u64::MAX / 4 - i as u64 * 1000,
        })
        .collect();
    let first = notes[0].nullifier(&key, 3).unwrap();
    for note in &notes {
        tree.append(note.commitment(3).unwrap()).unwrap();
    }
    let inputs = notes
        .iter()
        .enumerate()
        .map(|(i, note)| Input {
            key: key.clone(),
            note: note.clone(),
            path: tree.path(i as u32).unwrap(),
        })
        .collect();
    let total: u128 = notes.iter().map(|note| u128::from(note.value)).sum();
    let fee = 255;
    let first_value = u64::MAX;
    let second_value = u64::try_from(total - u128::from(first_value) - u128::from(fee)).unwrap();
    let outputs = [first_value, second_value]
        .into_iter()
        .enumerate()
        .map(|(i, value)| Note {
            owner: key.owner(3),
            rho: crate::model::transfer_rho(3, first, i).unwrap(),
            rcm: crate::crypto::Digest([200 + i as u32; WORDS]),
            value,
        })
        .collect();
    let witness = Witness { inputs, outputs };
    let statement = Statement::from_witness(3, tree.root(), fee, 0, [42; 32], &witness).unwrap();
    let proof = prove(&statement, &witness).unwrap();
    verify(&statement, &proof).unwrap();
    // One atom more in the second output: the balance row refuses it.
    let air = NarrowAir::new(MAX_INPUTS, TRANSFER_OUTPUTS);
    let public = statement.public_values().unwrap();
    let (mut registers, _) = air.program.witness(&statement, &witness).unwrap();
    let bits = air.program.outputs[1].bits;
    let low = (0..64).find(|&b| registers[bits + b] == F::ZERO).unwrap();
    registers[bits + low] = F::ONE;
    let mut inputs = air.program.run_jobs(&mut registers, &public);
    let trace = air.lay_out(&registers, &mut inputs);
    let mut forged = public.clone();
    for &(r, p) in &air.program.bindings {
        forged[p] = registers[r];
    }
    assert!(rejects(&air, &trace, &forged));
}

/// Plonky3's classical estimator for the narrow circuit, with version 3's
/// FRI parameters and hash assumption: at least as many bits as version 3
/// (`docs/MARGEN-DE-SOLIDEZ.md`). Not a quantum analysis.
#[test]
fn soundness_estimates_cover_every_shape() {
    use p3_field::coset::TwoAdicMultiplicativeCoset;
    use p3_uni_stark::{AirLayout, OpeningShape, ProvenSecurity, StarkSecurityParams};
    for (n, m) in shapes() {
        let air = NarrowAir::new(n, m);
        let fri = FriParameters::<()> {
            log_blowup: LOG_BLOWUP,
            log_final_poly_len: 0,
            max_log_arity: 1,
            num_queries: QUERIES,
            batch_proof_of_work_bits: 0,
            commit_proof_of_work_bits: 0,
            query_proof_of_work_bits: 0,
            mmcs: (),
        };
        let params = StarkSecurityParams::from_air::<F, Challenge, _>(
            fri.security_regime(),
            &air,
            AirLayout::from_air::<F>(&air),
            TwoAdicMultiplicativeCoset::new(F::ONE, air.layout.rows.ilog2() as usize).unwrap(),
            247,
            128,
            2,
            OpeningShape::hiding(RANDOM_CODEWORDS),
            fri.grinding_sites(),
        );
        let bits = ProvenSecurity::compute_from_proof(air.layout.rows.ilog2() as usize + 1, &params)
            .security_bits();
        eprintln!("shape {n}x{m}: {bits} estimated classical bits (hash assumption 128)");
        assert!(bits >= 100, "{n}x{m}: {bits}");
    }
}

#[test]
fn a_narrow_proof_verifies_and_is_no_proof_of_version_3() {
    let (s, w) = fixture();
    let narrow = prove(&s, &w).unwrap();
    assert_eq!(narrow.version, PROOF_V4);
    verify(&s, &narrow).unwrap();
    verify_transfer(&s, &narrow).unwrap();
    let wide = prove_transfer(&s, &w).unwrap();
    // The same bytes under the other version, either way, are refused.
    let mut as_wide = narrow.clone();
    as_wide.version = PROOF_V3;
    assert!(verify_transfer(&s, &as_wide).is_err());
    let mut as_narrow = wide.clone();
    as_narrow.version = PROOF_V4;
    assert!(verify_transfer(&s, &as_narrow).is_err());
    // Another statement: another fee.
    let mut other = s.clone();
    other.fee += 1;
    assert!(verify(&other, &narrow).is_err());
    eprintln!(
        "2x2 proof: {} bytes in version 3, {} in version 4",
        wide.proof.len(),
        narrow.proof.len()
    );
    assert!(narrow.proof.len() * 3 < wide.proof.len());
}

/// What a 2→2 proof of each version is made of, in encoded bytes: the
/// out-of-domain openings, the rows opened at every query (with their shared
/// Merkle proof), and the FRI rounds.
#[test]
#[ignore = "proves two payments; run with --ignored"]
fn what_a_proof_is_made_of() {
    let size = |x: &dyn erased::Size| x.bytes();
    let (s, w) = fixture();
    for version in [PROOF_V3, PROOF_V4] {
        let envelope = prove_transfer_as(&s, &w, version).unwrap();
        let (proof, _): (Proof<Config>, &[u8]) = postcard::take_from_bytes(&envelope.proof).unwrap();
        let (random, fri) = &proof.opening_proof;
        eprintln!("\nversion {version}: {} bytes", envelope.proof.len());
        eprintln!("  out-of-domain openings: {}", size(&proof.opened_values));
        eprintln!("  masking openings: {}", size(random));
        for (i, batch) in fri.input_openings.iter().enumerate() {
            let width: usize = batch.opened_values.first().map_or(0, |q| q.iter().map(Vec::len).sum());
            eprintln!(
                "  input batch {i}: {width} values a query, rows {} + Merkle {}",
                size(&batch.opened_values),
                size(&batch.opening_proof)
            );
        }
        let rounds: Vec<usize> = fri.commit_phase_openings.iter().map(|r| size(r)).collect();
        eprintln!(
            "  FRI: {} rounds, {} bytes (first {:?})",
            rounds.len(),
            rounds.iter().sum::<usize>(),
            rounds.iter().take(4).collect::<Vec<_>>()
        );
        eprintln!(
            "  commitments {}, final polynomial {}",
            size(&fri.commit_phase_commits),
            size(&fri.final_poly)
        );
    }
}

mod erased {
    pub trait Size {
        fn bytes(&self) -> usize;
    }
    impl<T: serde::Serialize> Size for T {
        fn bytes(&self) -> usize {
            postcard::to_allocvec(self).unwrap().len()
        }
    }
}

/// What each version costs, per shape: proof bytes, time to prove and to
/// verify. Slow (six proofs): run it by name, in release.
#[test]
#[ignore = "proves six payments; run with --ignored"]
fn what_each_version_costs() {
    eprintln!("\n| shape | version | proof bytes | prove | verify |");
    eprintln!("| --- | --- | ---: | ---: | ---: |");
    for inputs in TRANSFER_INPUTS {
        let (s, w) = super::super::mutations::shape(inputs.min(3), inputs);
        for version in [PROOF_V3, PROOF_V4] {
            let started = std::time::Instant::now();
            let proof = prove_transfer_as(&s, &w, version).unwrap();
            let proving = started.elapsed();
            let started = std::time::Instant::now();
            verify_transfer(&s, &proof).unwrap();
            let verifying = started.elapsed();
            eprintln!(
                "| {inputs}x{} | {version} | {} | {:.2} s | {:.0} ms |",
                TRANSFER_OUTPUTS,
                proof.proof.len(),
                proving.as_secs_f64(),
                verifying.as_secs_f64() * 1000.0
            );
        }
    }
}
