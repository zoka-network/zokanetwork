//! The transfer proof of version 4: the relation of version 3, proved on a
//! narrow trace.
//!
//! Version 3 keeps every value of the witness in a column of its own, the
//! same on every row, and each of the 160 FRI queries opens a whole row. A
//! payment that spends two notes made a proof of 2.59 MB and one that spends
//! eight 7.85 MB: about 0.88 MB more per note, the ~1,100 columns of its
//! Merkle path (`docs/PRUEBA-V4.md`). Here the same program runs on few
//! columns. Each value sits in a column from the row where a job writes it,
//! or where the witness loads it, to the last row that reads it; then the
//! column carries another value. Which rows keep, write or load each column
//! is fixed by the payment's shape and committed as periodic columns: the
//! prover chooses none of it.
//!
//! What changes besides the layout:
//! - the integer balance adds each note's 16-bit limbs into four running
//!   sums on the row that reads its value, and checks them on one row after
//!   every job, against the fee, the minted amount and the carries;
//! - an input's anchor check takes its value's limb sum from a column kept
//!   from that row to the end of its path;
//! - commitments and nullifiers bind to the public values on the rows that
//!   compute them.
//!
//! The jobs, their messages and domains, the padding inputs and the public
//! values are version 3's [`Program`], unchanged: both circuits prove the
//! same statements.
use super::*;

/// What a proof of this circuit declares in its envelope.
pub const PROOF_VERSION: u32 = 4;
/// The transcript of a version 4 proof starts from this: never one of v3.
const TRANSCRIPT_DOMAIN: &[u8] = b"ZOKA QUANTIC transfer proof v4";
/// The running sums of the balance, one per 16-bit limb.
const LIMBS: usize = 4;

/// Where a register of the program lives: one column, from row `from` to row
/// `to` included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Interval {
    pub(super) column: usize,
    pub(super) from: usize,
    pub(super) to: usize,
}

/// How the program is laid out on the narrow trace.
pub(super) struct Layout {
    pub(super) rows: usize,
    /// The row after every job: where the balance is checked.
    pub(super) last: usize,
    /// Per job: its first and last row.
    pub(super) jobs: Vec<(usize, usize)>,
    /// Per register of the program: where it is kept, if any row reads it.
    pub(super) place: Vec<Option<Interval>>,
    /// Per input: its value's limb sum, from the row after its limbs are
    /// read to the last row of its path.
    pub(super) worth: Vec<Interval>,
    /// Per note, inputs first: the row that reads its value's limbs.
    pub(super) limbs: Vec<usize>,
    /// Columns of field elements and of bits, after the hash and the sums.
    pub(super) words: usize,
    pub(super) bits: usize,
    /// Columns that keep their value into the next row on the same rows,
    /// and those rows: one periodic column each.
    pub(super) keeps: Vec<(Vec<usize>, Vec<bool>)>,
    /// The rows where the running sums keep their value.
    pub(super) sums_keep: Vec<bool>,
}

impl Layout {
    /// The first column after the hash: the running sums, then the words,
    /// then the bits.
    const SUMS: usize = HASH_COLS;
    fn first_word(&self) -> usize {
        Self::SUMS + LIMBS
    }
    fn first_bit(&self) -> usize {
        self.first_word() + self.words
    }
    pub(super) fn width(&self) -> usize {
        self.first_bit() + self.bits
    }
    /// The column of register `r`, which the caller reads where it lives.
    fn column(&self, r: usize) -> usize {
        self.place[r]
            .expect("the layout keeps every register a job reads")
            .column
    }

    pub(super) fn new(program: &Program) -> Self {
        // Every job takes one row per chunk of its message, in order.
        let mut jobs = Vec::with_capacity(program.jobs.len());
        let mut row = 0;
        for job in &program.jobs {
            let chunks = job.message.len() / RATE;
            jobs.push((row, row + chunks - 1));
            row += chunks;
        }
        let last = row;
        // A row after the last job always exists: the balance row, and the
        // row the last job writes its result into.
        let rows = (last + 1).max(512).next_power_of_two();

        let mut producer = vec![None; program.registers];
        for (j, job) in program.jobs.iter().enumerate() {
            for word in 0..WORDS {
                producer[job.result + word] = Some(j);
            }
        }
        // The rows that read each register. A job reads its first chunk on
        // its first row, and chunk k + 1 on the row that absorbs chunk k: the
        // constraint that links the two rows is written there.
        let mut uses = vec![Vec::new(); program.registers];
        for (j, job) in program.jobs.iter().enumerate() {
            let first = jobs[j].0;
            for (k, chunk) in job.message.chunks_exact(RATE).enumerate() {
                let at = first + k.saturating_sub(1);
                for source in chunk {
                    for r in source.registers() {
                        uses[r].push(at);
                    }
                }
            }
        }
        // The carries of the balance are read on its row.
        for r in program.carries..program.carries + 15 {
            uses[r].push(last);
        }
        // The row that reads each note's limbs: inputs first, then outputs.
        let notes: Vec<usize> = program
            .inputs
            .iter()
            .map(|input| input.note.bits)
            .chain(program.outputs.iter().map(|output| output.bits))
            .collect();
        let limbs: Vec<usize> = notes
            .iter()
            .map(|&bits| {
                let reads = &uses[bits];
                assert_eq!(reads.len(), 1, "a note's value is read on one row");
                reads[0]
            })
            .collect();

        // What each register needs, as an interval; then the limb sums.
        let is_bit = {
            let mut is_bit = vec![false; program.registers];
            for &bit in &program.bits {
                is_bit[bit] = true;
            }
            is_bit
        };
        struct Need {
            bit: bool,
            from: usize,
            to: usize,
        }
        let mut needs: Vec<(usize, Need)> = Vec::new();
        for r in 0..program.registers {
            let Some(&first_use) = uses[r].iter().min() else {
                continue;
            };
            let last_use = *uses[r].iter().max().unwrap();
            let from = match producer[r] {
                Some(j) => jobs[j].1 + 1,
                None => first_use,
            };
            assert!(
                from <= first_use,
                "register {r} is read on row {first_use} before it is written on row {from}"
            );
            needs.push((
                r,
                Need {
                    bit: is_bit[r],
                    from,
                    to: last_use,
                },
            ));
        }
        // Inputs only: (root's last row, limb row).
        let worth_spans: Vec<(usize, usize)> = program
            .anchors
            .iter()
            .zip(&limbs)
            .map(|(&(root, _), &limb_row)| {
                let job = producer[root].expect("a root is a job's result");
                let end = jobs[job].1;
                assert!(limb_row < end, "an input's value is read before its path ends");
                (limb_row + 1, end)
            })
            .collect();

        // Greedy colouring of the intervals, by start: a column is free for a
        // value that starts after its last value ended.
        enum Owner {
            Register(usize),
            Worth(usize),
        }
        let mut items: Vec<(bool, usize, usize, Owner)> = needs
            .into_iter()
            .map(|(r, need)| (need.bit, need.from, need.to, Owner::Register(r)))
            .collect();
        items.extend(
            worth_spans
                .iter()
                .enumerate()
                .map(|(k, &(from, to))| (false, from, to, Owner::Worth(k))),
        );
        items.sort_by_key(|item| (item.1, item.2));
        let mut word_free: Vec<usize> = Vec::new();
        let mut bit_free: Vec<usize> = Vec::new();
        let mut place = vec![None; program.registers];
        let mut worth = vec![None; worth_spans.len()];
        let mut assigned: Vec<(bool, usize, usize, usize)> = Vec::new();
        for (bit, from, to, owner) in items {
            let pool = if bit { &mut bit_free } else { &mut word_free };
            let index = match pool.iter().position(|&free_from| free_from <= from) {
                Some(index) => index,
                None => {
                    pool.push(0);
                    pool.len() - 1
                }
            };
            pool[index] = to + 1;
            assigned.push((bit, index, from, to));
            let interval = Interval {
                column: index,
                from,
                to,
            };
            match owner {
                Owner::Register(r) => place[r] = Some((bit, interval)),
                Owner::Worth(k) => worth[k] = Some(interval),
            }
        }
        let words = word_free.len();
        let bits = bit_free.len();
        let first_word = Self::SUMS + LIMBS;
        let first_bit = first_word + words;
        let absolute = |bit: bool, column: usize| if bit { first_bit + column } else { first_word + column };
        let place: Vec<Option<Interval>> = place
            .into_iter()
            .map(|slot| {
                slot.map(|(bit, interval)| Interval {
                    column: absolute(bit, interval.column),
                    ..interval
                })
            })
            .collect();
        let worth: Vec<Interval> = worth
            .into_iter()
            .map(|interval| {
                let interval = interval.expect("every input has its limb sum");
                Interval {
                    column: absolute(false, interval.column),
                    ..interval
                }
            })
            .collect();

        // Where each column keeps its value into the next row: inside each of
        // its intervals. Columns with the same rows share a periodic column.
        let mut patterns: std::collections::BTreeMap<usize, Vec<bool>> = Default::default();
        for &(bit, column, from, to) in &assigned {
            let pattern = patterns
                .entry(absolute(bit, column))
                .or_insert_with(|| vec![false; rows]);
            for keep in &mut pattern[from..to] {
                *keep = true;
            }
        }
        let mut grouped: std::collections::BTreeMap<Vec<bool>, Vec<usize>> = Default::default();
        for (column, pattern) in patterns {
            grouped.entry(pattern).or_default().push(column);
        }
        let mut keeps: Vec<(Vec<usize>, Vec<bool>)> = grouped
            .into_iter()
            .filter(|(pattern, _)| pattern.iter().any(|&keep| keep))
            .map(|(pattern, columns)| (columns, pattern))
            .collect();
        keeps.sort_by_key(|(columns, _)| columns[0]);
        // The running sums keep their value up to the balance row, except on
        // the rows that add a note's limbs.
        let mut sums_keep = vec![false; rows];
        for keep in &mut sums_keep[..last] {
            *keep = true;
        }
        for &row in &limbs {
            sums_keep[row] = false;
        }

        Self {
            rows,
            last,
            jobs,
            place,
            worth,
            limbs,
            words,
            bits,
            keeps,
            sums_keep,
        }
    }

    /// Periodic columns: one per row that runs an operation or checks the
    /// balance (a single 1), then the keep rows of each group, then those of
    /// the running sums.
    fn periodic(&self) -> Vec<Vec<F>> {
        let one_hot = |row: usize| {
            let mut column = vec![F::ZERO; self.rows];
            column[row] = F::ONE;
            column
        };
        let flags = |rows: &[bool]| rows.iter().map(|&keep| F::from_bool(keep)).collect();
        (0..=self.last)
            .map(one_hot)
            .chain(self.keeps.iter().map(|(_, rows)| flags(rows)))
            .chain(std::iter::once(flags(&self.sums_keep)))
            .collect()
    }
    fn keep_base(&self) -> usize {
        self.last + 1
    }
    fn sums_keep_index(&self) -> usize {
        self.keep_base() + self.keeps.len()
    }
}

impl Source {
    /// The registers this source reads.
    fn registers(&self) -> Vec<usize> {
        match *self {
            Self::Constant(_) | Self::Public(_) => vec![],
            Self::Register(i) => vec![i],
            Self::Limb(i) => (i..i + 16).collect(),
            Self::Select { bit, zero, one } => vec![bit, zero, one],
        }
    }
    /// [`Source::expr`] on the narrow trace: each register from its column.
    fn narrow_expr<AB: AirBuilder<F = F>>(
        &self,
        layout: &Layout,
        row: &[AB::Var],
        public: &[AB::PublicVar],
    ) -> AB::Expr {
        let at = |r: usize| -> AB::Expr { row[layout.column(r)].into() };
        match *self {
            Self::Constant(x) => AB::Expr::from(F::from_u32(x)),
            Self::Register(i) => at(i),
            Self::Public(i) => public[i].into(),
            Self::Limb(i) => (0..16).map(|b| at(i + b) * F::from_u32(1 << b)).sum(),
            Self::Select { bit, zero, one } => at(zero) + at(bit) * (at(one) - at(zero)),
        }
    }
}

/// The limb `l` of a note's value, from its bits in `row`.
fn limb_expr<AB: AirBuilder<F = F>>(layout: &Layout, row: &[AB::Var], bits: usize, l: usize) -> AB::Expr {
    (0..16)
        .map(|b| row[layout.column(bits + l * 16 + b)] * F::from_u32(1 << b))
        .sum()
}

pub(super) struct NarrowAir {
    pub(super) program: Program,
    pub(super) layout: Layout,
    hash: HashAir,
    /// Per job: the public values its result binds to, word by word.
    bound: Vec<Vec<(usize, usize)>>,
    /// Per job: the input whose root it computes, if it ends a path.
    roots: Vec<Option<usize>>,
}

impl NarrowAir {
    pub(super) fn new(n: usize, m: usize) -> Self {
        let program = Program::new(n, m);
        let layout = Layout::new(&program);
        let mut owner = vec![None; program.registers];
        for (j, job) in program.jobs.iter().enumerate() {
            for word in 0..WORDS {
                owner[job.result + word] = Some((j, word));
            }
        }
        let mut bound = vec![Vec::new(); program.jobs.len()];
        for &(register, public) in &program.bindings {
            let (job, word) = owner[register].expect("a bound register is a job's result");
            bound[job].push((word, public));
        }
        let mut roots = vec![None; program.jobs.len()];
        for (input, &(root, _)) in program.anchors.iter().enumerate() {
            let (job, word) = owner[root].expect("a root is a job's result");
            assert_eq!(word, 0, "a root is a whole result");
            roots[job] = Some(input);
        }
        Self {
            program,
            layout,
            hash: HashAir::new(crypto::constants()),
            bound,
            roots,
        }
    }

    /// The narrow trace of a valid witness. The host checks are version 3's.
    pub(super) fn trace(&self, s: &Statement, w: &Witness) -> Result<RowMajorMatrix<F>, Error> {
        let (mut registers, public) = self.program.witness(s, w)?;
        let mut hash_inputs = self.program.run_jobs(&mut registers, &public);
        self.program.check(&registers, &public, w)?;
        Ok(self.lay_out(&registers, &mut hash_inputs))
    }

    /// The trace of registers whose jobs already ran, with their hash inputs.
    pub(super) fn lay_out(&self, registers: &[F], hash_inputs: &mut Vec<[F; WIDTH]>) -> RowMajorMatrix<F> {
        let layout = &self.layout;
        hash_inputs.resize(layout.rows, [F::ZERO; WIDTH]);
        let hashes = generate_trace_rows::<F, GenericPoseidon2LinearLayersKoalaBear, 32, 3, 0, 4, ROUNDS>(
            std::mem::take(hash_inputs),
            &crypto::constants(),
            LOG_BLOWUP,
        );
        let width = layout.width();
        let mut values = vec![F::ZERO; layout.rows * width];
        for (row, hashed) in hashes.values.chunks_exact(HASH_COLS).enumerate() {
            values[row * width..row * width + HASH_COLS].copy_from_slice(hashed);
        }
        for (r, interval) in layout.place.iter().enumerate() {
            if let Some(interval) = interval {
                for row in interval.from..=interval.to {
                    values[row * width + interval.column] = registers[r];
                }
            }
        }
        let limb = |bits: usize, l: usize| -> F {
            (0..16)
                .map(|b| registers[bits + l * 16 + b] * F::from_u32(1 << b))
                .sum()
        };
        for (input, interval) in self.program.inputs.iter().zip(&layout.worth) {
            let worth: F = (0..LIMBS).map(|l| limb(input.note.bits, l)).sum();
            for row in interval.from..=interval.to {
                values[row * width + interval.column] = worth;
            }
        }
        // The running sums: on each row, what the notes read above it add.
        let notes: Vec<(usize, F)> = self
            .program
            .inputs
            .iter()
            .map(|input| (input.note.bits, F::ONE))
            .chain(self.program.outputs.iter().map(|output| (output.bits, -F::ONE)))
            .collect();
        let mut sums = [F::ZERO; LIMBS];
        let mut at = 0;
        let mut order: Vec<(usize, usize)> = layout.limbs.iter().copied().zip(0..).collect();
        order.sort();
        for row in 0..layout.rows {
            for (l, sum) in sums.iter().enumerate() {
                values[row * width + Layout::SUMS + l] = *sum;
            }
            while at < order.len() && order[at].0 == row {
                let (bits, sign) = notes[order[at].1];
                for (l, sum) in sums.iter_mut().enumerate() {
                    *sum += sign * limb(bits, l);
                }
                at += 1;
            }
        }
        RowMajorMatrix::new(values, width)
    }
}

impl BaseAir<F> for NarrowAir {
    fn width(&self) -> usize {
        self.layout.width()
    }
    fn num_public_values(&self) -> usize {
        self.program.public_count
    }
    fn num_periodic_columns(&self) -> usize {
        self.layout.sums_keep_index() + 1
    }
    fn periodic_columns(&self) -> Cow<'_, [Vec<F>]> {
        Cow::Owned(self.layout.periodic())
    }
    fn max_constraint_degree(&self) -> Option<usize> {
        Some(3)
    }
}

impl<AB: AirBuilder<F = F>> Air<AB> for NarrowAir {
    fn eval(&self, b: &mut AB) {
        let layout = &self.layout;
        let program = &self.program;
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
        let public = b.public_values().to_vec();
        let periodic = b.periodic_values().to_vec();

        // Bit columns hold bits on every row.
        for c in layout.first_bit()..layout.width() {
            b.assert_bool(local[c]);
        }
        // A column keeps its value into the next row while a value lives in
        // it: the prover cannot change a value between the rows that read it.
        for (group, (columns, _)) in layout.keeps.iter().enumerate() {
            let keep: AB::Expr = periodic[layout.keep_base() + group].into();
            for &c in columns {
                b.assert_zero(keep.dup() * (next[c].into() - local[c].into()));
            }
        }
        let sums_keep: AB::Expr = periodic[layout.sums_keep_index()].into();
        for l in 0..LIMBS {
            let c = Layout::SUMS + l;
            b.assert_zero(sums_keep.dup() * (next[c].into() - local[c].into()));
            b.when_first_row().assert_zero(local[c]);
        }

        for (j, job) in program.jobs.iter().enumerate() {
            let first = layout.jobs[j].0;
            let chunks = job.message.chunks_exact(RATE).collect::<Vec<_>>();
            for (k, chunk) in chunks.iter().enumerate() {
                let selector: AB::Expr = periodic[first + k].into();
                if k == 0 {
                    for i in 0..RATE {
                        b.when(selector.dup()).assert_eq(
                            cols.inputs[i],
                            chunk[i].narrow_expr::<AB>(layout, local, &public),
                        );
                    }
                    for i in RATE..WIDTH {
                        b.when(selector.dup()).assert_zero(cols.inputs[i]);
                    }
                }
                if k + 1 < chunks.len() {
                    for i in 0..RATE {
                        b.when(selector.dup()).assert_eq(
                            following.inputs[i],
                            output[i] + chunks[k + 1][i].narrow_expr::<AB>(layout, local, &public),
                        );
                    }
                    for i in RATE..WIDTH {
                        b.when(selector.dup())
                            .assert_eq(following.inputs[i], output[i]);
                    }
                    continue;
                }
                // The last row of the job: its result goes into the columns
                // of the rows that read it, and to the public values it binds.
                for word in 0..WORDS {
                    if let Some(interval) = layout.place[job.result + word] {
                        b.when(selector.dup())
                            .assert_eq(next[interval.column], output[word]);
                    }
                }
                for &(word, p) in &self.bound[j] {
                    b.when(selector.dup()).assert_eq(output[word], public[p]);
                }
                // (root − anchor) · value = 0: in the anchor's tree unless the
                // note is worth nothing (the limb sum is below 2^18, so it is
                // zero only for a zero value), as in version 3.
                if let Some(input) = self.roots[j] {
                    let worth: AB::Expr = local[layout.worth[input].column].into();
                    for word in 0..WORDS {
                        let gap: AB::Expr = output[word].into() - public[10 + word].into();
                        b.assert_zero(selector.dup() * gap * worth.dup());
                    }
                }
            }
        }

        // The row that reads a note's value adds its limbs to the running
        // sums (outputs subtract) and, for an input, writes its limb sum.
        let notes = program
            .inputs
            .iter()
            .map(|input| (input.note.bits, true))
            .chain(program.outputs.iter().map(|output| (output.bits, false)));
        for (index, (bits, spent)) in notes.enumerate() {
            let selector: AB::Expr = periodic[layout.limbs[index]].into();
            let mut total = AB::Expr::ZERO;
            for l in 0..LIMBS {
                let limb = limb_expr::<AB>(layout, local, bits, l);
                total += limb.dup();
                let c = Layout::SUMS + l;
                let change: AB::Expr = next[c].into() - local[c].into();
                let delta = if spent { change - limb } else { change + limb };
                b.assert_zero(selector.dup() * delta);
            }
            if spent {
                let c = layout.worth[index].column;
                b.assert_zero(selector.dup() * (next[c].into() - total));
            }
        }

        // The balance, in base 2^16 with signed carries, on the row after
        // every job: Σ inputs + minted = Σ outputs + fee as integers.
        let selector: AB::Expr = periodic[layout.last].into();
        let carry = |limb: usize| -> AB::Expr {
            (0..5)
                .map(|bit| local[layout.column(program.carries + limb * 5 + bit)] * F::from_u32(1 << bit))
                .sum::<AB::Expr>()
                - F::from_u32(16)
        };
        for l in 0..LIMBS {
            let mut balance: AB::Expr = local[Layout::SUMS + l].into();
            balance += public[6 + l].into();
            balance -= public[2 + l].into();
            if l > 0 {
                balance += carry(l - 1);
            }
            if l < 3 {
                balance -= carry(l) * F::from_u32(65536);
            }
            b.assert_zero(selector.dup() * balance);
        }
    }
}

pub(super) fn config() -> Result<Config, Error> {
    config_with(TRANSCRIPT_DOMAIN)
}

pub fn prove(statement: &Statement, witness: &Witness) -> Result<ProofEnvelope, Error> {
    let public = statement.public_values()?;
    if statement.nullifiers.is_empty() {
        return Err(Error::Shape);
    }
    let air = NarrowAir::new(statement.nullifiers.len(), statement.commitments.len());
    let trace = air.trace(statement, witness)?;
    let proof = p3_uni_stark::prove(&config()?, &air, trace, &public)
        .map_err(|e| Error::Proof(format!("{e:?}")))?;
    let bytes = postcard::to_allocvec(&proof).map_err(|e| Error::Proof(e.to_string()))?;
    if bytes.len() > MAX_PROOF_BYTES {
        return Err(Error::Proof("proof exceeds protocol limit".into()));
    }
    Ok(ProofEnvelope {
        version: PROOF_VERSION,
        proof: bytes,
    })
}

pub fn verify(statement: &Statement, envelope: &ProofEnvelope) -> Result<(), Error> {
    let public = statement.public_values()?;
    if envelope.version != PROOF_VERSION || envelope.proof.len() > MAX_PROOF_BYTES {
        return Err(Error::Encoding);
    }
    if statement.nullifiers.is_empty() {
        return Err(Error::Shape);
    }
    let air = NarrowAir::new(statement.nullifiers.len(), statement.commitments.len());
    guarded(|| {
        let (proof, rest): (Proof<Config>, &[u8]) =
            postcard::take_from_bytes(&envelope.proof).map_err(|_| Error::Encoding)?;
        if !rest.is_empty()
            || postcard::to_allocvec(&proof).map_err(|_| Error::Encoding)? != envelope.proof
        {
            return Err(Error::Encoding);
        }
        // The periodic columns have the layout's period: no other length.
        if proof.degree_bits != air.layout.rows.ilog2() as usize + 1 {
            return Err(Error::Shape);
        }
        p3_uni_stark::verify(&config()?, &air, &proof, &public)
            .map_err(|e| Error::Proof(format!("{e:?}")))
    })
}

#[cfg(test)]
mod tests;
