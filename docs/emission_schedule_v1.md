# ZOKA Emission Schedule v1

Fuente canonica: `src/economics.rs` y `docs/economics.mainnet.v1.json`

Parametros:

- Initial reward: `12 ZKA`
- Halving interval: `1,051,200` blocks
- Max supply: `23,000,000 ZKA`
- Accounting unit v1: whole `ZKA`
- Target block time: `120` seconds
- Rule: `reward(height) = min(halving_reward(height), remaining_supply(height))`

Resultados:

- Minted total: `23,000,000 ZKA`
- Final rewarded block height: `4,078,399`
- First zero-reward block height: `4,078,400`

| Era | Start Height | End Height | Blocks | Reward (ZKA/block) | Minted In Row (ZKA) | Cumulative Minted (ZKA) | Note |
|---|---:|---:|---:|---:|---:|---:|---|
| 0 | 0 | 1,051,199 | 1,051,200 | 12 | 12,614,400 | 12,614,400 | full era |
| 1 | 1,051,200 | 2,102,399 | 1,051,200 | 6 | 6,307,200 | 18,921,600 | full era |
| 2 | 2,102,400 | 3,153,599 | 1,051,200 | 3 | 3,153,600 | 22,075,200 | full era |
| 3 | 3,153,600 | 4,078,399 | 924,800 | 1 | 924,800 | 23,000,000 | partial era capped by remaining supply |

## Nota operativa

La version v1 implementada no declara decimales atomicos. Si se decide agregar subunidades futuras, wallet, circuitos, fees, documentos y estado deben migrarse juntos antes de genesis.
