# Threat Model

ZOKA is designed to reduce public-chain analysis of private user payments.

## Resistance Goals

- **Chain analysis resistance**: private transaction records do not expose
  sender, receiver, or amount.
- **Address tracking resistance**: the explorer does not expose full public
  address histories for private wallet identities.
- **Balance tracking resistance**: balances are wallet-side and not public
  explorer data.
- **Double-spend resistance**: nullifiers prevent reuse of spent notes.

## Metadata Considerations

Privacy is not only cryptography. Users should also consider:

- RPC endpoint choice.
- Network timing.
- Reusing public identities with the same receive address.
- Publishing screenshots with balances or QR codes.
- Sharing scan keys or recovery material.

## Non-Goals

The public chain still exposes block production, timestamps, proof-of-work,
hashes, commitments, nullifiers, and other consensus data.

