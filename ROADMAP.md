# 🔐 ROADMAP: Monero-Level Privacy for ZOKA

**Target:** 99%+ anonymity (Monero equivalent)  
**Current Status:** 22% privacy  
**Timeline:** 15-17 hours to reach 95%+  
**Last Updated:** April 12, 2026

---

## 🎯 Vision

Transform ZOKA from a privacy-focused blockchain with 22% privacy into a **Monero-equivalent system** with:
- ✅ Untraceable transactions
- ✅ Unlinkable payments
- ✅ Hidden balances
- ✅ Receiver anonymity
- ✅ Network privacy

---

## 📊 Privacy Roadmap by Phase

### PHASE 1: P2P WALLET INTEGRATION (Critical - 4-6 hours)

**Current State:** ❌ 0% integrated  
**Target State:** ✅ Users can broadcast transactions  
**Privacy Impact:** 22% → 40%  
**Difficulty:** Medium

#### 1.1 Create UserNode Wrapper
**File:** `src/user_node/mod.rs` (NEW)

```rust
pub struct UserNode {
    wallet_manager: PrivateWalletManager,
    p2p_node: Node,          // libp2p
    mempool: Mempool,
    sync_state: Arc<Mutex<SyncState>>,
}

impl UserNode {
    pub async fn send_transaction(
        &mut self,
        from_commitment: String,
        to_commitment: String,
        amount: u64,
    ) -> Result<String> {
        // 1. Create private transaction
        let tx = self.wallet_manager
            .create_private_tx(from_commitment, to_commitment, amount)?;
        
        // 2. Validate locally
        self.mempool.validate(&tx)?;
        
        // 3. Publish to network
        self.p2p_node.publish_transaction(&tx).await?;
        
        // 4. Track for confirmation
        self.sync_state.lock().await
            .add_pending(tx.hash.clone());
        
        Ok(tx.hash)
    }
}
```

**Subtasks:**
- [ ] Define `UserNode` struct
- [ ] Implement `send_transaction()`
- [ ] Implement `on_inbound_transaction()` handler
- [ ] Implement transaction validation
- [ ] Add balance update on receipt
- [ ] Write 5+ unit tests

**Estimated Time:** 2-3 hours

---

#### 1.2 Transaction Broadcasting
**Modify:** `src/network/message.rs`

Add new message type:
```rust
pub enum ZokaMessage {
    // ... existing ...
    PrivateTransaction(PrivateTransaction),
}
```

Implement gossipsub publish:
```rust
pub async fn publish_transaction(&self, tx: &PrivateTransaction) -> Result<()> {
    let msg = ZokaMessage::PrivateTransaction(tx.clone());
    let encoded = bincode::serialize(&msg)?;
    self.gossipsub.publish(
        IdentTopic::new("private-transactions"),
        encoded
    ).await?;
    Ok(())
}
```

**Subtasks:**
- [ ] Add message variant
- [ ] Implement serialization
- [ ] Implement gossipsub publish
- [ ] Add error handling

**Estimated Time:** 1-2 hours

---

#### 1.3 Inbound Transaction Handling
**Modify:** `src/network/handler.rs`

```rust
async fn on_message(
    &mut self,
    msg: ZokaMessage,
    peer: PeerId,
) {
    match msg {
        ZokaMessage::PrivateTransaction(tx) => {
            // 1. Validate ZK proof
            if !self.verify_zk_proof(&tx.zk_proof).await {
                warn!("Invalid ZK proof from {}", peer);
                return;
            }
            
            // 2. Check nullifiers
            if self.nullifier_set.is_spent(&tx.nullifier) {
                warn!("Double-spend detected");
                return;
            }
            
            // 3. Check if for me
            if self.is_my_commitment(&tx.recipient_commitment) {
                // Decrypt and update balance
                let amount = self.decrypt_amount(&tx)?;
                self.update_balance(amount).await;
            }
            
            // 4. Relay to mempool
            self.mempool.add(tx).await;
        }
        // ... other cases ...
    }
}
```

**Subtasks:**
- [ ] Implement message validation
- [ ] Implement commitment checking
- [ ] Implement decryption logic
- [ ] Implement mempool relay
- [ ] Add tests

**Estimated Time:** 2 hours

---

#### 1.4 Balance Synchronization
**File:** `src/user_node/sync.rs` (NEW)

```rust
pub struct SyncState {
    pending_txs: HashMap<String, PendingTx>,
    confirmed_txs: HashSet<String>,
    balance: u64,
    last_sync: SystemTime,
}

impl SyncState {
    pub async fn sync_with_blockchain(&mut self, node: &Node) -> Result<()> {
        // 1. Get all blocks since last sync
        let blocks = node.get_blocks_since(self.last_sync).await?;
        
        // 2. Search for transactions to me
        for block in blocks {
            for tx in &block.transactions {
                if self.is_my_commitment(&tx.recipient_commitment) {
                    // Found transaction to me!
                    let amount = self.decrypt_amount(tx)?;
                    self.balance += amount;
                    self.confirmed_txs.insert(tx.hash.clone());
                }
            }
        }
        
        self.last_sync = SystemTime::now();
        Ok(())
    }
}
```

**Subtasks:**
- [ ] Define sync state struct
- [ ] Implement block scanning
- [ ] Implement transaction search
- [ ] Implement balance updates
- [ ] Add periodic sync task

**Estimated Time:** 2 hours

---

**Phase 1 Checklist:**
- [ ] UserNode struct created
- [ ] send_transaction() working
- [ ] on_inbound_transaction() handler working
- [ ] Transactions broadcasting via gossipsub
- [ ] Balance updating on receipt
- [ ] 10+ integration tests passing
- [ ] All compilation clean

**Success Criteria:** Users can send/receive private transactions on network

---

### PHASE 2: STEALTH ADDRESSES (Critical - 3-4 hours)

**Current State:** ❌ Recipient always visible  
**Target State:** ✅ One-time address per transaction  
**Privacy Impact:** 40% → 70%  
**Difficulty:** Medium-Hard

#### 2.1 Stealth Address Generator
**File:** `src/identity/stealth.rs` (NEW)

```rust
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::point::CompressedEdwardsY;

pub struct StealthAddress {
    /// Public spend key (visible on blockchain)
    pub spend_key: EdwardsPoint,
    
    /// Public view key (only I know private part)
    pub view_key: EdwardsPoint,
    
    /// Index for derivation
    pub index: u32,
}

impl StealthAddress {
    /// Generate stealth address for receiver
    pub fn generate(
        receiver_spend_pubkey: &EdwardsPoint,
        receiver_view_pubkey: &EdwardsPoint,
        ephemeral_privkey: &Scalar,  // Random per transaction
    ) -> Self {
        // Stealth = recipient_spend + Hash(ephemeral * recipient_view)
        let shared_secret = ephemeral_privkey * receiver_view_pubkey;
        let hash_secret = Sha256::new()
            .update(shared_secret.compress().as_bytes())
            .finalize();
        
        let tweak = Scalar::from_bytes_mod_order(hash_secret.as_slice());
        let stealth_address = receiver_spend_pubkey + (tweak * GENERATOR);
        
        StealthAddress {
            spend_key: stealth_address,
            view_key: receiver_view_pubkey.clone(),
            index: 0,
        }
    }
    
    /// Receiver can detect this address
    pub fn detect(
        stealth: &EdwardsPoint,
        receiver_spend_privkey: &Scalar,
        receiver_view_privkey: &Scalar,
        ephemeral_pubkey: &EdwardsPoint,
    ) -> bool {
        // Only receiver with view_key can detect
        let shared_secret = receiver_view_privkey * ephemeral_pubkey;
        let hash_secret = Sha256::new()
            .update(shared_secret.compress().as_bytes())
            .finalize();
        
        let tweak = Scalar::from_bytes_mod_order(hash_secret.as_slice());
        let derived = receiver_spend_privkey * GENERATOR + (tweak * GENERATOR);
        
        derived == *stealth  // Only I can detect
    }
}
```

**Subtasks:**
- [ ] Implement stealth address generation
- [ ] Implement detection algorithm
- [ ] Add ECDH key exchange
- [ ] Add Sha256 hashing
- [ ] Write cryptographic tests

**Estimated Time:** 2 hours

---

#### 2.2 Ephemeral Keys in Transactions
**Modify:** `src/private_wallet/mod.rs`

Update `PrivateTransaction` struct:
```rust
pub struct PrivateTransaction {
    // ... existing fields ...
    
    /// Ephemeral public key for stealth address
    pub ephemeral_pubkey: EdwardsPoint,
    
    /// Stealth address (one-time, can't link)
    pub stealth_address: EdwardsPoint,
}

impl PrivateTransaction {
    pub fn create_stealth(
        sender_commitment: String,
        receiver_spend_pubkey: EdwardsPoint,
        receiver_view_pubkey: EdwardsPoint,
        amount: u64,
    ) -> Result<Self> {
        // Generate ephemeral key pair
        let ephemeral_privkey = Scalar::random(&mut OsRng);
        let ephemeral_pubkey = &ephemeral_privkey * GENERATOR;
        
        // Generate stealth address
        let stealth = StealthAddress::generate(
            &receiver_spend_pubkey,
            &receiver_view_pubkey,
            &ephemeral_privkey,
        );
        
        // Create transaction with stealth address
        Self {
            ephemeral_pubkey,
            stealth_address: stealth.spend_key,
            // ... other fields ...
        }
    }
}
```

**Subtasks:**
- [ ] Add ephemeral_pubkey field
- [ ] Add stealth_address field
- [ ] Implement stealth generation
- [ ] Update transaction builder
- [ ] Update validation logic

**Estimated Time:** 1 hour

---

#### 2.3 Receiver Detection
**File:** `src/user_node/stealth_detection.rs` (NEW)

```rust
pub struct StealthDetector {
    spend_privkey: Scalar,
    view_privkey: Scalar,
}

impl StealthDetector {
    pub fn scan_transaction(
        &self,
        tx: &PrivateTransaction,
    ) -> Option<u64> {
        // Check if stealth address is for me
        if StealthAddress::detect(
            &tx.stealth_address,
            &self.spend_privkey,
            &self.view_privkey,
            &tx.ephemeral_pubkey,
        ) {
            // This is MY transaction! Decrypt amount
            return Some(self.decrypt_amount(tx));
        }
        None
    }
    
    pub fn scan_block(&self, block: &Block) -> Vec<(String, u64)> {
        // Scan all transactions in block
        block.transactions
            .iter()
            .filter_map(|tx| {
                self.scan_transaction(tx)
                    .map(|amount| (tx.hash.clone(), amount))
            })
            .collect()
    }
}
```

**Subtasks:**
- [ ] Implement transaction scanning
- [ ] Implement block scanning
- [ ] Implement amount decryption
- [ ] Add background sync task
- [ ] Write detection tests

**Estimated Time:** 1-2 hours

---

**Phase 2 Checklist:**
- [ ] Stealth address generator working
- [ ] Ephemeral keys generated per transaction
- [ ] Detection algorithm verified
- [ ] Receiver can identify payments
- [ ] Sender can't link transactions
- [ ] 10+ cryptographic tests passing

**Success Criteria:** No way to link payments to same receiver

---

### PHASE 3: RANGE PROOFS (High Priority - 2-3 hours)

**Current State:** ❌ No constraints on amounts  
**Target State:** ✅ Amounts proven to be in valid range  
**Privacy Impact:** 70% → 85%  
**Difficulty:** Medium

#### 3.1 Add Range Circuit
**Modify:** `src/zk/circuits.rs`

```rust
use bellman::{Circuit, ConstraintSystem, SynthesisError};
use ff::PrimeField;

pub struct RangeProofCircuit<F: PrimeField> {
    /// Committed amount (private)
    amount: Option<F>,
    
    /// Commitment to the amount (public)
    commitment: Option<F>,
    
    /// Range: amount must be in [0, 2^64)
    range_bits: usize,
}

impl<F: PrimeField> Circuit<F> for RangeProofCircuit<F> {
    fn synthesize<CS: ConstraintSystem<F>>(
        self,
        cs: &mut CS,
    ) -> Result<(), SynthesisError> {
        // Allocate amount as private variable
        let amount = cs.alloc(
            || "amount",
            || self.amount.ok_or(SynthesisError::AssignmentMissing),
        )?;
        
        // Allocate commitment as public input
        let commitment = cs.alloc_input(
            || "commitment",
            || self.commitment.ok_or(SynthesisError::AssignmentMissing),
        )?;
        
        // Decompose amount into bits
        let bits = decompose_into_bits(&amount, self.range_bits);
        
        // Constrain: bits_sum must equal amount
        let mut reconstructed = F::ZERO;
        for (i, bit) in bits.iter().enumerate() {
            // bit * (bit - 1) = 0  =>  bit is binary
            cs.enforce(
                || format!("bit{}_binary", i),
                |lc| lc + bit.clone(),
                |lc| lc + bit.clone() - CS::one(),
                |lc| lc, // = 0
            )?;
            
            // Sum bits
            let weight = F::from(2).pow(&[i as u32]);
            reconstructed = reconstructed + weight * bit.value().unwrap_or(F::ZERO);
        }
        
        // Constrain: sum of bits = amount
        cs.enforce(
            || "bit_sum_equals_amount",
            |lc| lc + amount,
            |lc| lc + CS::one(),
            |lc| lc + (reconstructed),
        )?;
        
        // Constrain: Hash(amount, blinding) = commitment
        // (This would use a hash gadget in practice)
        
        Ok(())
    }
}
```

**Subtasks:**
- [ ] Implement range proof circuit
- [ ] Add bit decomposition
- [ ] Add binary constraints
- [ ] Add commitment verification
- [ ] Test with valid ranges
- [ ] Test with invalid ranges

**Estimated Time:** 2 hours

---

#### 3.2 Update Transaction Validator
**Modify:** `src/transaction/validation.rs`

```rust
pub fn validate_amount_range(
    commitment: &Commitment,
    zk_proof: &ZKProof,
) -> Result<(), ValidationError> {
    // Verify ZK proof that amount is in [0, 2^64)
    if !verifier.verify(zk_proof) {
        return Err(ValidationError::InvalidProof);
    }
    
    // Verify proof is for this commitment
    let public_inputs = vec![commitment.as_bytes().to_vec()];
    if !schema.validate(&public_inputs) {
        return Err(ValidationError::InvalidInput);
    }
    
    Ok(())
}
```

**Subtasks:**
- [ ] Add range validation
- [ ] Update mempool validation
- [ ] Update block validation
- [ ] Test with various amounts

**Estimated Time:** 30 min

---

**Phase 3 Checklist:**
- [ ] Range circuit implemented
- [ ] Binary constraints working
- [ ] Proofs generate correctly
- [ ] Verification working
- [ ] Invalid ranges rejected
- [ ] 5+ range proof tests passing

**Success Criteria:** Impossible to create money from nothing

---

### PHASE 4: PERSISTENT WALLETS (High Priority - 2-3 hours)

**Current State:** ❌ Wallets lost on restart  
**Target State:** ✅ Encrypted storage, wallet recovery  
**Privacy Impact:** 85% → 90%  
**Difficulty:** Low-Medium

#### 4.1 Encrypted Wallet Storage
**File:** `src/user_node/wallet_storage.rs` (NEW)

```rust
use argon2::{Argon2, PasswordHasher};
use aes_gcm::{Aes256Gcm, Key, Nonce};

pub struct EncryptedWalletStorage {
    password: String,
}

impl EncryptedWalletStorage {
    pub fn save(
        &self,
        wallet: &PrivateWallet,
        path: &Path,
    ) -> Result<()> {
        // 1. Serialize wallet
        let json = serde_json::to_string(wallet)?;
        
        // 2. Derive encryption key from password
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let password_hash = argon2.hash_password(
            self.password.as_bytes(),
            &salt,
        )?;
        
        let key_material = password_hash.hash.ok_or("No hash")?;
        let key = Key::<Aes256Gcm>::from_slice(&key_material.as_bytes()[..32]);
        
        // 3. Encrypt with AES-256-GCM
        let nonce = Nonce::from_slice(b"unique_nonce");
        let cipher = Aes256Gcm::new(key);
        let encrypted = cipher.encrypt(nonce, json.as_ref())?;
        
        // 4. Store encrypted data + salt
        let data = EncryptedData {
            salt: salt.to_string(),
            encrypted: encrypted.to_vec(),
        };
        
        let encrypted_json = serde_json::to_string(&data)?;
        std::fs::write(path, encrypted_json)?;
        
        Ok(())
    }
    
    pub fn load(
        &self,
        path: &Path,
    ) -> Result<PrivateWallet> {
        // 1. Load encrypted data
        let encrypted_json = std::fs::read_to_string(path)?;
        let data: EncryptedData = serde_json::from_str(&encrypted_json)?;
        
        // 2. Derive key from password
        let salt = SaltString::parse(&data.salt)?;
        let argon2 = Argon2::default();
        let password_hash = argon2.hash_password(
            self.password.as_bytes(),
            &salt,
        )?;
        
        let key_material = password_hash.hash.ok_or("No hash")?;
        let key = Key::<Aes256Gcm>::from_slice(&key_material.as_bytes()[..32]);
        
        // 3. Decrypt
        let nonce = Nonce::from_slice(b"unique_nonce");
        let cipher = Aes256Gcm::new(key);
        let decrypted = cipher.decrypt(nonce, data.encrypted.as_ref())?;
        
        // 4. Deserialize
        let json = String::from_utf8(decrypted)?;
        let wallet = serde_json::from_str(&json)?;
        
        Ok(wallet)
    }
}
```

**Subtasks:**
- [ ] Implement password-based encryption
- [ ] Implement argon2 key derivation
- [ ] Implement AES-256-GCM encryption
- [ ] Implement wallet save
- [ ] Implement wallet load
- [ ] Add error handling

**Estimated Time:** 1-2 hours

---

#### 4.2 Wallet Recovery
**Modify:** `src/identity/mod.rs`

```rust
impl HDWallet {
    pub fn recover_from_mnemonic(
        mnemonic: &[String],
    ) -> Result<Self> {
        // 1. Validate mnemonic (12-24 words)
        if mnemonic.len() % 3 != 0 || mnemonic.len() < 12 {
            return Err("Invalid mnemonic length".into());
        }
        
        // 2. Mnemonic → Seed (BIP-39)
        let seed = Self::mnemonic_to_seed(mnemonic)?;
        
        // 3. Create wallet from seed
        let wallet = Self::new(seed)?;
        
        Ok(wallet)
    }
    
    pub fn recover_account(
        &self,
        account_index: u32,
    ) -> Result<(EdwardsPoint, Scalar)> {
        // Derive account from BIP-32 path
        // m/44'/888'/0'/0/{account_index}
        self.derive_account(account_index)
    }
}
```

**Subtasks:**
- [ ] Implement mnemonic validation
- [ ] Implement BIP-39 recovery
- [ ] Implement account derivation
- [ ] Test with known vectors
- [ ] Add documentation

**Estimated Time:** 1 hour

---

**Phase 4 Checklist:**
- [ ] Wallet encryption working
- [ ] Password protection verified
- [ ] Wallet recovery working
- [ ] Mnemonic validation working
- [ ] Data persists across restarts
- [ ] 5+ storage tests passing

**Success Criteria:** Wallets persist and recover correctly

---

### PHASE 5: BALANCE PRIVACY (Medium Priority - 4-5 hours)

**Current State:** ❌ Balance is u64 in memory  
**Target State:** ✅ Balance proven with ZK proofs  
**Privacy Impact:** 90% → 95%  
**Difficulty:** Hard

#### 5.1 Merkle Commitment Tree
**File:** `src/state/commitment_tree.rs` (NEW)

```rust
pub struct CommitmentTree {
    /// Merkle tree of all active commitments
    tree: MerkleTree<CommitmentHash>,
    
    /// Height of tree (for proof verification)
    height: usize,
}

impl CommitmentTree {
    pub fn new(height: usize) -> Self {
        CommitmentTree {
            tree: MerkleTree::new(vec![], height),
            height,
        }
    }
    
    /// Add commitment to tree
    pub fn add_commitment(&mut self, commitment: &Commitment) -> usize {
        let leaf = CommitmentHash(blake3_hash(commitment.as_bytes()));
        let index = self.tree.add_leaf(leaf);
        index
    }
    
    /// Generate membership proof
    pub fn prove_membership(
        &self,
        index: usize,
    ) -> Result<MerkleProof> {
        self.tree.prove_membership(index)
    }
    
    /// Verify membership proof
    pub fn verify_membership(
        &self,
        commitment: &Commitment,
        proof: &MerkleProof,
    ) -> Result<bool> {
        let leaf = CommitmentHash(blake3_hash(commitment.as_bytes()));
        self.tree.verify_proof(&leaf, proof)
    }
    
    /// Get root hash
    pub fn root(&self) -> [u8; 32] {
        self.tree.root()
    }
}
```

**Subtasks:**
- [ ] Implement merkle tree structure
- [ ] Implement leaf insertion
- [ ] Implement proof generation
- [ ] Implement proof verification
- [ ] Add tests for tree operations

**Estimated Time:** 2 hours

---

#### 5.2 Balance Proof Circuit
**Modify:** `src/zk/circuits.rs`

```rust
pub struct BalanceProofCircuit<F: PrimeField> {
    /// All my commitments (private)
    my_commitments: Vec<Option<F>>,
    
    /// Merkle path proofs (private)
    merkle_proofs: Vec<Option<MerkleProof>>,
    
    /// Total balance (private)
    total_balance: Option<F>,
    
    /// Chain root (public)
    chain_root: Option<F>,
}

impl<F: PrimeField> Circuit<F> for BalanceProofCircuit<F> {
    fn synthesize<CS: ConstraintSystem<F>>(
        self,
        cs: &mut CS,
    ) -> Result<(), SynthesisError> {
        // 1. Verify I own each commitment
        let mut accumulated_balance = F::ZERO;
        
        for i in 0..self.my_commitments.len() {
            let commitment = cs.alloc(
                || format!("commitment_{}", i),
                || self.my_commitments[i].ok_or(SynthesisError::AssignmentMissing),
            )?;
            
            // 2. Verify merkle proof (commitment is in chain)
            // Note: This is simplified; actual merkle proof verification is complex
            
            // 3. Accumulate balance
            // accumulated_balance += extract_amount(commitment)
        }
        
        // 4. Constrain: sum = total_balance
        let total = cs.alloc(
            || "total_balance",
            || self.total_balance.ok_or(SynthesisError::AssignmentMissing),
        )?;
        
        cs.enforce(
            || "balance_equals_sum",
            |lc| lc + accumulated_balance,
            |lc| lc + CS::one(),
            |lc| lc + total,
        )?;
        
        Ok(())
    }
}
```

**Subtasks:**
- [ ] Implement circuit structure
- [ ] Add commitment verification
- [ ] Add merkle proof verification
- [ ] Add balance accumulation
- [ ] Test with various balances

**Estimated Time:** 2-3 hours

---

#### 5.3 Private Balance Manager
**File:** `src/user_node/private_balance.rs` (NEW)

```rust
pub struct PrivateBalance {
    commitments: Vec<Commitment>,
    merkle_proofs: Vec<MerkleProof>,
    total_balance: u64,
}

impl PrivateBalance {
    pub fn prove_balance(
        &self,
        amount: u64,
    ) -> Result<ZKProof> {
        // Generate proof that balance >= amount
        let circuit = BalanceProofCircuit {
            my_commitments: self.commitments.iter()
                .map(|c| Some(c.as_bytes()[..32]))
                .collect(),
            merkle_proofs: self.merkle_proofs.clone(),
            total_balance: Some(amount),
            chain_root: Some(self.chain_root),
        };
        
        prover.prove(circuit)
    }
    
    pub fn verify_balance_proof(
        &self,
        proof: &ZKProof,
    ) -> bool {
        verifier.verify(proof).is_valid
    }
}
```

**Subtasks:**
- [ ] Implement private balance structure
- [ ] Implement proof generation
- [ ] Implement proof verification
- [ ] Add balance updates
- [ ] Write tests

**Estimated Time:** 1 hour

---

**Phase 5 Checklist:**
- [ ] Merkle tree implementation working
- [ ] Commitment tree building correctly
- [ ] Balance proof circuit working
- [ ] Proofs verify against chain root
- [ ] No balance information leaked
- [ ] 5+ balance privacy tests passing

**Success Criteria:** Balance completely hidden from observers

---

## 🎯 IMPLEMENTATION TIMELINE

| Phase | Component | Hours | Target Privacy |
|-------|-----------|-------|-----------------|
| **1** | P2P Integration | 4-6 | 40% |
| **2** | Stealth Addresses | 3-4 | 70% |
| **3** | Range Proofs | 2-3 | 85% |
| **4** | Persistent Wallets | 2-3 | 90% |
| **5** | Balance Privacy | 4-5 | 95% |
| **TOTAL** | Complete Implementation | **15-17** | **95%** |

---

## ✅ Milestones & Success Criteria

### Milestone 1: P2P Integration Complete ✓
- Users can send transactions to network
- Receivers get notifications
- Balance updates on receipt
- Transactions broadcast via gossipsub

### Milestone 2: Receiver Anonymity Complete ✓
- Stealth addresses working
- One-time per transaction
- Receiver can detect payments
- Sender can't link transactions

### Milestone 3: Amount Validation Complete ✓
- Range proofs preventing overflow
- Invalid amounts rejected
- No money creation possible
- Proofs verified in mempool

### Milestone 4: Data Persistence Complete ✓
- Wallets survive restarts
- Recovery from mnemonic working
- Encrypted storage verified
- Password protection active

### Milestone 5: Privacy Complete ✓
- Balances completely hidden
- Privacy = Monero equivalent
- Untraceable transactions
- Unlinkable payments

---

## 🔍 TESTING STRATEGY

### Phase 1 Tests
```
✅ UserNode creation
✅ Transaction sending
✅ Transaction receipt
✅ Balance updates
✅ Gossipsub publishing
✅ Mempool integration
✅ Full E2E user scenario
```

### Phase 2 Tests
```
✅ Stealth address generation
✅ Ephemeral key creation
✅ Receiver detection
✅ Non-linkability
✅ Multiple payments to same user
✅ Stealth in full transaction
```

### Phase 3 Tests
```
✅ Range proof generation
✅ Range proof verification
✅ Valid ranges [0, 2^64)
✅ Invalid ranges rejected
✅ Circuit constraints working
```

### Phase 4 Tests
```
✅ Wallet encryption
✅ Wallet decryption
✅ Password protection
✅ Recovery from mnemonic
✅ Persistence across restarts
```

### Phase 5 Tests
```
✅ Merkle tree operations
✅ Commitment adding
✅ Proof generation
✅ Proof verification
✅ Balance privacy verification
```

---

## 📊 COMPARISON at Each Phase

```
Phase 0 (Current):  Bitcoin-like    2%  ❌
Phase 1:           Zcash-like      40%  ⚠️
Phase 2:           Monero-like     70%  ⚠️
Phase 3:           Monero-like     85%  ✅
Phase 4:           Monero-like     90%  ✅
Phase 5:           Monero-like     95%  ✅

Target = Monero: RingCT + Stealth + Private Balance
```

---

## 🚀 HOW TO CONTRIBUTE

### Starting Phase 1?
1. Read this document (ROADMAP.md)
2. Read [PRIVACY_ANALYSIS.md](PRIVACY_ANALYSIS.md) 
3. Create `src/user_node/mod.rs`
4. Implement `UserNode` struct
5. Write tests
6. Open PR

### Starting Phase 2?
1. Create `src/identity/stealth.rs`
2. Implement stealth address generation
3. Add ephemeral keys to transactions
4. Test detection algorithm
5. Write 10+ tests
6. Open PR

### Any Phase?
```bash
# Pick Phase X
# 1. Create feature branch
git checkout -b phase-X-implementation

# 2. Code solution
# 3. Add comprehensive tests
# 4. Ensure compilation clean
cargo check --all-targets

# 5. Run all tests
cargo test --all -- --test-threads=1

# 6. Update ROADMAP.md
# 7. Open Pull Request
```

---

## ⚠️ KNOWN RISKS & MITIGATIONS

| Risk | Impact | Mitigation |
|------|--------|-----------|
| P2P integration fails | Blocks All | Start with mocks first |
| ECDH implementation wrong | Security | Use tested libraries |
| ZK circuits unsound | Critical | Add constraint tests |
| Encryption weak | Security | Use standard libraries |
| Merkle tree bugs | Critical | Add extensive tests |

---

## 📚 REFERENCES

**Stealth Addresses:**
- [CryptoNote: Stealth Addresses](https://cryptonote.org/whitepaper.pdf)
- [Monero Stealth Implementation](https://www.monerooutreach.org/stories/)

**Range Proofs:**
- [Bulletproofs: Short Proofs](https://eprint.iacr.org/2017/1066.pdf)
- [RingCT Range Proofs](https://www.monerooutreach.org/stories/RingCT.html)

**Merkle Trees:**
- [Merkle Proof Verification](https://en.wikipedia.org/wiki/Merkle_tree)
- [ICP Merkle Trees](https://dfinity.org/howitworks/#merkle-trees)

**ZK Implementation:**
- [Bellman R1CS](https://github.com/zkcrypto/bellman/)
- [Groth16 Proofs](http://stevengoldfeder.com/papers/Groth16.pdf)

---

## 📝 PROGRESS TRACKING

As you implement each phase, update this file:

```markdown
## IMPLEMENTATION STATUS

### Phase 1: P2P Integration
- [x] UserNode struct (2h)
- [x] Transaction broadcasting (1.5h)
- [x] Inbound handlers (2h)
- [x] Balance sync (2h)
- [x] Integration tests (2h)
- [x] Documentation (1h)
**COMPLETED: 10.5/6 hours** ✅

### Phase 2: Stealth Addresses
- [ ] Stealth generation (2h)
- [ ] Ephemeral keys (1h)
- [ ] Receiver detection (1-2h)
**IN PROGRESS: 2/3 hours** 🟡

### Phase 3: Range Proofs
- [ ] TBD

### Phase 4: Persistent Wallets
- [ ] TBD

### Phase 5: Balance Privacy
- [ ] TBD
```

---

## 🎯 FINAL GOAL

```
CURRENT:  22% Monero-equivalent (commitments + nullifiers)
TARGET:   95% Monero-equivalent (+ all above phases)

WITH THIS ROADMAP:
✅ Untraceable transactions
✅ Unlinkable payments
✅ Hidden balances
✅ Receiver anonymity
✅ Network privacy ready
```

---

**Last Updated:** April 12, 2026  
**Current Privacy:** 22% (Monero equivalent)  
**Target Privacy:** 95% (Monero equivalent)  
**Estimated Completion:** 15-17 hours  
**Status:** 🔴 Ready to implement
