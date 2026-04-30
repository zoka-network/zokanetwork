# ZOKA Local RC Complete Status

Fecha: 2026-04-28

Este estado cierra el pipeline tecnico de release en esta maquina. No declara
mainnet publica viva: los endpoints aplicados son locales/no-publicos para
validar el flujo completo de genesis, network lint, package, checksums y firma.

## Resultado

- `scripts/mainnet_readiness_check.ps1`: `Failures: 0`.
- Genesis final local aplicado:
  `f0d9e1261a3dec08d754f63e1bcfd2663f6cedfd476657125bfaebe7aefd5745`.
- `zoka mainnet-network-lint docs/mainnet-network.v1.json --json`: OK.
- Paquete generado:
  `.zoka/test/release-complete/local-rc-complete/zoka-local-rc-complete-native.zip`.
- Checksums:
  `.zoka/test/release-complete/local-rc-complete/SHA256SUMS.txt`.
- Firma GPG:
  `.zoka/test/release-complete/local-rc-complete/SHA256SUMS.txt.asc`.
- Clave publica GPG:
  `.zoka/test/release-complete/local-rc-complete/zoka-release-signing.asc`.
- Fingerprint local de release:
  `4AB9 49CA 7255 79DF FF5D BE2F 7007 39CE F978 DD3C`.
- SHA256 del ZIP:
  `cc2ff5fee043cef6386529501c77f023b40006620764b554648d27ec37002ab8`.

## Material sensible

Las claves privadas no estan en el repo. Se generaron en:

```text
C:\Users\mderramus\zoka-secrets
```

No commitear ni publicar ese directorio.

## Advertencia

Para una mainnet publica real, reemplazar los endpoints locales
`127.0.0.1`/local-rc por seed nodes, onion/I2P, RPC y operadores reales antes
de anunciar launch.
