# Wallet Security

ZOKA wallets are self-custodial. The wallet controls keys locally and public RPC
nodes provide chain data.

## Local Secrets

- Recovery phrase.
- Scan key.
- Spend key.
- Wallet password.
- Encrypted local wallet files.

## ZKAPriv Protection

ZKAPriv uses PIN or biometric unlock, encrypted local storage, automatic
session locking, and screenshot protection.

## ZSilent Core Protection

ZSilent Core avoids passing secrets through command-line arguments by using
environment-backed process channels for wallet and node passwords.

