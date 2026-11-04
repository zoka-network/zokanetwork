// Step 2 of the WebAssembly payment test: the wallet of the fixture words
// reads the three regtest blocks, and pays Bob 7 ZKA with a proof made by this
// WebAssembly. The node then takes and mines it (step 3).
//
//   1. cargo test --release --features quantic-pq --lib web_wallet_regtest_fixture -- --ignored
//   2. wasm-pack build --release --target nodejs --out-dir pkg-node --no-pack
//      node tests/regtest-payment.mjs        (from crates/zoka-pq-wallet-wasm)
//   3. cargo test --release --features quantic-pq --lib a_payment_proved_in_webassembly -- --ignored
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const w = require("../pkg-node/zoka_pq_wallet_wasm.js");
const at = (name) => new URL(`./fixtures/${name}`, import.meta.url);

const phrase = readFileSync(at("regtest-phrase.txt"), "utf8").trim();
const bob = readFileSync(at("regtest-bob.txt"), "utf8").trim();
const page = new Uint8Array(readFileSync(at("regtest-scan2.bin")));

const wallet = new w.WebWallet(phrase, 3, new Uint8Array());
const tip = wallet.addPage(page);
const read = JSON.parse(wallet.status());
console.log(`read ${read.height} of ${tip} blocks; spendable ${read.spendable_atoms} atoms`);
if (read.spendable_atoms <= 0) throw new Error("the fixture wallet has nothing to spend");

const quote = JSON.parse(wallet.quote(7_000_000n));
console.log("quote", quote);
if (!quote.enough) throw new Error(quote.reason);

const started = performance.now();
const proved = JSON.parse(wallet.prove(bob, 7_000_000n, "from the web"));
const ms = performance.now() - started;
const bytes = wallet.lastTransaction();
writeFileSync(at("regtest-web-payment.bin"), bytes);
console.log(`proved ${proved.txid} in ${(ms / 1000).toFixed(2)} s: ${bytes.length} bytes, fee ${proved.fee_atoms}, expiry ${proved.expiry}`);

// The notes stay reserved until the payment is mined or expires.
const after = JSON.parse(wallet.status());
if (!after.pending || after.pending.txid !== proved.txid) throw new Error("the payment is not reserved");
const restored = new w.WebWallet(phrase, 3, wallet.encodeState());
if (JSON.parse(restored.status()).pending?.txid !== proved.txid) throw new Error("the reservation was not saved");
console.log("reserved, and the reservation survives the saved state");
let refused = "";
try {
  wallet.prove(bob, 1_000_000n, "");
} catch (error) {
  refused = error.message ?? String(error);
}
if (!/pending/.test(refused)) throw new Error(`a second payment was not refused: ${refused}`);
console.log(`a second payment waits: ${refused}`);
