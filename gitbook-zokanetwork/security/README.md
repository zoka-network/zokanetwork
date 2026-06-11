# Security

ZOKA's security model combines cryptography, proof-of-work consensus, local
wallet custody, encrypted storage, and conservative operational practices.

## Security Philosophy

- Keep wallet secrets local.
- Validate public chain data.
- Avoid transparent leakage of private transfer metadata.
- Treat release, setup, and signing materials as sensitive.

## Security Posture

The project combines several sensitive areas: proof-of-work consensus,
zero-knowledge proof validation, wallet key derivation, encrypted local storage,
mobile release signing, desktop process orchestration, and public RPC surfaces.

Each area has a different failure mode. A public website bug should not expose
wallet secrets. A desktop log should not contain a recovery phrase. A node
should reject invalid blocks even if a wallet or explorer displays them.

## Practical Rule

When in doubt, keep secrets local, keep public data minimal, and document the
exact release behavior instead of making broad security claims.
