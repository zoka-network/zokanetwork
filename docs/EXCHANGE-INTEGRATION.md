# Integrating Zoka Quantic: a guide for exchanges

Zoka Quantic is a private, post-quantum proof-of-work coin. Every payment is a
shielded note: the chain shows neither amounts nor addresses. This guide covers
everything an exchange needs: a node, a wallet server, deposits with a tag per
customer, withdrawals that are never paid twice, and when a deposit is final.

**Status.** Zoka Quantic runs as a **testnet** today; the mainnet targets Q4
2027, once an external review passes. Integrate on the testnet now: the mainnet
uses the same software, methods and flows. The only change is `--network`.

## 1. Architecture

```text
your backend ──JSON-RPC 2.0──▶ zoka quantic-pq-wallet-rpc ──HTTP──▶ zoka quantic-pq-node
   (private network)            (holds the hot wallet)           (validates the chain)
```

- **Run your own node.** Never rely on someone else's: your node validates
  every block and every proof itself.
- **Run the wallet server next to it**, on the same machine or private
  network. It reads the chain from your node and builds payments, including
  their STARK proofs (1 to 5 seconds each on a desktop CPU).
- **Talk to the wallet server only from your backend.** It listens on
  `127.0.0.1` unless you pass `--allow-remote`, and every request needs a bearer
  token.

Requirements: a 64-bit Linux or Windows host, 4 GB of RAM, a few GB of disk.
Download the release and **verify its signature** before running it
([release-verification.md](release-verification.md)).

## 2. Setup

```sh
# 1. The node. Its API listens on 127.0.0.1:9140 only. The password protects
#    the node's network identity (it is created on the first start).
export ZOKA_PQ_PASSWORD='<node identity password>'
zoka quantic-pq-node serve --network testnet \
  --db ./zoka/node/db --identity ./zoka/node/node.identity --api 127.0.0.1:9140

# 2. The hot wallet. Write the recovery phrase down offline: it is shown once
#    and it is the only way to restore the wallet.
export ZOKA_WALLET_PASSWORD='<wallet password>'
zoka quantic-pq-wallet new --network testnet --dir ./zoka/wallet \
  --name exchange --password-env ZOKA_WALLET_PASSWORD

# 3. Publish the wallet's address once, so payers can resolve deposit
#    addresses (they carry its short form).
zoka quantic-pq-wallet publish-address --network testnet --dir ./zoka/wallet \
  --name exchange --api http://127.0.0.1:9140

# 4. The wallet server. The token needs 32 characters or more.
export ZOKA_RPC_TOKEN="$(openssl rand -hex 32)"
zoka quantic-pq-wallet-rpc --network testnet --dir ./zoka/wallet --name exchange \
  --password-env ZOKA_WALLET_PASSWORD --rpc-token-env ZOKA_RPC_TOKEN \
  --api http://127.0.0.1:9140 --rpc-bind 127.0.0.1:18090
```

The first start reads the whole chain once (seconds to a few minutes); after
that the server reads only new blocks, every 10 seconds. While it runs, the
wallet is locked: the command-line wallet cannot open it at the same time.

## 3. Calling the server

JSON-RPC 2.0 over HTTP `POST /json_rpc` (or `/`), with
`Authorization: Bearer <token>`. Batches of up to 50 requests are accepted;
requests without an `id` are notifications and get no reply.

```sh
curl -s http://127.0.0.1:18090/json_rpc \
  -H "Authorization: Bearer $ZOKA_RPC_TOKEN" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"get_info"}'
```

**Units.** Amounts are whole numbers of atoms: **1 ZKA = 1,000,000 atoms**.
Customer tags are 64-bit numbers sent and returned **as decimal strings**,
because they can exceed 2^53 and JavaScript would round them.

## 4. Deposits

Each customer gets an **integrated address**: your wallet's address plus that
customer's tag (8 bytes). The payer's wallet writes the tag into the payment's
memo, encrypted like the whole note, so a customer cannot forget it the way
they can forget a memo or destination tag.

1. When a customer asks for a deposit address, call `make_integrated_address`
   with your own tag for that customer (for example their internal id), or
   without one for a random tag. Store the tag–customer pair.
2. Poll `get_deposits` (for example with `min_height` = the last height you
   processed minus 30, to cover reorganisations).
3. Credit each deposit **once**, keyed by its `deposit_id`, when it has the
   confirmations your policy requires (see section 6). `tag` tells you whose it
   is; `null` means a payment to your plain address without a tag: handle it
   manually.

A deposit address looks like `zqtesti2…` on the testnet and `zqi2…` on the
mainnet, about 70 characters. All the integrated addresses of one wallet share
its short address, so anyone can see they belong to the same wallet, as with
Monero's integrated addresses. Customers do not learn each other's deposits.

## 5. Withdrawals

Call `transfer` with a `request_id` you generate and store **before** the call
(1 to 64 letters, digits or `-_.:`):

- If the call times out or your backend restarts, call `transfer` again with
  the **same** `request_id` and the same parameters: you get the same withdrawal
  back, never a second payment.
- Reusing a `request_id` with other parameters is refused (`-32004`).
- Follow it with `get_withdrawal`: `pending` → `confirmed` (with its height,
  confirmations and `finalized`). A withdrawal the node refused is `rejected`;
  one not mined within 30 blocks (about an hour) is `expired`, and its funds
  are spendable again: issue a new withdrawal with a **new** `request_id`.

Several withdrawals can be pending at once: each spends its own notes, and
change comes back when it is mined. A payment spends at most 8 notes; the fee
is fixed by how many it spends: **0.0002, 0.0004 or 0.0008 ZKA** (2, 4 or 8
notes). `estimate_fee` tells you which before you send. Destinations can be
full, short or integrated addresses; the server checks a short or integrated
one against the full address your node returns.

## 6. Confirmations and finality

Blocks come every **120 seconds** on average. Two protections apply:

- **Confirmations**: `confirmations` counts the block itself (1 = just mined).
- **Checkpoints**: three team keys sign checkpoints (Ed25519 and ML-DSA-65,
  two of three needed), and nodes never reorganise below one. A deposit or a
  withdrawal with `finalized: true` sits at or below the latest checkpoint your
  node follows (`checkpoint_height` in `get_info`).

A reasonable policy: credit deposits when `finalized` is true, or after 20
confirmations (about 40 minutes), whichever your risk team prefers.

## 7. Method reference

| Method | Params | Result |
| --- | --- | --- |
| `get_info` | — | `network`, `wallet_height`, `node_tip`, `synced`, `lag`, `checkpoint_height`, `version` |
| `get_balance` | — | `spendable_atoms`, `immature_atoms`, `reserved_atoms`, `total_atoms`, `max_sendable_atoms`, `notes`, `pending_withdrawals` |
| `get_address` | — | `address` (short), `full_address` |
| `make_integrated_address` | `tag` (string or number, optional) | `integrated_address`, `tag`, `address` |
| `split_integrated_address` | `integrated_address` | `address`, `tag` |
| `validate_address` | `address` | `valid`, `kind` (`full`, `short` or `integrated`), `short_address`, `tag` |
| `get_deposits` | `min_height`, `max_height`, `tag`, `include_coinbase` (all optional) | `wallet_height`, `deposits[]`: `deposit_id`, `txid`, `height`, `amount_atoms`, `tag`, `confirmations`, `finalized`, `coinbase`, `spent` |
| `get_transfer_by_txid` | `txid` | `deposits[]` and the `withdrawal` with that txid, if any |
| `estimate_fee` | `amount_atoms` | `enough`, `fee_atoms`, `inputs` (or `reason`) |
| `transfer` | `request_id`, `destination`, `amount_atoms`, `memo` (optional, up to 512 bytes with the tag) | the withdrawal, as `get_withdrawal` |
| `get_withdrawal` | `request_id` | `request_id`, `txid`, `destination`, `amount_atoms`, `fee_atoms`, `tag`, `memo`, `status`, `height`, `confirmations`, `finalized`, `reason` |
| `get_withdrawals` | — | `withdrawals[]` |

Errors use the JSON-RPC codes (`-32700` parse, `-32600` request, `-32601`
method, `-32602` params, `-32603` internal) and these:

| Code | Meaning |
| --- | --- |
| `-32001` | The wallet is not in sync with its node yet: retry later. |
| `-32002` | Not enough spendable funds within the 8-note limit. |
| `-32003` | The destination is not a valid address, or the node does not know that short address. |
| `-32004` | The `request_id` was used for another withdrawal. |
| `-32005` | The node refused the withdrawal (the reason says why). |

## 8. Security

- Keep the wallet server and its node on a private network; never expose
  either to the internet.
- Keep only the float you need in the hot wallet. Move the rest to a wallet
  whose recovery phrase stays offline.
- The recovery phrase restores the wallet; the files next to it
  (`<name>.json.light`, `<name>.json.rpc-withdrawals`) are a cache and the
  withdrawal record. Back up the wallet folder, and the phrase offline.
- Rotate the RPC token by restarting the server with a new one.

## 9. Testing on the testnet

- Test coins have no value: get them from the faucet at
  <https://faucet.zoka.network> (10 ZKA a day), or mine them.
- Explorer: <https://zokascan.cc>. Pay your own integrated address from
  ZSilent, ZKAPriv or the web wallet, and watch the deposit arrive with its tag.

## 10. Limits to know

- A block holds at most 16 payments: about 11,500 payments a day for the whole
  network. A network upgrade may raise it before the mainnet.
- Payments are private: the chain shows no amounts or addresses. What a payment
  publishes is described in the whitepaper.

Questions: write to **zokanetwork@proton.me**, or open an issue at
<https://github.com/zoka-network/zokanetwork/issues>. Report a vulnerability
only by mail, never in a public issue.
