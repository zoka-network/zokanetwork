# Addresses and Keys

ZOKA wallets use a two-key account model.

## Key Types

| Key | Purpose |
|---|---|
| Scan key | Detect incoming funds. |
| Spend key | Authorize spending. |
| Recovery phrase | Restore the account keys. |

## Address Forms

- `zka1...` is the user-facing receive address.
- Internally, the private payment protocol can represent payment material as
  scan and spend public keys.

Public docs should prefer `zka1...` when speaking to wallet users.

## Important Warning

A scan key is sensitive. It can reveal wallet-related detection capability and
must not be posted publicly. ZokaScan's scan-key mining reward helper runs in
the browser and does not send the key to the server.

