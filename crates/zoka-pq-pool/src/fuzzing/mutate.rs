//! A small deterministic generator and the classic byte-level mutations: the
//! same seed always produces the same inputs, so a failure is reproduced by
//! running the same test again.

/// SplitMix64. Not a cryptographic generator: it only has to be repeatable.
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// A number below `bound`, which is not zero.
    pub fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

const INTERESTING: [u8; 8] = [0x00, 0x01, 0x7f, 0x80, 0xff, 0x10, 0x20, 0xfe];

/// A span of at most `most` bytes inside `len`, which is not zero.
fn span(rng: &mut Rng, len: usize, most: usize) -> (usize, usize) {
    let start = rng.below(len);
    let size = 1 + rng.below(most.min(len - start));
    (start, size)
}

/// One to four edits of `data`. `donor` lends spans of another valid input.
pub fn bytes(data: &mut Vec<u8>, rng: &mut Rng, donor: &[u8]) {
    for _ in 0..1 + rng.below(4) {
        if data.is_empty() {
            data.push(rng.next() as u8);
            continue;
        }
        let len = data.len();
        let at = rng.below(len);
        match rng.below(12) {
            0 => data[at] ^= 1 << rng.below(8),
            1 => data[at] = INTERESTING[rng.below(INTERESTING.len())],
            2 => data[at] = data[at].wrapping_add(1),
            3 => data[at] = data[at].wrapping_sub(1),
            4 => data[at] = rng.next() as u8,
            5 => {
                let (start, size) = span(rng, len, 64);
                data.drain(start..start + size);
            }
            6 => {
                let (start, size) = span(rng, len, 64);
                let copy = data[start..start + size].to_vec();
                data.splice(at..at, copy);
            }
            7 => data.truncate(at),
            8 => {
                let (start, size) = span(rng, len, 32);
                for byte in &mut data[start..start + size] {
                    *byte = rng.next() as u8;
                }
            }
            9 if !donor.is_empty() => {
                let (start, size) = span(rng, donor.len(), 256);
                data.splice(at..at, donor[start..start + size].iter().copied());
            }
            10 if !donor.is_empty() => {
                let (start, size) = span(rng, donor.len(), 256);
                let end = (at + size).min(len);
                data.splice(at..end, donor[start..start + size].iter().copied());
            }
            _ => {
                let extra = 1 + rng.below(8);
                data.extend((0..extra).map(|_| rng.next() as u8));
            }
        }
    }
}
