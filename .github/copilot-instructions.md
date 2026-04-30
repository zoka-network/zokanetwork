# ZOKA Workspace Instructions

AI agent guide for the ZOKA private blockchain project. Rust-based implementation with zero-knowledge proofs, PBFT consensus, and privacy-first transaction handling.

---

## 🚀 Quick Start for AI Agents

### Essential Commands

```bash
# Build and test (always use these)
cargo build                    # Debug build
cargo build --release          # Optimized build
cargo test --all               # Run all tests
cargo run --example private_scenario  # Example with ZK transactions

# Interactive CLI
cargo run --bin user-cli-private      # Private transaction CLI
cargo run --bin user-cli              # Main user CLI

# Check compilation
cargo check                    # Fast syntax check
```

### Project Status

- **Privacy Level:** 22% (Monero-equivalent) → Target: 90%+ by Phase 5
- **Build:** ✅ Passing (0 errors)
- **Tests:** ✅ 177/177 passing
- **Architecture:** 12-layer modular blockchain

---

## 📦 Architecture Overview

### Module Hierarchy

```
Core Blockchain Layer (Foundation)
├─ crypto/        → Signatures (Ed25519), hashing (Blake3/SHA3), KDF (Argon2)
├─ types/         → Error enums, core types, constants
├─ block/         → Block creation, validation, merkle trees
├─ transaction/   → Commitments, nullifiers, ZK proof structure
├─ zk/            → Circuits (R1CS), prover, verifier

Consensus Layer
├─ consensus/     → PBFT (pre-prepare/prepare/commit), PoA

Network Layer
├─ network/       → P2P (libp2p), discovery, handlers
└─ state/         → Blockchain state, UTXOs, merkle proofs

Storage & Memory
├─ storage/       → RocksDB persistence, snapshots
├─ mempool/       → Transaction pool, pending transactions
└─ transaction_pool/  → Legacy mempool interface

User-Facing Layer (Adapters)
├─ wallet/        → HD wallets (BIP-32/39), user-friendly transactions
├─ private_wallet/→ Private transaction management
├─ cli/           → Command-line interface framework
├─ api/           → REST API with sensitive field redaction
├─ user_node/     → User node orchestration

Support Layers
├─ admin/         → Administrative functions
├─ health/        → Health checks
├─ metrics/       → Monitoring
├─ mining/        → PoW mining (legacy)
└─ identity/      → Identity management
```

### Dependency Flow

```
user-cli, api, user-node
    ↓ (use abstractions)
wallet, private_wallet, cli
    ↓ (use domain logic)
block, consensus, transaction, state
    ↓ (depend on)
crypto, types, zk, network, storage
```

**Key Principle:** Core modules never depend on user-facing modules.

---

## 🏗️ Code Patterns & Conventions

### 1. Module Export Pattern

Every module exports via `mod.rs`. In `src/lib.rs`:

```rust
pub mod crypto;
pub mod types;
pub mod block;
// ... all modules re-exported
```

**Usage:** Always import from crate root: `use zoka::block::Block;` (not `use crate::block::Block;` in lib code).

### 2. Error Handling (Type-Safe)

All errors defined in `src/types/mod.rs` using `thiserror::Error`:

```rust
#[derive(Error, Debug)]
pub enum BlockError {
    #[error("Invalid block hash")]
    InvalidHash,
    
    #[error("Failed to validate transaction: {0}")]
    TransactionValidation(String),
}

// Result type alias pattern
pub type BlockResult<T> = Result<T, BlockError>;
```

**Convention:** Each subsystem has its own `Error` enum (e.g., `TransactionError`, `StateError`).

### 3. Type Safety & Newtypes

Core types are newtypes over Vec<u8>:

```rust
pub struct PublicKey(pub Vec<u8>);
pub struct Signature(pub Vec<u8>);
pub struct TxHash(pub Vec<u8>);
```

**Benefits:**
- Type-safe: Can't mix up hashes with signatures
- Serializable: Auto-derives via serde
- Cloneable: No allocation overhead in type system

### 4. User-Facing Type Adapters

User modules create simplified types that adapt core types:

```rust
// Core type (full blockchain complexity)
pub struct Transaction {
    hash: TxHash,
    sender: Address,
    receiver: Address,
    amount: u64,
    nonce: u64,
    commitment: Option<Commitment>,
    nullifier: Option<Nullifier>,
    zk_proof: Option<ZKProof>,
    // ... more fields
}

// User type (simple, friendly)
pub struct UserTransaction {
    from: String,
    to: String,
    amount: u64,
}

// Conversion
impl From<UserTransaction> for Transaction {
    fn from(utx: UserTransaction) -> Self {
        // Populate all required fields
    }
}
```

### 5. State Machines in HashMaps

Transaction lifecycle uses HashMap buckets:

```rust
pub struct PrivateWalletState {
    pending: HashMap<TxHash, Transaction>,
    confirmed: HashMap<TxHash, Transaction>,
    failed: HashMap<TxHash, Transaction>,
}
```

**Why:** O(1) lookups, natural state separation.

### 6. Shared Mutable State Pattern

For global state (e.g., blockchain state):

```rust
pub struct BlockchainState {
    inner: Arc<Mutex<BlockchainStateInner>>,
}

impl BlockchainState {
    pub async fn get_balance(&self, addr: &Address) -> Result<u64> {
        let state = self.inner.lock().await;
        Ok(state.utxos.get(addr).map(|u| u.amount).unwrap_or(0))
    }
}
```

**When to use:**
- Shared across multiple tasks
- Async context required
- Concurrent reads + occasional writes

---

## 🧪 Testing Patterns

### Integration Tests (Primary)

Located in [tests/](tests/), focus on **end-to-end flows**:

```rust
#[tokio::test]
async fn test_private_transaction_flow() {
    // 1. Setup
    let storage = RocksDB::new("test_db");
    let state = BlockchainState::new(storage);
    
    // 2. Create accounts and balance
    state.add_account(alice, 1000);
    state.add_account(bob, 0);
    
    // 3. Execute transaction
    let tx = private_wallet.create_private_tx(alice, bob, 500);
    state.apply_transaction(&tx)?;
    
    // 4. Verify state changed
    assert_eq!(state.get_balance(alice), Ok(500));
    assert_eq!(state.get_balance(bob), Ok(500));
}
```

**Key Pattern:**
1. Setup storage + state
2. Populate initial data
3. Execute operation
4. Assert state + side effects

### Unit Tests (Secondary)

For logic-specific modules, use inline `#[cfg(test)]` blocks:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_commitment_calculation() {
        let commitment = Commitment::new_random(0);
        assert_ne!(commitment.value, Commitment::new_random(0).value); // Different blindness
    }
}
```

### Async Testing

Always use `#[tokio::test]` for async code:

```rust
#[tokio::test]
async fn test_network_broadcast() {
    let (tx, mut rx) = broadcast::channel(10);
    
    // Spawn background task
    tokio::spawn(async move {
        tx.send("hello").unwrap();
    });
    
    // Await result
    let msg = rx.recv().await;
    assert_eq!(msg, Ok("hello"));
}
```

### Dummy Crypto in Tests

For testing transaction logic without real crypto signing:

```rust
// Use dummy signature for testing
let dummy_sig = Signature(vec![0; 64]);

let tx = Transaction {
    signature: dummy_sig,  // Dummy for MVP testing
    // ... rest of fields
};
```

---

## 🔐 Privacy & ZK Proof Patterns

### Commitment Pattern

Used to hide transaction amounts:

```rust
pub struct Commitment {
    pub value: [u8; 32],        // Pedersen commitment (Blake3-based)
    pub blindness: [u8; 32],    // Random blinding factor
}

impl Commitment {
    pub fn new_random(amount: u64) -> Self {
        let blindness = rand::random();  // Always randomize for privacy
        Commitment {
            value: calculate_pedersen_commitment(amount, blindness),
            blindness,
        }
    }
}
```

**Key:** Always use random blindness factor; never reuse.

### Nullifier Pattern

Prevents double-spending in private transactions:

```rust
pub struct Nullifier {
    pub value: [u8; 32],  // Derived from private key + nonce
}

impl Nullifier {
    pub fn new(private_key: &PrivateKey, nonce: &[u8; 16]) -> Self {
        let hash = blake3(concat(private_key, nonce));
        Nullifier { value: hash }
    }
}
```

**Usage:** Track in `NullifierSet` to prevent double-spending.

### ZK Proof Pattern

Proves transaction balance without revealing amounts:

```rust
pub struct ZKProof {
    pub proof_bytes: Vec<u8>,   // Bellman R1CS proof (only digest, no witness)
    pub public_inputs: Vec<u8>, // Commitment values (public)
}

impl ZKProof {
    pub fn generate(tx: &Transaction) -> Result<Self> {
        let circuit = TransactionBalanceCircuit::new(
            commitment_in,
            commitment_out,
            tx.amount,
        );
        
        let proof = Prover::prove(&circuit)?;
        Ok(ZKProof {
            proof_bytes: proof.serialize(),  // No witness leaked
            public_inputs: tx.commitment.clone(),
        })
    }
}
```

**Security:** `proof_bytes` contains **no witness data**, only circuit tag + digest.

---

## 📝 Common Development Tasks

### Adding a New Module

1. **Create module directory:**
   ```bash
   mkdir -p src/my_feature
   touch src/my_feature/mod.rs
   ```

2. **Implement module logic:**
   ```rust
   // src/my_feature/mod.rs
   pub struct MyType { ... }
   
   impl MyType {
       pub fn new() -> Self { ... }
   }
   
   pub fn my_function() -> Result<MyType, MyError> { ... }
   ```

3. **Define error type in `src/types/mod.rs`:**
   ```rust
   #[derive(Error, Debug)]
   pub enum MyError {
       #[error("Something failed")]
       Failed,
   }
   ```

4. **Export from `src/lib.rs`:**
   ```rust
   pub mod my_feature;
   ```

5. **Write integration test in `tests/`:**
   ```rust
   #[test]
   fn test_my_feature() {
       use zoka::my_feature::*;
       let result = my_function();
       assert!(result.is_ok());
   }
   ```

### Running Specific Tests

```bash
# Run single test
cargo test test_name

# Run tests in module
cargo test --test integration_test

# Run with output
cargo test -- --nocapture

# Run ignored tests
cargo test -- --ignored
```

### Debugging Build Issues

```bash
# Check for syntax errors without building
cargo check

# Show detailed compiler output
RUST_LOG=debug cargo build 2>&1 | head -50

# Clean and rebuild
cargo clean && cargo build --release
```

### Adding Dependencies

1. **Add to `Cargo.toml`:**
   ```toml
   [dependencies]
   my_crate = "0.1"
   ```

2. **Use in code:**
   ```rust
   use my_crate::*;
   ```

3. **Test compilation:**
   ```bash
   cargo check
   cargo test --all
   ```

---

## ⚠️ Common Pitfalls & How to Avoid Them

| Pitfall | Problem | Solution |
|---------|---------|----------|
| **Forgetting to export module** | Module compiles but can't be imported | Always add `pub mod name;` to `src/lib.rs` |
| **Mixing core + user types** | Type mismatch errors, confusing code | Use `impl From` converters; never mix abstractions |
| **Reusing commitment blindness** | Privacy leak; observer can link transactions | Always call `new_random()`, never copy blindness |
| **Storing private keys in plain text** | Security vulnerability | Always use HD wallets with AES-256-GCM encryption |
| **Not testing async code** | Deadlocks or race conditions | Always use `#[tokio::test]` for async; use `.await` |
| **Generic errors instead of custom** | Hard to match error cases | Always define domain-specific error enums |
| **Forgetting locks on shared state** | Race condition bugs | Use `Arc<Mutex<T>>` for shared state; always `.lock().await` |
| **Skipping integration tests** | Logic bugs only found in production | Always write end-to-end tests with real state |
| **Wrong module layer** | Dependency cycle | Never import user modules in core; always one direction |

---

## 🎯 Key Files to Know

| File | Purpose | When to Modify |
|------|---------|-----------------|
| [src/lib.rs](src/lib.rs) | Module re-exports | Adding new modules |
| [src/types/mod.rs](src/types/mod.rs) | Error enums, core types | Adding new error types, constants |
| [src/crypto/mod.rs](src/crypto/mod.rs) | Signatures, hashing, KDF | Adding crypto primitives |
| [src/wallet/mod.rs](src/wallet/mod.rs) | HD wallets, user types | Adding wallet features |
| [src/transaction/mod.rs](src/transaction/mod.rs) | Commitments, nullifiers | Transaction structure changes |
| [src/zk/mod.rs](src/zk/mod.rs) | ZK circuits, proofs | Privacy improvements |
| [src/consensus/mod.rs](src/consensus/mod.rs) | PBFT, PoA | Changing consensus rules |
| [src/network/mod.rs](src/network/mod.rs) | P2P, messaging | Network layer changes |
| [tests/integration_test.rs](tests/integration_test.rs) | Main integration tests | Adding end-to-end tests |
| [Cargo.toml](Cargo.toml) | Dependencies, build config | Adding crates, features |

---

## 🔗 Documentation & Resources

- **[README.md](README.md)** - Project overview, quick start, build commands
- **[ROADMAP.md](ROADMAP.md)** - Privacy phases and implementation plan (Spanish/English)
- **Inline Comments** - Most complex logic has implementation notes
- **Integration Tests** - Best examples of how to use modules

---

## 📋 Checklist for New Features

- [ ] Module created in `src/my_feature/mod.rs`
- [ ] Public API documented with doc comments
- [ ] Custom error type defined in `src/types/mod.rs`
- [ ] Module exported in `src/lib.rs`
- [ ] Integration test added in `tests/`
- [ ] Example usage in doc comments
- [ ] `cargo test --all` passes
- [ ] `cargo build --release` succeeds
- [ ] Type safety verified (no `.expect()` in public API)
- [ ] Async code uses `#[tokio::test]` where needed

---

## 🚦 When to Ask for Clarification

Before making changes, ask the user if:
- The feature spans multiple modules (might need design discussion)
- It's a breaking change to public API
- It involves cryptography changes
- It modifies the transaction structure
- The user has specific privacy requirements

---

**Last Updated:** April 2026
**Rust Version:** 1.70+
**Edition:** 2021
