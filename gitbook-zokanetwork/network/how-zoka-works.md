# How Zoka Works

At a high level:

1. A wallet owns a recovery phrase and derives wallet keys.
2. The wallet scans public chain data and detects outputs that belong to it.
3. When sending, the wallet builds a private transaction locally.
4. The network validates the transaction without exposing private participants
   or amounts.
5. Miners include valid transactions in proof-of-work blocks.
6. The receiver's wallet later detects and decrypts the incoming output.

## Wallets

Wallets are the privacy boundary. ZKAPriv and ZSilent Core keep secrets local
and reconstruct history from public chain data.

## Nodes

Nodes validate blocks and expose chain state. Public RPC nodes are read-only
infrastructure and must not receive wallet seeds, scan keys, or spend keys.

## Miners

Miners create blocks and receive block rewards directly to their `zka1...`
wallet address when configured.

## Proof Generation

Private transaction proof generation is performed by wallet-side or local client
code. Public nodes validate what the protocol requires; they do not need access
to the sender's wallet secrets.

