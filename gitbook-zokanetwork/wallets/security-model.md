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

The Android implementation stores account data through encrypted local storage
and uses Android Keystore-backed encryption where available. Sensitive screens,
including recovery and key-related views, are protected by the app lock flow.

The app also enables screenshot protection in normal builds so wallet screens,
seed phrases, and scan keys do not appear in Android recents, screenshots, or
screen recordings.

## ZSilent Core Protection

ZSilent Core avoids passing secrets through command-line arguments by using
environment-backed process channels for wallet and node passwords.

The desktop architecture keeps the UI as a coordinator. Wallet lifecycle,
node control, mining control, encrypted settings, log redaction, and sensitive
value handling are separated into explicit service modules. Runtime logs are
sanitized so lines containing obvious secret terms are redacted before display.

## What Users Still Control

The security model depends on user behavior as well as code:

- Keep recovery phrases offline.
- Do not paste scan keys, spend keys, or wallet passwords into websites.
- Treat exported wallet backups as sensitive even when encrypted.
- Lock the device when not in use.
- Prefer local node operation when possible.
