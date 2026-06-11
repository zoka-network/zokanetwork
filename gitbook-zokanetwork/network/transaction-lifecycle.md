# Transaction Lifecycle

## 1. Create

The sender's wallet selects spendable private notes and a recipient `zka1...`
receive address.

## 2. Build Proof Data

The wallet constructs the private transfer locally. Private metadata remains in
the wallet boundary; the public transaction does not reveal sender, receiver, or
amount.

## 3. Broadcast

The locally built transaction is submitted to the network through a node or RPC
endpoint. Public RPC should receive transaction payloads, not wallet secrets.

## 4. Validate

Nodes validate consensus rules, proof data, nullifiers, commitments, and block
structure.

## 5. Include in Block

A miner includes the transaction hash or body data required by the active
protocol format in a proof-of-work block.

## 6. Recover by Receiver

The receiver's wallet scans chain data, detects outputs that belong to its scan
key, and decrypts the data needed to update local balance and recent activity.

