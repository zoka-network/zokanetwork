# Infrastructure Logs Policy

Official ZOKA infrastructure should log only what is needed to operate the
network safely.

## Allowed

- Service start and stop events.
- Aggregate peer counts.
- Chain height and sync health.
- Error categories without wallet secrets.
- Resource usage needed for reliability.

## Not Allowed

- Wallet recovery phrases.
- Spend keys, scan keys, or exported wallet secrets.
- User passphrases.
- Private transaction plaintext.
- Logs that map a user identity to a wallet or peer unless explicitly required
  for an operator-owned system.

## Retention

Public testnet logs should be short-lived unless needed to debug a reported
issue. Any shared logs must be reviewed for secrets first.

## Operator Guidance

Run with normal info-level logging. Enable debug logs only for short diagnostic
windows and only after confirming that no wallet secrets will be printed.
