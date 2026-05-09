# ZOKA Ceremony Round Checklist

Round: `R<round-number>`
Contributor: `<contributor-id>`
Coordinator: `<coordinator-id>`

## Pre-send (Coordinator)

- [ ] `prepare-adapter` executed successfully.
- [ ] Input files exist in `input/*.params`.
- [ ] Round directory includes:
  - [ ] `ROUND_SUMMARY.txt`
  - [ ] `CHECKLIST.md`
  - [ ] `ATTESTATION_contrib-<id>.md`
- [ ] Contributor ID and round number reviewed.

## Execution (Contributor)

- [ ] Input package received intact.
- [ ] Contribution executed with fresh entropy.
- [ ] Output `*.params` generated.
- [ ] Attestation document completed.

## Post-return (Coordinator)

- [ ] Output files saved in `output_from_contributor/`.
- [ ] `finalize-adapter` succeeded.
- [ ] Transcript updated with correct `--contributor`.
- [ ] `verify --transcript --final-dir` succeeded after round.
- [ ] Round attestation archived.

## Round closure metadata

- Before digest: `<hex>`
- After digest: `<hex>`
- Transcript index: `<n>`
- Notes: `<optional>`
