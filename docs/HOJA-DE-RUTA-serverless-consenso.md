# Hoja de ruta — ZOKA serverless: propagación, transferencias, persistencia y checkpoint canónico

> Documento vivo de auto-ejecución. Estado a **2026-07-15**.
> Rama: `codex/mainnet-hotfix-economics-private-tx`.

---

## 0. Contexto y estado actual

La red pivotó a **serverless** (los droplets/RPC `rpc.zoka.network:3000` están muertos desde 2026-07-09). Todo el estado de red debe venir del **nodo local vía p2p**, no del RPC.

### Ya hecho (commiteado)
- **1.7.3** (commit `9022220`, Codex): hashrate universal (RandomX fast-mode por RAM total), endpoint `/peers`, panel Peers, panel Seed (iroh EndpointId + onion), limpieza serverless de toda la UI.
- **1.7.4 / tail-sync iroh** (commit `8d6847e`): protocolo iroh `HED1` (head) + `BLK1` (rango de bloques), `ChainServe` sobre storage vivo, `maybe_iroh_head_sync` (follower jala bloques faltantes cada ~20s), gateado por `!mining_active`, test round-trip pasa. **MSI 1.7.4 construido, no instalado aún.**

### Diagnóstico refinado (con datos vivos del host, 2026-07-15)
El host está en **3127 caught_up**, con **1 peer conectado por Tor** (`/onion3/pwhjqt...`), gossip fluyendo (995 rx) pero **43 `gossip_publish_err`**. El bloque 3127 no llegó a la VM porque **la conexión entre máquinas se cae** (Tor sin keepalive/reconnect) — cuando se minó, la VM estaba desconectada, y sin link libp2p vivo nunca lo recibió.

→ El **tail-sync iroh (1.7.4)** es la red de seguridad correcta: recupera al follower aunque el link libp2p se caiga. Pero hay que atacar **también** la persistencia de la conexión y forzar convergencia de cadena.

### Datos clave capturados
- **Hash canónico bloque 3127:** `b5c20cfbdc9e2f56bb143334c03f7bcc9df60fbd7c2c7ed553b61ee2dd9740df`
  - prev (`3126`): `f827341c9d35640c1f9a2fed9bfeb8306478f4c97d998b7f1fc51afd382fae6d`
  - timestamp: `1784077666`
- Manifest iroh seed: `ca7721e29d60af28dda8406191f6d7a98e146b95cd456392d0c2d2b996639f99`
- RPC local del nodo: `127.0.0.1:3000`
- Wallet VM (de captura): `zsilent`, ~2077 ZKA disponibles.

---

## FASE 0 — Desplegar y validar el tail-sync 1.7.4

**Pre-requisito crítico:** el EndpointId iroh del **host** debe coincidir con el seed del manifest (`ca7721...`), porque la VM solo hace tail-sync desde seeds del manifest.
- [ ] Abrir panel **Node → Seed** en el host y comparar su iroh EndpointId con `ca7721...`.
  - Si coincide → OK.
  - Si difiere → actualizar `docs/mainnet-network.v1.json` (campo `iroh`) con el id real del host, **o** alinear la key `%APPDATA%/Zoka/iroh_key`.

**Instalación (host y VM):**
- [ ] Cerrar ZSilent (no reinstalar con la app abierta: rompe el nodo/DB).
- [ ] Uninstall 1.7.3 + install `zb/ZSilent Core-1.7.4.msi` (comandos PowerShell separados; ver nota MSI abajo).
- [ ] VM: descargar 1.7.4 desde `http://10.0.2.2:8090`.

**Validación (la prueba de fuego):**
- [ ] Con la VM **desconectada a propósito** del host, minar 1 bloque en el host (p.ej. 3128).
- [ ] Reconectar/observar: en ≤20-40s la VM debe saltar al nuevo height **por tail-sync iroh**, sin link libp2p.
- **Criterio de éxito:** la VM alcanza el height del host aunque la conexión libp2p esté caída. En los logs de la VM: `iroh tail-sync: pulled N block(s)...`.

---

## FASE 1 — Probar transferencias de balance (serverless)

Antes las transferencias se apoyaban en el RPC (muerto). Ahora dependen de gossip p2p + (respaldo) tail-sync iroh.

- [ ] Desde la wallet del host (con saldo) enviar X ZKA a una address de la VM.
- [ ] Verificar la cadena completa: tx → mempool → minada en bloque → bloque propaga → **balance aparece en la VM**.
- [ ] Repetir con la VM previamente desconectada, para confirmar que el balance llega vía tail-sync iroh al reconectar.
- **Sub-tarea (transferencias privadas):** mi `BLK1` envía `private_tx_bodies` **vacío** (correcto para consenso, pero el follower no puede recuperar la private tx para su wallet). Verificar si una transferencia **privada** se recupera en el follower; si no → **backfill de bodies por iroh** (extender `BLK1` para incluir bodies, o reusar el body-fetch existente).
- **Criterio de éxito:** balance correcto en ambos nodos tras transferencias públicas; decisión tomada sobre bodies privados.

---

## FASE 2 — Robustecer la persistencia de peers

**Investigación (primero):**
- [ ] Extraer el **error de desconexión** de los logs vivos (panel Logs / stdout del nodo). El usuario lo vio anoche.
- [ ] Revisar `gossip_publish_err` (43 y subiendo): con 1 solo peer el mesh de gossipsub queda en flood-publish, frágil.

**Causas probables:**
1. Circuitos Tor mueren sin keepalive → la conexión onion se corta.
2. Sin reconexión automática al peer/seed cuando se cae.
3. Gossipsub tuneado para redes grandes; con 1-2 peers el mesh no se forma.

**Acciones:**
- [ ] Keepalive/ping libp2p sobre la conexión onion.
- [ ] Auto-reconnect: redial del onion del seed/peer cuando se detecta desconexión (con backoff).
- [ ] Revisar timeouts de conexión Tor / circuitos.
- [ ] Tuning de gossipsub para redes chicas (mesh_n bajo, flood-publish explícito).
- **Criterio de éxito:** conexión host↔VM estable por horas sin caerse; `gossip_publish_err` deja de crecer.

---

## FASE 3 — Checkpoint canónico de reorganización (bloque 3127) ⚠️ consenso

**Objetivo:** fijar el 3127 (`b5c20cfb...`) como canónico. Cualquier nodo cuyo bloque en 3127 no sea ese hash tiene una cadena inválida: sus bloques locales se rechazan y debe re-sincronizar a la nuestra.

**Diseño propuesto:**
- [ ] Hardcodear un checkpoint `{ height: 3127, hash: "b5c20cfb...dd9740df" }` en el módulo de consenso (`economics`/`state::chain`).
- [ ] En `apply_block` / fork-choice: rechazar cualquier bloque en `height == 3127` con hash != checkpoint, y cualquier cadena que diverja antes/en el checkpoint.
- [ ] Un nodo que arranca con una cadena divergente en 3127 debe hacer rollback y re-sync (snapshot iroh o tail-sync) a la canónica.
- [ ] Extender el checkpoint a un mapa `{height: hash}` para poder agregar más anclas en el futuro.

**⚠️ Riesgos (leer antes de ejecutar):**
- Es **semi-irreversible**: una vez desplegado, los nodos divergentes quedan forzados a nuestra cadena.
- El hash debe ser el **verdadero canónico** (ya capturado del host: `b5c20cfb...`). **Verificarlo dos veces** antes de hardcodear.
- Es un **cambio de consenso**: todos los nodos deben actualizar; los que no, no aplican el checkpoint pero igual seguirán la cadena más pesada.
- **BACKUP obligatorio** de la DB canónica antes de desplegar (ver Fase 4).
- **Confirmar con el usuario antes de mergear** — no auto-ejecutar esta fase sin aprobación explícita.

---

## FASE 4 — Lo que podrías estar omitiendo (revisión crítica)

1. **Backup de la cadena canónica ANTES del checkpoint** (Fase 3 es casi irreversible). Exportar snapshot firmado + copia de `blockchain.db`.
2. **`gossip_publish_err: 43`** ya está degradando la propagación hoy → es parte de la Fase 2, no cosmético.
3. **Difficulty 23 y emisión**: el tuning pre-mainnet sigue abierto (block time/difficulty demasiado rápidos según notas previas). Revisar antes de mainnet real.
4. **El tail-sync iroh solo jala de seeds del manifest.** Para una red multi-minero real, los peers deberían **anunciar su iroh EndpointId** (peer-exchange) para que cualquiera pueda servir de fuente, no solo los seeds hardcodeados.
5. **Múltiples seeds iroh en el manifest** para no depender de una sola máquina (si el host se apaga, la red pierde su fuente de tail-sync).
6. **Metodología de testing repetible**: 2 wallets fijas, montos conocidos, verificación de balance en ambos nodos, script que automatice la prueba de propagación con desconexión.
7. **Bodies de private tx** no viajan por tail-sync (Fase 1 sub-tarea) → las transferencias privadas podrían no recuperarse en followers.
8. **Rotación de secretos pendiente** (tokens DO/Hostinger, password de wallet que se pegaron en chat) — higiene de seguridad.
9. **Versionado/freeze**: si la Fase 3 sube la versión del nodo, rompe el protocol-freeze libp2p con 1.7.3. Coordinar: o se mantiene 1.7.3 con checkpoint aditivo, o se hace un corte de red coordinado.

---

## Orden de ejecución recomendado

1. **FASE 0** (desplegar+validar 1.7.4) — desbloquea todo lo demás; bajo riesgo.
2. **FASE 2 investigación** (leer el error de desconexión) — en paralelo, informa la Fase 2.
3. **FASE 1** (transferencias) — valida el objetivo de negocio.
4. **FASE 2 acciones** (persistencia) — estabiliza la red.
5. **FASE 3** (checkpoint) — **solo con aprobación explícita + backup**.

> Regla: las fases 0-2 se pueden auto-ejecutar. La **Fase 3 (consenso) requiere confirmación del usuario** antes de mergear/desplegar.
