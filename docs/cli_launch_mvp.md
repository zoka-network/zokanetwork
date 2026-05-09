# ZOKA CLI Launch Scope (MVP vs Post-Launch)

Objetivo: definir el alcance minimo de CLI/wallet para abrir red publica sin comprometer seguridad operativa ni experiencia basica de usuario.

## MVP obligatorio (antes de launch)

### 1) Wallet security baseline

- `create` con seed phrase mostrada una sola vez + confirmacion de respaldo.
- `restore-from-seed` con validacion estricta de checksum.
- `restore-from-keys` (scan/spend) para soporte view-only.
- `restore-height` para evitar rescans completos innecesarios.
- Encriptacion de wallet con passphrase obligatoria.
- `change-passphrase`.
- `backup` (export cifrado).
- `wipe` seguro (archivo + limpieza de memoria sensible en runtime).

### 2) Send flow seguro y predecible

- `send <address> <amount>` con confirmacion explicita antes de firmar/broadcast.
- `estimate-fee`.
- Politica de fees por `--priority low|normal|high` (no fee arbitraria libre en CLI de usuario).
- Soporte minimo de `send-many`.

### 3) Estado util para usuario

- `balance` separado en `unlocked` y `locked`.
- `sync-status`.
- `history [N]` con campos minimos operativos.

### 4) Privacidad de transporte (minimo)

- Proxy anonimo por defecto (Tor si disponible).
- `--allow-clearnet` explicito para degradar privacidad.
- `network-status` para saber si va por Tor/I2P/clearnet.

### 5) Integracion y automatizacion

- Modo no interactivo consistente por args/stdin/env.
- `--json` en comandos de consulta.
- Exit codes consistentes por tipo de error.
- `stdout` para datos y `stderr` para logs/errores.

## Post-launch (fase 2)

- `address new/list` con subdirecciones etiquetadas.
- `integrated-address` (si se mantiene payment-id en protocolo).
- `sweep-all`.
- Pruebas criptograficas completas:
  - `get-tx-key`, `check-tx-key`
  - `get-tx-proof`, `check-tx-proof`
  - `sign-message`, `verify-message`
- Flujo cold-wallet completo:
  - `export/import-outputs`
  - `export/import-key-images`
  - `create-unsigned-tx`, `sign-tx`, `submit-tx`
- Diagnostico avanzado:
  - `rescan-blockchain`, `rescan-spent`, `repair`, `log-level`.

## Criterio de aprobacion de launch

Launch bloqueado si falta cualquiera de:

1. Encriptacion obligatoria + restore confiable.
2. Send con confirmacion explicita y fee predecible.
3. Estado minimo (`unlocked/locked`, `sync-status`, `history`).
4. Transporte anonimo activo por defecto o bloqueo explicito de salida.
5. Salida JSON y codigos de error aptos para integracion.
