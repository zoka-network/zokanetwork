# ZOKA Ceremony Execution Board (Mainnet v1)

Operational board for real external run. Update status as each round completes.

## Configuration

- Ceremony dir: `.zoka/ceremony-mainnet-v1`
- Transcript: `.zoka/ceremony-mainnet-v1/transcript.json`
- Label: `zoka-mainnet-v1`
- Round plan file: `.zoka/ceremony-mainnet-v1/round-plan.json`

## Round assignments

| Round | Contributor ID | Status | Package | Return | Finalized |
|---|---|---|---|---|---|
| R1 | contrib-a | prepared (live) | `out/zoka-round-R1-contrib-a.zip` | pending | no |
| R2 | contrib-b | pending | - | pending | no |
| R3 | contrib-c | pending | - | pending | no |
| R4 | contrib-d | pending | - | pending | no |
| R5 | contrib-e | pending | - | pending | no |

## Commands

### Prepare a round

```powershell
powershell -ExecutionPolicy Bypass -File scripts/init_ceremony_round.ps1 `
  -CeremonyDir .zoka/ceremony-mainnet-v1 `
  -CurrentDir .zoka/ceremony-mainnet-v1/current-R1 `
  -RoundNumber 2 `
  -ContributorId contrib-b `
  -Label zoka-mainnet-v1 `
  -Force
```

### Export package for sending

```powershell
powershell -ExecutionPolicy Bypass -File scripts/export_ceremony_round_package.ps1 `
  -RoundDir .zoka/ceremony-mainnet-v1/round-R2 `
  -OutZip .zoka/ceremony-mainnet-v1/out/zoka-round-R2-contrib-b.zip
```

### Process contributor return, finalize, and open next round

```powershell
powershell -ExecutionPolicy Bypass -File scripts/process_ceremony_round_return.ps1 `
  -CeremonyDir .zoka/ceremony-mainnet-v1 `
  -RoundNumber 1 `
  -ContributorId contrib-a `
  -ReturnPath C:\returns\zoka-round-R1-contrib-a-return.zip `
  -Label zoka-mainnet-v1 `
  -InitNextRound `
  -NextContributorId contrib-b `
  -ExportNextRoundPackage
```

### Initialize and use round plan automation

Initialize plan:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/init_ceremony_round_plan.ps1 `
  -OutPath .zoka/ceremony-mainnet-v1/round-plan.json `
  -Label zoka-mainnet-v1 `
  -Contributors contrib-a,contrib-b,contrib-c,contrib-d,contrib-e
```

Process return using plan:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/process_ceremony_return_with_plan.ps1 `
  -CeremonyDir .zoka/ceremony-mainnet-v1 `
  -PlanPath .zoka/ceremony-mainnet-v1/round-plan.json `
  -RoundNumber 1 `
  -ReturnPath C:\returns\zoka-round-R1-contrib-a-return.zip `
  -ExportNextRoundPackage
```

Current R1 package hash:
- `eb4c0490d08b25052690bd6953b781907d8dab6f20d7dab429df8c2bf73e6853`

### Closeout after R5

```powershell
powershell -ExecutionPolicy Bypass -File scripts/ceremony_closeout.ps1 `
  -Transcript .zoka/ceremony-mainnet-v1/transcript.json `
  -FinalDir .zoka/ceremony-mainnet-v1/current-R5 `
  -BundleDir .zoka/ceremony-mainnet-v1/public_bundle `
  -MinUniqueContributors 5 `
  -ForceBundle `
  -BuildRelease
```
