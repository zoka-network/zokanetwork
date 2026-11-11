#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = zoka_pq_pool::fuzzing::envelope(data);
});
