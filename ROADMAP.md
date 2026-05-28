# Zoka Network — Roadmap

Última actualización: 2026-05-27

---

## Fase A — Core Protocol: Flujo Correcto de TX Privadas

**Objetivo:** implementar el ciclo de vida completo de una transacción privada con reserva de saldo, estado intermedio y protección anti-doble-gasto persistente.

### A1 — Estado intermedio `reserved` en notas privadas

Hoy las notas pasan de `unspent → spent` al instante de crear la TX.
El flujo correcto es:

```
unspent → reserved (TX en mempool) → spent (bloque confirmado)
                 ↘ unspent (TX rechazada o expirada)
```

- Agregar campo `reserved: bool` (o estado enum `Unspent / Reserved / Spent`) en `PrivateNote`.
- Al crear una TX: marcar notas como `reserved`, no `spent`.
- Al confirmar bloque: marcar notas como `spent`.
- Al rechazar o expirar TX: revertir notas a `unspent`.

Archivo: `src/private_wallet/mod.rs` (~línea 2696)

### A2 — Balance reportado: disponible / pendiente / confirmado

Hoy `sync_balance_cache` solo suma notas con `spent = false`.
Debe exponer tres valores:

```
available = notas confirmadas y no reservadas
pending   = notas reservadas en TXs aún no confirmadas
locked    = (futuro) fondos inmaduros por regla de madurez de coinbase
```

Archivo: `src/private_wallet/mod.rs` (~línea 2313)

### A3 — Persistencia de nullifiers en estado de la cadena

Hoy el `NullifierSet` es in-memory en `PrivateTransactionPool` — se pierde al reiniciar el nodo.

- Guardar nullifiers gastados en el estado persistente al confirmar cada bloque.
- Al arrancar el nodo: reconstruir `NullifierSet` desde el historial de bloques confirmados.
- El chequeo de doble-gasto debe consultar siempre el set persistido, no solo el in-memory.

Archivos: `src/transaction/nullifier.rs`, `src/state/chain.rs`, `src/state/mod.rs`

### A4 — Pre-validación de prueba ZK en mempool

Antes de aceptar una TX privada en el pool, el nodo debe:

1. Verificar que la prueba ZK incluida es válida (no solo bien formada).
2. Verificar que los nullifiers de las notas de entrada no están ya gastados.
3. Solo entonces aceptar la TX como `pending`.

Si la validación falla → rechazar y no reservar nada.

Archivo: `src/private_wallet/mod.rs` (~línea 2193), `src/state/chain.rs`

### A5 — Build y pruebas de integración

Una vez implementados A1–A4:

- [ ] Compilar binario Linux: `cargo build --release --bin zoka`
- [ ] Prueba 1: crear TX → pool → confirmar → balance descontado correctamente
- [ ] Prueba 2: TX rechazada → notas liberadas → balance restaurado
- [ ] Prueba 3: intento de doble gasto → nullifier lo rechaza
- [ ] Prueba 4: reiniciar nodo → nullifier set reconstruido → doble gasto sigue rechazado
- [ ] Deploy a nodo bootstrap y RPC vía GitHub Actions

---

## Fase B — ZSilent Core (Desktop)

Estado base: compilable, pantallas principales funcionales, flujo de onboarding casi completo.

### B1 — Conectar nuevo balance (disponible / pendiente)

- Actualizar `WalletViewModel` para mostrar los tres valores de la Fase A2.
- Dashboard: mostrar "Disponible", "Pendiente confirmación" y "Bloqueado".

### B2 — Tracking de estado de TX en UI

- Mostrar estado de cada TX: `Enviada → En mempool → Confirmada / Rechazada`.
- Al rechazarse una TX: mostrar notificación y saldo restaurado.

### B3 — Onboarding y wallet (pendientes Phase 2/3)

- Importar wallets adicionales después del onboarding.
- Address book.
- Auto-lock por inactividad.
- Mejor gestión multi-wallet.
- Mensajes de validación/error más claros.
- UX de "backup confirmado" más fuerte.

### B4 — Send Flow (pendientes Phase 4)

- Validación de dirección más robusta antes de llamar al backend.
- Modal de confirmación más explícito.
- Tracking de confirmado/rechazado directo desde datos de inclusión en bloque.
- Selector de prioridad/fee opcional.

### B5 — Control de nodo (pendientes Phase 5)

- Detectar nodo ya corriendo al abrir la app.
- Restart de nodo desde UI.
- Configurar puertos RPC/P2P y directorio de datos.
- Mostrar peers, CPU/RAM/disco.

### B6 — Mining (pendientes Phase 6)

- Persistir preferencia de auto-start mining.
- Accounting de fondos minados por wallet.
- Datos de madurez/próximo-unlock por wallet.
- Pausa/reanudación y presets de potencia desde UI.

### B7 — Privacidad y seguridad (Phase 7)

- Encriptar toda la configuración local que pueda revelar patrones de uso.
- Limpieza de memoria en campos de contraseña.
- Checklist de auditoría de privacidad antes del release.
- Asegurar que JSON exportado sea siempre encriptado.

### B8 — Packaging MSI (Phase 8)

- Ícono de aplicación y branding del instalador.
- Validar MSI en entorno Windows limpio.
- Checklist de verificación de release.
- Plan para Linux/macOS.
- Mecanismo de actualización.

---

## Fase C — ZKAPriv (Android)

Estado base: identidad Zoka completa, derivación alineada con el backend Rust, build debug exitoso (APK en `zkapriv-debug.apk`), instalado en SM-A135M.

Derivación activa verificada:
```
scan_key  = BIP-39 entropy raw
spend_key = SHA256("zoka-private-wallet-spend-v1" || 0x1f || scan_key)
address   = zka1 + Base58(scan_pub || spend_pub || checksum)
```

### C1 — Prueba end-to-end contra nodo mainnet

- Actualizar `Constants.kt` con RPC DO: `http://157.230.5.103:3000/` (ya hecho).
- Rebuild APK debug con nuevo endpoint.
- Probar en dispositivo SM-A135M:
  - [ ] Dashboard muestra altura de bloque real y estado de red.
  - [ ] Seed → address correcta (vector: `brief simple... → zka1DvXCz...`).
  - [ ] Scan key hex → misma address.
  - [ ] Envío ZKA: crear TX → transmitir al nodo → ver en explorer.
  - [ ] Recepción: recibir fondos → balance actualizado.

### C2 — Escaneo de outputs privados

- Implementar descifrado local de outputs usando `scan_key`.
- Escanear bloques raw del nodo y detectar outputs propios.
- Mostrar balance real basado en outputs descifrados (no solo coinbase maduros).

### C3 — Balance con estados (conectar Fase A)

- Una vez implementada la Fase A, conectar los tres estados de balance en la app Android.
- Mostrar "Disponible", "Pendiente" y "Bloqueado" en el dashboard.

### C4 — Settings

- Configurar RPC/bootstrap manualmente desde la app.
- Opción de conectar a nodo local.

### C5 — Branding final

- Ícono de la app definitivo.
- Splash screen.
- Paleta de colores y tipografía final.
- Nombre en Play Store / distribución directa APK.

### C6 — Release APK firmado

- Configurar keystore de producción.
- Build release firmado.
- SHA256 publicado para verificación.

---

## Fase D — Infraestructura y Mantenimiento

### D1 — Seguridad inmediata

- [ ] Rotar DO API token (`dop_v1_1062...`) — token actual debe invalidarse.
- [ ] Corregir remote del repo zkapriv (actualmente apunta a repo incorrecto).

### D2 — Monitoreo de nodos

- Bootstrap (`159.223.119.216`): P2P + mining, RandomX light mode.
- RPC (`157.230.5.103`): API only, sincroniza vía P2P desde bootstrap.
- Explorer: `zokascan.cc` → Vercel (`prj_PD2Z79dLOW84cMdibWWYXhlLZ998`) → proxy `/rpc` → RPC node.
- Agregar alertas automáticas si algún nodo cae (uptime check).

### D3 — Documentación pública

- Actualizar `docs/` para reflejar mainnet (reemplazar referencias a testnet).
- Publicar guía de operación de nodo para mainnet.
- Publicar proceso de verificación de release para binarios mainnet.

---

## Orden de prioridad

1. **A1–A4** — flujo correcto de TX privadas en el protocolo (fundamento de todo)
2. **A5** — build, pruebas y deploy
3. **C1** — prueba end-to-end de ZKAPriv contra mainnet
4. **B1–B2** — ZSilent Core conectado al nuevo balance
5. **C2** — escaneo de outputs privados en Android
6. **D1** — rotar token DO y fix remote zkapriv
7. **B3–B8** — pulido de ZSilent Core
8. **C3–C6** — branding y release de ZKAPriv
9. **D2–D3** — monitoreo y documentación
