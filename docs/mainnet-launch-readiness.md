# ZOKA Mainnet Launch Readiness

## Operations gates

- [ ] `scripts/mainnet_readiness_check.ps1 -BuildRelease` termina con
  `Failures: 0`.
- [ ] Bug bounty active before launch.
- [ ] Operator and validator on-call roster confirmed.
- [ ] Incident bridge and escalation tree validated.
- [ ] Dry-run T-24/48h completed with evidence.

## Network and genesis gates

- [ ] `docs/mainnet-network.v1.json` published with launch-ready endpoints.
- [ ] `docs/genesis.mainnet.v1.json` published with final digest.
- [ ] `zoka genesis-verify docs/genesis.mainnet.v1.json` output matches announced digest.
- [ ] `zoka mainnet-network-lint docs/mainnet-network.v1.json` returns OK.

## Launch execution

- [ ] Launch window announced publicly.
- [ ] Mainnet started from final genesis.
- [ ] First blocks validated against expected supply curve.
- [ ] Post-launch status update published.
