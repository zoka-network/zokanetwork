# Transport Privacy

ZOKA network configuration supports Tor, I2P, and TCP seed addresses. Public
network policy can prefer anonymous transport while still allowing explicit TCP
operation when needed.

## Testnet

Testnet may use TCP seed nodes during early bootstrapping so operators can debug
connectivity and sync behavior. Tor and I2P endpoints are preferred when they
are available.

## Mainnet Candidate

Mainnet candidate configuration should prefer anonymous transport and require an
explicit operator choice before relying on clearnet seed addresses.

## Operator Rules

- Publish only the addresses you intend peers to use.
- Avoid mixing wallet identities with node identity logs.
- Treat seed peer metadata as operational data, not user identity data.
- Use local firewall and host hardening even when anonymous transport is used.
