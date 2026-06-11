# Developers

ZOKA is developed across several repositories. The public docs should stay
aligned with code and releases. If a claim is not implemented, label it as
planned or omit it.

## Main Languages

- Rust for the core node, CLI, wallet logic, mining, state, and RPC.
- Kotlin and Compose for ZKAPriv and ZSilent Core.
- TypeScript, React, and Tailwind for website and explorer frontends.

## Development Principles

Developers should preserve the self-custody boundary:

- Wallet secrets stay in wallet surfaces and local binaries.
- RPC servers provide chain data and accept transactions; they do not custody
  recovery phrases, scan keys, or spend keys.
- Explorers show public chain metadata, not private wallet histories.
- Consensus changes require tests, release notes, and operator coordination.

When adding UI features, prefer existing APIs and local service boundaries over
new ad hoc flows that move secrets into frontends or public infrastructure.
