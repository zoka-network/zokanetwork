# Abstract

ZOKA Network is a privacy-first proof-of-work blockchain with self-custodial
wallets and zero-knowledge proof infrastructure.

The network is designed so that:

- Private transfers hide sender, receiver, and amount from the public explorer.
- Wallet balances are reconstructed locally by the user's wallet.
- Nullifiers prevent double spends without revealing which output was spent.
- Commitments and proof data keep private activity verifiable.
- Miners and nodes can participate through the public network.

ZOKA is not a centralized privacy service. Public RPC nodes provide chain data;
they are not custodians and do not need wallet secrets.

The practical model is local-first: wallet applications use local secret
material to scan chain data and prepare private transfer flows. Public
infrastructure exists to synchronize, validate, mine, and observe network state,
not to hold or reveal user balances.
