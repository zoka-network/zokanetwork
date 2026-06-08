# Audits and Review

ZOKA should treat the following as release-critical:

- Consensus rules.
- Private transaction proof verification.
- Trusted setup or CRS material.
- Wallet key derivation.
- Android and desktop wallet storage.
- Release signing keys.
- RPC endpoints that accept transaction data.

## Current Public Guidance

Do not claim third-party audit coverage unless an actual audit report exists and
is linked. Internal review, tests, and staged validation should be described as
such.

## Review Checklist

Before a release, review:

- Consensus constants and activation heights.
- RandomX build configuration.
- Private transaction proof verification.
- Fee and reward accounting.
- Nullifier handling.
- Wallet backup and restore flows.
- Android release signing and screenshot protection.
- Desktop secret passing and log redaction.
- RPC endpoints that accept transaction data.
- Explorer behavior around private data.

## Public Language

Use precise language. It is accurate to say that ZOKA is privacy-oriented,
self-custodial, and designed so wallet secrets remain local in the documented
flows. Do not claim formal audit coverage, permanent anonymity, or absolute
trace-proof behavior unless the project has published evidence for that exact
claim.
