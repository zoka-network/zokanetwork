use crate::{
    crypto::{self, HashAir, Perm, F, RATE, ROUNDS, WIDTH, WORDS},
    model::*,
    Error,
};
use p3_air::{Air, AirBuilder, BaseAir, RowWindow, WindowAccess};
use p3_challenger::DuplexChallenger;
use p3_commit::ExtensionMmcs;
use p3_dft::Radix2DitParallel;
use p3_field::{extension::BinomialExtensionField, Dup, Field, PrimeCharacteristicRing};
use p3_fri::{FriParameters, HidingFriPcs};
use p3_koala_bear::{default_koalabear_poseidon2_32, GenericPoseidon2LinearLayersKoalaBear};
use p3_matrix::dense::RowMajorMatrix;
use p3_merkle_tree::MerkleTreeHidingMmcs;
use p3_poseidon2_air::{generate_trace_rows, num_cols, Poseidon2Cols};
use p3_symmetric::{PaddingFreeSponge, Permutation, TruncatedPermutation};
use p3_uni_stark::{prove, verify, Proof, StarkConfig};
use rand::{rngs::StdRng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::{borrow::Borrow, borrow::Cow};

type Challenge = BinomialExtensionField<F, 8>;
type Hash = PaddingFreeSponge<Perm, 32, 16, 16>;
type Compress = TruncatedPermutation<Perm, 2, 16, 32>;
type ValMmcs = MerkleTreeHidingMmcs<
    <F as Field>::Packing,
    <F as Field>::Packing,
    Hash,
    Compress,
    StdRng,
    2,
    16,
    16,
>;
type ExtMmcs = ExtensionMmcs<F, Challenge, ValMmcs>;
type Pcs = HidingFriPcs<F, Radix2DitParallel<F>, ValMmcs, ExtMmcs, StdRng>;
type Config = StarkConfig<Pcs, Challenge, DuplexChallenger<F, Perm, 32, 16>>;
type Columns<T> = Poseidon2Cols<T, 32, 3, 0, 4, ROUNDS>;
const HASH_COLS: usize = num_cols::<32, 3, 0, 4, ROUNDS>();
const MAX_PROOF_BYTES: usize = 16 * 1024 * 1024;
const RANDOM_CODEWORDS: usize = 8;
const QUERIES: usize = 160;
const LOG_BLOWUP: usize = 3;

fn config() -> Result<Config, Error> {
    // OS-seeded CSPRNGs for every fresh prover configuration. No fixed masks.
    let perm = default_koalabear_poseidon2_32();
    let rng = || {
        StdRng::try_from_rng(&mut rand::rngs::SysRng)
            .map_err(|_| Error::Proof("operating-system randomness unavailable".into()))
    };
    let mmcs = ValMmcs::new(
        Hash::new(perm.clone()),
        Compress::new(perm.clone()),
        0,
        rng()?,
    );
    let fri = FriParameters {
        log_blowup: LOG_BLOWUP,
        log_final_poly_len: 0,
        max_log_arity: 1,
        num_queries: QUERIES,
        batch_proof_of_work_bits: 0,
        commit_proof_of_work_bits: 0,
        query_proof_of_work_bits: 0,
        mmcs: ExtMmcs::new(mmcs.clone()),
    };
    let pcs = Pcs::new(
        Radix2DitParallel::default(),
        mmcs,
        fri,
        RANDOM_CODEWORDS,
        rng()?,
    );
    Ok(Config::new(pcs, DuplexChallenger::new(perm)))
}

#[derive(Clone)]
enum Source {
    Constant(u32),
    Register(usize),
    Public(usize),
    Limb(usize),
    Select { bit: usize, zero: usize, one: usize },
}
impl Source {
    fn value(&self, r: &[F], public: &[F]) -> F {
        match *self {
            Self::Constant(x) => F::from_u32(x),
            Self::Register(i) => r[i],
            Self::Public(i) => public[i],
            Self::Limb(i) => (0..16).map(|b| r[i + b] * F::from_u32(1 << b)).sum(),
            Self::Select { bit, zero, one } => r[zero] + r[bit] * (r[one] - r[zero]),
        }
    }
    fn expr<AB: AirBuilder<F = F>>(&self, r: &[AB::Var], public: &[AB::PublicVar]) -> AB::Expr {
        match *self {
            Self::Constant(x) => AB::Expr::from(F::from_u32(x)),
            Self::Register(i) => r[i].into(),
            Self::Public(i) => public[i].into(),
            Self::Limb(i) => (0..16).map(|b| r[i + b] * F::from_u32(1 << b)).sum(),
            Self::Select { bit, zero, one } => r[zero] + r[bit] * (r[one] - r[zero]),
        }
    }
}

struct HashJob {
    message: Vec<Source>,
    result: usize,
}
struct NoteLayout {
    owner: usize,
    rho: usize,
    rcm: usize,
    bits: usize,
    hide: usize,
    commitment: usize,
}
struct InputLayout {
    note: NoteLayout,
    key: usize,
    nullifier: usize,
    index: usize,
    siblings: usize,
}
struct Program {
    registers: usize,
    inputs: Vec<InputLayout>,
    outputs: Vec<NoteLayout>,
    jobs: Vec<HashJob>,
    bindings: Vec<(usize, usize)>,
    bits: Vec<usize>,
    carries: usize,
    public_count: usize,
    rows: usize,
}

impl Program {
    fn alloc(&mut self, n: usize) -> usize {
        let start = self.registers;
        self.registers += n;
        start
    }
    fn bit_registers(&mut self, n: usize) -> usize {
        let start = self.alloc(n);
        self.bits.extend(start..start + n);
        start
    }
    fn hash(&mut self, domain: u32, message: Vec<Source>, result: usize) {
        let mut words = vec![Source::Constant(domain), Source::Public(1)];
        words.extend(message);
        words.push(Source::Constant(1));
        words.resize(words.len().div_ceil(RATE) * RATE, Source::Constant(0));
        self.jobs.push(HashJob {
            message: words,
            result,
        });
    }
    /// Registers of a note and its two jobs, `hide = H(owner ‖ rcm)` and
    /// `cm = H(rho ‖ value ‖ hide)` (see `model::Note`).
    fn note(&mut self) -> NoteLayout {
        let note = NoteLayout {
            owner: self.alloc(WORDS),
            rho: self.alloc(WORDS),
            rcm: self.alloc(WORDS),
            bits: self.bit_registers(64),
            hide: self.alloc(WORDS),
            commitment: self.alloc(WORDS),
        };
        self.hash(
            crypto::HIDE,
            (note.owner..note.owner + WORDS)
                .chain(note.rcm..note.rcm + WORDS)
                .map(Source::Register)
                .collect(),
            note.hide,
        );
        let mut fields = (note.rho..note.rho + WORDS)
            .map(Source::Register)
            .collect::<Vec<_>>();
        fields.extend((0..4).map(|i| Source::Limb(note.bits + i * 16)));
        fields.extend((note.hide..note.hide + WORDS).map(Source::Register));
        self.hash(crypto::NOTE, fields, note.commitment);
        note
    }
    /// `n >= 1`: output `rho`s come from the first nullifier. A coinbase has
    /// no inputs and no proof (`transaction::Transaction::verify`).
    fn new(n: usize, m: usize) -> Self {
        assert!(n >= 1, "a proven transfer spends at least one note");
        let mut p = Self {
            registers: 0,
            inputs: vec![],
            outputs: vec![],
            jobs: vec![],
            bindings: vec![],
            bits: vec![],
            carries: 0,
            public_count: 26 + WORDS * (n + m) + 32,
            rows: 0,
        };
        for i in 0..n {
            let key = p.alloc(WORDS);
            let note = p.note();
            // Reorder owner derivation before its note commitment.
            let note_jobs = p.jobs.split_off(p.jobs.len() - 2);
            p.hash(
                crypto::OWNER,
                (key..key + WORDS).map(Source::Register).collect(),
                note.owner,
            );
            p.jobs.extend(note_jobs);
            let nullifier = p.alloc(WORDS);
            p.hash(
                crypto::NULLIFIER,
                (key..key + WORDS)
                    .chain(note.rho..note.rho + WORDS)
                    .map(Source::Register)
                    .collect(),
                nullifier,
            );
            p.bindings
                .extend((0..WORDS).map(|j| (nullifier + j, 26 + i * WORDS + j)));
            let index = p.bit_registers(DEPTH);
            let siblings = p.alloc(DEPTH * WORDS);
            let mut current = note.commitment;
            for depth in 0..DEPTH {
                let sibling = siblings + depth * WORDS;
                let next = p.alloc(WORDS);
                let mut fields = vec![Source::Constant(depth as u32)];
                fields.extend((0..WORDS).map(|j| Source::Select {
                    bit: index + depth,
                    zero: current + j,
                    one: sibling + j,
                }));
                fields.extend((0..WORDS).map(|j| Source::Select {
                    bit: index + depth,
                    zero: sibling + j,
                    one: current + j,
                }));
                p.hash(crypto::TREE, fields, next);
                current = next;
            }
            p.bindings.extend((0..WORDS).map(|j| (current + j, 10 + j)));
            p.inputs.push(InputLayout {
                note,
                key,
                nullifier,
                index,
                siblings,
            });
        }
        let first = p.inputs[0].nullifier;
        for i in 0..m {
            let note = p.note();
            // The output's rho is H(first nullifier ‖ i), never the sender's
            // choice: no two notes on a chain can share a nullifier.
            let note_jobs = p.jobs.split_off(p.jobs.len() - 2);
            let mut fields = (first..first + WORDS)
                .map(Source::Register)
                .collect::<Vec<_>>();
            fields.push(Source::Constant(i as u32));
            p.hash(crypto::RHO, fields, note.rho);
            p.jobs.extend(note_jobs);
            p.bindings
                .extend((0..WORDS).map(|j| (note.commitment + j, 26 + (n + i) * WORDS + j)));
            p.outputs.push(note);
        }
        p.carries = p.bit_registers(15);
        let operations: usize = p.jobs.iter().map(|h| h.message.len() / RATE).sum();
        p.rows = operations.max(512).next_power_of_two();
        p
    }
    fn operations(&self) -> usize {
        self.jobs.iter().map(|j| j.message.len() / RATE).sum()
    }

    fn trace(&self, s: &Statement, w: &Witness) -> Result<RowMajorMatrix<F>, Error> {
        if w.inputs.len() != self.inputs.len() || w.outputs.len() != self.outputs.len() {
            return Err(Error::Shape);
        }
        let public = s.public_values()?;
        let mut registers = vec![F::ZERO; self.registers];
        fn bits(r: &mut [F], start: usize, value: u64, count: usize) {
            for i in 0..count {
                r[start + i] = F::from_bool((value >> i) & 1 == 1)
            }
        }
        fn note(r: &mut [F], layout: &NoteLayout, n: &Note) -> Result<(), Error> {
            r[layout.owner..layout.owner + WORDS].copy_from_slice(&n.owner.fields()?);
            r[layout.rho..layout.rho + WORDS].copy_from_slice(&n.rho.fields()?);
            r[layout.rcm..layout.rcm + WORDS].copy_from_slice(&n.rcm.fields()?);
            bits(r, layout.bits, n.value, 64);
            Ok(())
        }
        for (layout, input) in self.inputs.iter().zip(&w.inputs) {
            if input.key.owner(s.network) != input.note.owner {
                return Err(Error::Witness("owner"));
            }
            note(&mut registers, &layout.note, &input.note)?;
            registers[layout.key..layout.key + WORDS].copy_from_slice(&input.key.fields());
            bits(
                &mut registers,
                layout.index,
                u64::from(input.path.index),
                DEPTH,
            );
            for (d, sibling) in input.path.siblings.iter().enumerate() {
                registers[layout.siblings + d * WORDS..layout.siblings + (d + 1) * WORDS]
                    .copy_from_slice(&sibling.fields()?);
            }
        }
        for (i, (layout, n)) in self.outputs.iter().zip(&w.outputs).enumerate() {
            if n.rho != crate::model::transfer_rho(s.network, s.nullifiers[0], i)? {
                return Err(Error::Witness("output rho"));
            }
            note(&mut registers, layout, n)?;
        }
        let incoming: u128 = w
            .inputs
            .iter()
            .map(|i| u128::from(i.note.value))
            .sum::<u128>()
            + u128::from(s.minted);
        let outgoing: u128 =
            w.outputs.iter().map(|o| u128::from(o.value)).sum::<u128>() + u128::from(s.fee);
        if incoming != outgoing {
            return Err(Error::Witness("balance"));
        }
        let mut carry = 0i64;
        for limb in 0..4 {
            let part = |value: u64| ((value >> (limb * 16)) & 65535) as i64;
            let delta = w.inputs.iter().map(|i| part(i.note.value)).sum::<i64>() + part(s.minted)
                - w.outputs.iter().map(|n| part(n.value)).sum::<i64>()
                - part(s.fee)
                + carry;
            if delta % 65536 != 0 {
                return Err(Error::Witness("balance limb"));
            }
            carry = delta / 65536;
            if limb < 3 {
                if !(-16..16).contains(&carry) {
                    return Err(Error::Witness("carry"));
                }
                bits(
                    &mut registers,
                    self.carries + limb * 5,
                    (carry + 16) as u64,
                    5,
                );
            }
        }
        if carry != 0 {
            return Err(Error::Witness("overflow"));
        }
        let (trace, registers) = self.hash_trace(registers, &public);
        for &(r, p) in &self.bindings {
            if registers[r] != public[p] {
                return Err(Error::Witness("commitment or membership"));
            }
        }
        Ok(trace)
    }

    // Low-level trace generation is separated from host witness checks so
    // adversarial tests can recompute a fully consistent hash trace for a
    // forged balance. The verifier must reject it using AIR constraints.
    fn hash_trace(&self, mut registers: Vec<F>, public: &[F]) -> (RowMajorMatrix<F>, Vec<F>) {
        let permutation = default_koalabear_poseidon2_32();
        let mut hash_inputs = Vec::with_capacity(self.rows);
        for job in &self.jobs {
            let mut state = [F::ZERO; WIDTH];
            for chunk in job.message.chunks_exact(RATE) {
                for i in 0..RATE {
                    state[i] += chunk[i].value(&registers, &public);
                }
                hash_inputs.push(state);
                permutation.permute_mut(&mut state);
            }
            registers[job.result..job.result + WORDS].copy_from_slice(&state[..WORDS]);
        }
        hash_inputs.resize(self.rows, [F::ZERO; WIDTH]);
        let hashes =
            generate_trace_rows::<F, GenericPoseidon2LinearLayersKoalaBear, 32, 3, 0, 4, ROUNDS>(
                hash_inputs,
                &crypto::constants(),
                LOG_BLOWUP,
            );
        let width = HASH_COLS + self.registers;
        let mut values = Vec::with_capacity(self.rows * width);
        for row in hashes.values.chunks_exact(HASH_COLS) {
            values.extend_from_slice(row);
            values.extend_from_slice(&registers);
        }
        (RowMajorMatrix::new(values, width), registers)
    }
}

/// Narrow the upstream Poseidon2 AIR to the prefix that contains its columns.
/// All its constraints are forwarded unchanged to the enclosing builder.
struct Prefix<'a, AB: AirBuilder> {
    inner: &'a mut AB,
    window: RowWindow<'a, AB::Var>,
}
impl<'a, AB: AirBuilder> AirBuilder for Prefix<'a, AB> {
    type F = AB::F;
    type Expr = AB::Expr;
    type Var = AB::Var;
    type PreprocessedWindow = AB::PreprocessedWindow;
    type MainWindow = RowWindow<'a, AB::Var>;
    type PublicVar = AB::PublicVar;
    type PeriodicVar = AB::PeriodicVar;
    fn main(&self) -> Self::MainWindow {
        self.window
    }
    fn preprocessed(&self) -> &Self::PreprocessedWindow {
        self.inner.preprocessed()
    }
    fn is_first_row(&self) -> Self::Expr {
        self.inner.is_first_row()
    }
    fn is_last_row(&self) -> Self::Expr {
        self.inner.is_last_row()
    }
    fn is_transition(&self) -> Self::Expr {
        self.inner.is_transition()
    }
    fn assert_zero<I: Into<Self::Expr>>(&mut self, x: I) {
        self.inner.assert_zero(x)
    }
    fn public_values(&self) -> &[Self::PublicVar] {
        self.inner.public_values()
    }
    fn periodic_values(&self) -> &[Self::PeriodicVar] {
        self.inner.periodic_values()
    }
}

struct TransferAir {
    program: Program,
    hash: HashAir,
}
impl TransferAir {
    fn new(n: usize, m: usize) -> Self {
        Self {
            program: Program::new(n, m),
            hash: HashAir::new(crypto::constants()),
        }
    }
}
impl BaseAir<F> for TransferAir {
    fn width(&self) -> usize {
        HASH_COLS + self.program.registers
    }
    fn num_public_values(&self) -> usize {
        self.program.public_count
    }
    fn num_periodic_columns(&self) -> usize {
        self.program.operations()
    }
    fn periodic_columns(&self) -> Cow<'_, [Vec<F>]> {
        Cow::Owned(
            (0..self.program.operations())
                .map(|i| {
                    let mut col = vec![F::ZERO; self.program.rows];
                    col[i] = F::ONE;
                    col
                })
                .collect(),
        )
    }
    fn max_constraint_degree(&self) -> Option<usize> {
        Some(3)
    }
}
impl<AB: AirBuilder<F = F>> Air<AB> for TransferAir {
    fn eval(&self, b: &mut AB) {
        let window = b.main();
        let local = window.current_slice();
        let next = window.next_slice();
        self.hash.eval(&mut Prefix {
            inner: b,
            window: RowWindow::from_two_rows(&local[..HASH_COLS], &next[..HASH_COLS]),
        });
        let cols: &Columns<AB::Var> = local[..HASH_COLS].borrow();
        let following: &Columns<AB::Var> = next[..HASH_COLS].borrow();
        let output = &cols.ending_full_rounds[3].post;
        let r = &local[HASH_COLS..];
        let nr = &next[HASH_COLS..];
        for i in 0..r.len() {
            b.when_transition().assert_eq(r[i], nr[i]);
        }
        for &bit in &self.program.bits {
            b.when_first_row().assert_bool(r[bit]);
        }
        let public = b.public_values().to_vec();
        for &(register, p) in &self.program.bindings {
            b.when_first_row().assert_eq(r[register], public[p]);
        }
        let selectors = b.periodic_values().to_vec();
        let mut row = 0;
        for job in &self.program.jobs {
            let chunks = job.message.chunks_exact(RATE).collect::<Vec<_>>();
            for (j, chunk) in chunks.iter().enumerate() {
                let selector: AB::Expr = selectors[row].into();
                if j == 0 {
                    for i in 0..RATE {
                        b.when(selector.dup())
                            .assert_eq(cols.inputs[i], chunk[i].expr::<AB>(r, &public));
                    }
                    for i in RATE..WIDTH {
                        b.when(selector.dup()).assert_zero(cols.inputs[i]);
                    }
                }
                if j + 1 < chunks.len() {
                    for i in 0..RATE {
                        b.when(selector.dup()).assert_eq(
                            following.inputs[i],
                            output[i] + chunks[j + 1][i].expr::<AB>(r, &public),
                        );
                    }
                    for i in RATE..WIDTH {
                        b.when(selector.dup())
                            .assert_eq(following.inputs[i], output[i]);
                    }
                } else {
                    for i in 0..WORDS {
                        b.when(selector.dup())
                            .assert_eq(output[i], r[job.result + i]);
                    }
                }
                row += 1;
            }
        }
        // Integer balance in base 2^16; every amount and signed carry is range
        // constrained. Bounds are far below the field modulus, so modular
        // wraparound cannot create money.
        let carry = |limb: usize| -> AB::Expr {
            (0..5)
                .map(|bit| r[self.program.carries + limb * 5 + bit] * F::from_u32(1 << bit))
                .sum::<AB::Expr>()
                - F::from_u32(16)
        };
        for limb in 0..4 {
            let mut balance: AB::Expr = public[6 + limb].into();
            balance -= public[2 + limb].into();
            for input in &self.program.inputs {
                balance += Source::Limb(input.note.bits + limb * 16).expr::<AB>(r, &public);
            }
            for out in &self.program.outputs {
                balance -= Source::Limb(out.bits + limb * 16).expr::<AB>(r, &public);
            }
            if limb > 0 {
                balance += carry(limb - 1);
            }
            if limb < 3 {
                balance -= carry(limb) * F::from_u32(65536);
            }
            b.when_first_row().assert_zero(balance);
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProofEnvelope {
    pub version: u32,
    pub proof: Vec<u8>,
}

pub fn prove_transfer(statement: &Statement, witness: &Witness) -> Result<ProofEnvelope, Error> {
    let public = statement.public_values()?;
    if statement.nullifiers.is_empty() {
        return Err(Error::Shape);
    }
    let air = TransferAir::new(statement.nullifiers.len(), statement.commitments.len());
    let trace = air.program.trace(statement, witness)?;
    let proof =
        prove(&config()?, &air, trace, &public).map_err(|e| Error::Proof(format!("{e:?}")))?;
    let bytes = postcard::to_allocvec(&proof).map_err(|e| Error::Proof(e.to_string()))?;
    if bytes.len() > MAX_PROOF_BYTES {
        return Err(Error::Proof("proof exceeds protocol limit".into()));
    }
    Ok(ProofEnvelope {
        version: 2,
        proof: bytes,
    })
}

pub fn verify_transfer(statement: &Statement, envelope: &ProofEnvelope) -> Result<(), Error> {
    let public = statement.public_values()?;
    if envelope.version != 2 || envelope.proof.len() > MAX_PROOF_BYTES {
        return Err(Error::Encoding);
    }
    if statement.nullifiers.is_empty() {
        return Err(Error::Shape);
    }
    let air = TransferAir::new(statement.nullifiers.len(), statement.commitments.len());
    let (proof, rest): (Proof<Config>, &[u8]) =
        postcard::take_from_bytes(&envelope.proof).map_err(|_| Error::Encoding)?;
    if !rest.is_empty()
        || postcard::to_allocvec(&proof).map_err(|_| Error::Encoding)? != envelope.proof
    {
        return Err(Error::Encoding);
    }
    // Reject alternative trace lengths: program selectors have a fixed period.
    if proof.degree_bits != air.program.rows.ilog2() as usize + 1 {
        return Err(Error::Shape);
    }
    verify(&config()?, &air, &proof, &public).map_err(|e| Error::Proof(format!("{e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Digest;

    fn fixture() -> (Statement, Witness) {
        let network = 3;
        let key = SpendKey::from_seed(&[1; 32], network, 0).unwrap();
        let other = SpendKey::from_seed(&[2; 32], network, 0).unwrap();
        let note = Note {
            owner: key.owner(network),
            rho: Digest([12; WORDS]),
            rcm: Digest([13; WORDS]),
            value: 1000,
        };
        let first = note.nullifier(&key, network).unwrap();
        let rho = |i| crate::model::transfer_rho(network, first, i).unwrap();
        let path = MerklePath {
            index: 17,
            siblings: std::array::from_fn(|i| Digest([i as u32 + 100; WORDS])),
        };
        let anchor = path
            .root(network, note.commitment(network).unwrap())
            .unwrap();
        let outputs = vec![
            Note {
                owner: other.owner(network),
                rho: rho(0),
                rcm: Digest([21; WORDS]),
                value: 600,
            },
            Note {
                owner: key.owner(network),
                rho: rho(1),
                rcm: Digest([22; WORDS]),
                value: 395,
            },
        ];
        let witness = Witness {
            inputs: vec![Input { key, note, path }],
            outputs,
        };
        let statement = Statement::from_witness(network, anchor, 5, 0, [7; 32], &witness).unwrap();
        (statement, witness)
    }

    #[test]
    fn complete_transfer_binds_membership_balance_authority_and_effects() {
        let (s, w) = fixture();
        let started = std::time::Instant::now();
        let proof = prove_transfer(&s, &w).unwrap();
        eprintln!(
            "private transfer: {} bytes, {:?} proving",
            proof.proof.len(),
            started.elapsed()
        );
        verify_transfer(&s, &proof).unwrap();
        for field in 0..7 {
            let mut tampered = s.clone();
            match field {
                0 => tampered.fee += 1,
                1 => tampered.anchor.0[0] += 1,
                2 => tampered.nullifiers[0].0[0] += 1,
                3 => tampered.commitments[0].0[0] += 1,
                4 => tampered.binding[0] ^= 1,
                5 => tampered.network = 2,
                _ => tampered.version = 1,
            }
            assert!(
                verify_transfer(&tampered, &proof).is_err(),
                "public field {field}"
            );
        }
        let mut broken = proof.clone();
        let last = broken.proof.len() - 1;
        broken.proof[last] ^= 1;
        assert!(verify_transfer(&s, &broken).is_err());
        let mut extra = proof.clone();
        extra.proof.push(0);
        assert!(verify_transfer(&s, &extra).is_err());
        let second = prove_transfer(&s, &w).unwrap();
        verify_transfer(&s, &second).unwrap();
        assert_ne!(proof.proof, second.proof, "independent hiding randomness");
    }

    #[test]
    fn witness_rejects_wrong_owner_inflation_and_foreign_tree() {
        let (s, mut w) = fixture();
        w.outputs[0].value += 1;
        assert!(prove_transfer(&s, &w).is_err());
        let (s, mut w) = fixture();
        w.inputs[0].key = SpendKey::from_seed(&[99; 32], 3, 0).unwrap();
        assert!(prove_transfer(&s, &w).is_err());
        let (s, mut w) = fixture();
        w.inputs[0].path.siblings[0].0[0] += 1;
        assert!(prove_transfer(&s, &w).is_err());
        // A sender cannot pick an output rho (Faerie Gold): a reused one,
        // even with every public commitment recomputed, is refused.
        let (_, mut w) = fixture();
        w.outputs[1].rho = w.outputs[0].rho;
        let s = Statement::from_witness(3, s.anchor, 5, 0, [7; 32], &w).unwrap();
        assert!(matches!(
            prove_transfer(&s, &w),
            Err(Error::Witness("output rho"))
        ));
    }

    #[test]
    fn air_itself_rejects_forged_private_registers() {
        let (s, w) = fixture();
        let air = TransferAir::new(1, 2);
        let public = s.public_values().unwrap();
        let original = air.program.trace(&s, &w).unwrap();
        p3_air::check_constraints(&air, &original, &public);
        // Tamper every row consistently, bypassing all host witness checks.
        // Ownership, balance/ranges and Merkle path are enforced by the AIR.
        for register in [
            air.program.inputs[0].key,
            air.program.inputs[0].note.bits,
            air.program.inputs[0].note.rcm,
            air.program.inputs[0].index,
            air.program.inputs[0].siblings,
            air.program.outputs[0].bits,
            air.program.outputs[0].rho,
            air.program.outputs[0].rcm,
            air.program.carries,
        ] {
            let mut trace = original.clone();
            for row in trace.values.chunks_exact_mut(trace.width) {
                row[HASH_COLS + register] += F::ONE;
            }
            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                p3_air::check_constraints(&air, &trace, &public)
            }))
            .is_err());
        }
    }

    #[test]
    fn air_rejects_inflation_after_recomputing_all_hashes_and_public_outputs() {
        let (s, w) = fixture();
        let air = TransferAir::new(1, 2);
        let public = s.public_values().unwrap();
        let trace = air.program.trace(&s, &w).unwrap();
        assert!(p3_air::check_all_constraints(&air, &trace, &public, Some(8)).is_ok());
        // Change the first output 600 -> 601, preserving boolean amount bits,
        // and honestly recompute every Poseidon permutation and its public cm.
        let mut registers = trace.values[HASH_COLS..trace.width].to_vec();
        let output = air.program.outputs[0].bits;
        assert_eq!(registers[output], F::ZERO);
        registers[output] = F::ONE;
        let (forged, registers) = air.program.hash_trace(registers, &public);
        let mut forged_public = public.clone();
        for &(r, p) in &air.program.bindings {
            forged_public[p] = registers[r];
        }
        assert!(!p3_air::check_all_constraints(&air, &forged, &forged_public, Some(8)).is_ok());
        // A bogus bit value cannot encode an amount outside the u64 range.
        let mut registers = trace.values[HASH_COLS..trace.width].to_vec();
        registers[output] = F::from_u32(2);
        let (forged, registers) = air.program.hash_trace(registers, &public);
        for &(r, p) in &air.program.bindings {
            forged_public[p] = registers[r];
        }
        assert!(!p3_air::check_all_constraints(&air, &forged, &forged_public, Some(8)).is_ok());
    }

    #[test]
    fn air_rejects_a_chosen_output_rho_after_recomputing_every_hash() {
        let (s, w) = fixture();
        let air = TransferAir::new(1, 2);
        let public = s.public_values().unwrap();
        let trace = air.program.trace(&s, &w).unwrap();
        // Overwrite the second output's rho with the first one's and redo
        // all permutations: the RHO job's result no longer matches.
        let mut registers = trace.values[HASH_COLS..trace.width].to_vec();
        let (from, to) = (air.program.outputs[0].rho, air.program.outputs[1].rho);
        for i in 0..WORDS {
            registers[to + i] = registers[from + i];
        }
        let (mut forged, registers) = air.program.hash_trace(registers, &public);
        // hash_trace writes the true rho back; force the chosen one again.
        for row in forged.values.chunks_exact_mut(forged.width) {
            for i in 0..WORDS {
                row[HASH_COLS + to + i] = registers[from + i];
            }
        }
        let mut forged_public = public.clone();
        for &(r, p) in &air.program.bindings {
            forged_public[p] = registers[r];
        }
        assert!(!p3_air::check_all_constraints(&air, &forged, &forged_public, Some(8)).is_ok());
    }

    #[test]
    fn parameter_soundness_estimates_cover_every_supported_shape() {
        use p3_field::coset::TwoAdicMultiplicativeCoset;
        use p3_uni_stark::{AirLayout, OpeningShape, ProvenSecurity, StarkSecurityParams};
        // This is the upstream classical algebraic estimator with an explicit
        // 128-bit collision-resistance assumption. It is not a PQ/ZK audit.
        for n in 1..=MAX_INPUTS {
            for m in 1..=MAX_OUTPUTS {
                let air = TransferAir::new(n, m);
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
                    TwoAdicMultiplicativeCoset::new(F::ONE, air.program.rows.ilog2() as usize)
                        .unwrap(),
                    247,
                    128,
                    2,
                    OpeningShape::hiding(RANDOM_CODEWORDS),
                    fri.grinding_sites(),
                );
                let bits = ProvenSecurity::compute_from_proof(
                    air.program.rows.ilog2() as usize + 1,
                    &params,
                )
                .security_bits();
                assert!(
                    bits >= 100,
                    "{n} inputs, {m} outputs: only {bits} estimated classical bits"
                );
                if (n, m) == (1, 2) || (n, m) == (8, 8) {
                    eprintln!("shape {n}x{m}: {bits} estimated classical soundness bits (explicit hash assumption)");
                }
            }
        }
    }

    #[test]
    fn maximum_shape_and_u64_carries_prove_and_verify() {
        use crate::ledger::NoteTree;
        let key = SpendKey::from_seed(&[42; 32], 3, 0).unwrap();
        let mut tree = NoteTree::new(3).unwrap();
        let notes: Vec<_> = (0..MAX_INPUTS)
            .map(|i| Note {
                owner: key.owner(3),
                rho: Digest([100 + i as u32; WORDS]),
                rcm: Digest([150 + i as u32; WORDS]),
                value: u64::MAX - i as u64 * 1000,
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
        let outputs = notes
            .iter()
            .enumerate()
            .map(|(i, note)| Note {
                owner: note.owner,
                rho: crate::model::transfer_rho(3, first, i).unwrap(),
                rcm: Digest([200 + i as u32; WORDS]),
                value: note.value - if i == 0 { 255 } else { 0 },
            })
            .collect();
        let witness = Witness { inputs, outputs };
        let statement =
            Statement::from_witness(3, tree.root(), 255, 0, [42; 32], &witness).unwrap();
        let start = std::time::Instant::now();
        let proof = prove_transfer(&statement, &witness).unwrap();
        eprintln!(
            "maximum 8x8 transfer: {} proof bytes, {:?} proving",
            proof.proof.len(),
            start.elapsed()
        );
        verify_transfer(&statement, &proof).unwrap();
    }
}
