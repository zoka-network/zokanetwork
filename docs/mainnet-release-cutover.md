# ZOKA Mainnet Release Cutover Checklist

## Build and provenance

- [ ] Release commit frozen (`v1.0.0-rcN` tag or equivalent).
- [ ] Windows build completed (`--release --features randomx`).
- [ ] Linux build completed (`--release --features randomx`).
- [ ] `scripts/mainnet_readiness_check.ps1 -BuildRelease` completed with
  `Failures: 0`.
- [ ] `scripts/package_release.ps1` executed on frozen commit.
- [ ] `SHA256SUMS.txt` generated.
- [ ] GPG signatures generated and verified.

## Verification

- [ ] Third-party reproducible build check passed.
- [ ] `docs/release-verification.md` steps validated end-to-end.
- [ ] Artifacts and manifest published.

## Runtime smoke

- [ ] `zokahd --help`
- [ ] `zoka wallet-create --help`
- [ ] `zoka wallet-restore-from-seed --help`
- [ ] `zoka wallet-balance --help`
- [ ] `zoka tx-send --help`
- [ ] `zoka mine --help`
