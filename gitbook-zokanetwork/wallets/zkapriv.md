# ZKAPriv Android

ZKAPriv is the Android self-custodial wallet for ZOKA Network.

![ZKAPriv screenshots](https://raw.githubusercontent.com/ZokaNetwork/zkapriv/main/docs/screenshots/zkapriv-v1.0.0-screenshots-collage.png)

## Capabilities

- Create a new wallet.
- Restore from recovery phrase.
- Import with scan key where supported by the flow.
- Send and receive ZKA privately.
- Scan on-chain data to recover balance and recent activity.
- Configure RPC endpoints.
- Protect the app with PIN, biometrics, encrypted local storage, session lock,
  and screenshot protection.

## What Happens Locally

ZKAPriv is designed as a self-custody mobile wallet, not as a hosted wallet
frontend. The private transfer path uses the Android application and its native
`zoka_mobile` engine to build private transactions on the device before they
are submitted to a node RPC endpoint.

The RPC node provides public chain data and accepts transaction payloads. In
the normal self-custody flow, it does not need the user's recovery phrase, scan
key, or spend key.

## Chain Scanning

Private balances are recovered by scanning chain data with local wallet
material. ZKAPriv downloads the relevant public/private chain data from RPC,
then uses the account's local detection material to classify owned outputs,
recover balance, and rebuild recent activity.

This is why a restored wallet can recover its balance and history after the
user imports the recovery phrase and lets the wallet scan the chain.

## Privacy Notes

ZKAPriv reconstructs wallet state locally. The public RPC provides chain data;
it does not need the user's seed phrase, scan key, or spend key.

Sensitive wallet material is stored locally with Android security primitives,
including encrypted preferences and Android Keystore-backed encryption where
available. The application also uses app locking and screenshot protection to
reduce accidental exposure through recents, screen recording, or shared device
access.

## Compatibility

ZKAPriv v1.0.0 targets Android 8.0 or newer. The release is intended for ARM
Android devices, with native wallet support available for `armeabi-v7a` and
`arm64-v8a`.

## Release Notes

ZKAPriv v1.0.0 is the public Android wallet release for Mainnet Zenith.
