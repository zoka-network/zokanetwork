# Zoka Network — Roadmap

Última actualización: 2026-05-27

---

## Fase A — Core Protocol: Flujo Correcto de TX Privadas ✅ COMPLETADA

**Objetivo:** implementar el ciclo de vida completo de una transacción privada con reserva de saldo, estado intermedio y protección anti-doble-gasto persistente.

### A1 — Estado intermedio `reserved` en notas privadas ✅

```
unspent → reserved (TX en mempool) → spent (bloque confirmado)
                 ↘ unspent (TX rechazada o expirada)
```

Implementado: campos `reserved: bool` y `reserved_by_tx: Option<String>` en `PrivateNote` (`src/private_wallet/mod.rs`). `release_reserved_notes(tx_hash)` revierte reservas.

### A2 — Balance reportado: disponible / pendiente / confirmado ✅

```
available = notas confirmadas y no reservadas  (private_balance)
pending   = notas reservadas en TXs sin confirmar (pending_balance)
locked    = fondos inmaduros de coinbase (locked_rewards_for_address)
```

`sync_balance_cache()` en `src/private_wallet/mod.rs` implementa esto correctamente.

### A3 — Persistencia de nullifiers en estado de la cadena ✅

`spent_nullifiers: Vec<String>` persiste en el archivo de wallet. `rebuild_nullifier_set()` reconstruye el set en memoria al arrancar. `clear_committed_local_private_txs` también limpia sidecar files (fix commit `b6d5f3f`).

### A4 — Pre-validación de prueba ZK en mempool ✅

`validate_inbound_private_transaction()` verifica: nullifiers no gastados, pruebas de rango en outputs, y ZK proof del circuito. Se ejecuta en el endpoint `POST /private/tx` antes de aceptar la TX.

### A5 — Build y pruebas de integración ✅

- [x] Compilar binario Linux: `cargo build --release --bin zoka`
- [x] Prueba 1: TX `577f851badf6ce54f7710afd15b36c2a` creada → minada en bloque 77 → propagada via P2P
- [x] Prueba 2: balance reservado durante TX pendiente, descontado tras confirmación
- [x] Prueba 3: nullifiers persistidos en wallet file y reconstruidos al reiniciar
- [x] Prueba 4: TX anónima verificada: ring signatures (3 miembros), Pedersen commitments, stealth addresses, range proofs
- [x] Deploy a nodo bootstrap y RPC vía GitHub Actions

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

- [x] Actualizar `Constants.kt` con RPC DO: `http://157.230.5.103:3000/` (ya hecho).
- [x] TX de 10 ZKA enviada desde tesorería → confirmada en bloque 77 → propagada por P2P → cuerpo accesible en ambos nodos.
- [x] Corregir 4 bugs críticos en `Ristretto255.kt` — descifrado de outputs privados ahora funciona (verificado con vector TX 577f851b, Output 1 MATCH).
- [ ] Rebuild APK debug y verificar en dispositivo SM-A135M:
  - [ ] Dashboard muestra altura de bloque real y estado de red.
  - [ ] Seed → address correcta (vector: `brief simple... → zka1DvXCz...`).
  - [ ] Scan key hex → misma address.
  - [ ] Recepción: wallet del destinatario detecta los 10 ZKA recibidos en bloque 77.
  - [ ] Envío ZKA: crear TX → transmitir al nodo → ver en explorer.

### C2 — Escaneo de outputs privados

- Implementar descifrado local de outputs usando `scan_key`.
- Escanear bloques raw del nodo y detectar outputs propios.
- Mostrar balance real basado en outputs descifrados (no solo coinbase maduros).

### C3 — Balance con estados y actualización en tiempo real

- Conectar los tres estados de balance (Fase A) en la app Android.
- Mostrar "Disponible", "Pendiente" y "Bloqueado" en el dashboard.
- **Actualización proactiva de balances:** Los balances (disponible y bloqueado) deben
  refrescarse automáticamente sin necesidad de cerrar y reabrir la app. Reducir el intervalo
  de polling a ~15–20 s cuando hay actividad reciente, y hacer que el refresco sea visible
  (indicador de carga) para que el usuario sepa cuándo se está sincronizando.

### C3b — Recent Activity: entradas, salidas y hash de TX

- Registrar tanto transacciones **entrantes** como **salientes** en la sección Recent Activity.
- Cada ítem pendiente debe mostrar el **hash de la TX** (no solo un enlace al explorador).
- El enlace al explorador debe abrirse sobre el hash de la TX correcta
  (actualmente lleva al explorador sin contexto de hash).
- Una vez confirmada la TX, marcarla como confirmada en la lista sin reiniciar la app.

### C3c — Fix Reset de wallet (Login y Settings)

- El reset de wallet está roto tanto en la pantalla de Unlock (LockScreen) como en Settings.
- Verificar y corregir `WalletResetCoordinator` / `clearAllData()` para que limpie
  correctamente todo el estado local y lleve al onboarding.
- Probar el flujo completo: reset desde Lock → onboarding → restaurar wallet.

### C3d — UI de Unlock: rediseñar "Forgot Password"

- El botón "Forgot Password?" en LockScreen actualmente es solo texto rojo simple.
- Rediseñarlo con mejor presentación: ícono + texto descriptivo, diálogo de confirmación
  más elaborado que explique las consecuencias del reset antes de ejecutarlo.

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

1. ✅ **A1–A5** — protocolo completo, build, pruebas, deploy
2. ✅ **Ristretto255.kt** — 4 bugs corregidos, descifrado de outputs verificado
3. **C1** — rebuild APK + prueba end-to-end en SM-A135M
4. **C3c** — fix reset de wallet (Login + Settings) + rediseño Forgot Password (C3d)
5. **C3** — balance en tiempo real (polling ~15 s, indicador de carga)
6. **C3b** — Recent Activity: entradas, salidas, hash de TX en pendientes
7. **B1–B2** — ZSilent Core conectado al nuevo balance
8. **C2** — escaneo automático de bloques para outputs privados en Android
9. **D1** — rotar token DO y fix remote zkapriv
10. **B3–B8** — pulido de ZSilent Core
11. **C4–C6** — Settings avanzados, branding final, release APK firmado
12. **D2–D3** — monitoreo y documentación
