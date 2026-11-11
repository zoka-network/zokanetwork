//! Writes one valid input per fuzz target, in the layout `cargo fuzz` reads:
//! `<directory>/<target>/seed-<n>`.
//!
//! ```text
//! cargo run --release --features fuzzing --example fuzz_seeds -- ../../fuzz/corpus
//! ```
//!
//! It proves two transfers: some seconds.
use std::{collections::BTreeMap, fs, path::Path};
use zoka_pq_pool::fuzzing::Corpus;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args()
        .nth(1)
        .ok_or("usage: fuzz_seeds <corpus directory>")?;
    let mut written = BTreeMap::new();
    for (target, seed) in Corpus::build()?.seeds() {
        let directory = Path::new(&root).join(target);
        fs::create_dir_all(&directory)?;
        let count = written.entry(target).or_insert(0usize);
        fs::write(directory.join(format!("seed-{count}")), seed)?;
        *count += 1;
    }
    for (target, count) in written {
        println!("{target}: {count}");
    }
    Ok(())
}
