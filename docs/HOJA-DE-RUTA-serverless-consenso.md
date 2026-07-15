# Hoja de ruta — ZOKA serverless: propagación, persistencia, transferencias y checkpoint

> Documento vivo de auto-ejecución. **Actualizado: 2026-07-15 (v2).**
> Rama: `codex/mainnet-hotfix-economics-private-tx`.

---

## 0. Contexto y estado actual

La red es **serverless** desde el 2026-07-09 (los droplets/RPC `rpc.zoka.network:3000` están muertos). Todo el estado de red viene del **nodo local vía p2p**, no del RPC.

### Ya hecho y commiteado
- **1.7.3** (`9022220`): hashrate universal (RandomX fast-mode por RAM total), endpoint `/peers`, panel Peers, panel Seed (iroh id + onion), limpieza serverless de toda la UI.
- **1.7.4 / tail-sync iroh** (`8d6847e`): protocolo iroh `HED1` (head) + `BLK1` (rango de bloques), `ChainServe` sobre storage vivo, `maybe_iroh_head_sync` (follower jala bloques faltantes cada ~20s, gateado por `!mining_active`), test round-trip pasa.
- **Comms**: anuncio serverless publicado en Discord `#anuncios-es` + `#announcements-en` (**sin links de release** — la descarga se publica recién con versión estable). Artifact bilingüe EN/ES.

### Desplegado y validado hoy (2026-07-15)
- ✅ **1.7.4 instalado en host + VM** (host: desinstalado 1.7.3 → instalado 1.7.4; VM: bajado de `http://10.0.2.2:8090`).
- ✅ **iroh id del host = `ca7721...` = seed del manifest** → el host **es** seed1; el tail-sync está **armado** (la VM jalará del host).
- ✅ **Ambos en 3127, sincronizados**, conectados por onion (intermitente).
- ✅ **Paneles Peers + Seed funcionando** (confirmado en capturas de la VM/host).

### Diagnóstico de persistencia — CERRADO (causa raíz concreta)
- **Tor mata circuitos por inactividad** (`tor.log`: `Tor has been idle for 37872 seconds; assuming established circuits no longer work`) + guards inestables (`8 collapsed, 15 timed out`).
- **CAUSA RAÍZ: falta el protocolo `ping` de libp2p** en `NodeBehaviour` (`src/network/p2p.rs:713`). No hay keepalive → sin tráfico periódico garantizado, el circuito Tor se muere. `ping` **tampoco está en las features de libp2p** en `Cargo.toml:52`.
- El **reconnect ya es agresivo** (2s con 0 peers, `reconnect_delay_with_jitter`) → reconectar no es el problema; **mantener viva** la conexión sí.
- **Log de la VM (captura 8):** los WARN rojos NO son bugs nuevos → son endpoints muertos (`rpc.zoka.network:3000`, `seed2` clearnet, `seed1` clearnet inalcanzable por NAT) + la VM sondeando el bloque **3128 que no existe todavía**. Ruido, no fallas.

### Datos clave (persistidos)
| Dato | Valor |
|---|---|
| iroh seed / id del host | `ca7721e29d60af28dda8406191f6d7a98e146b95cd456392d0c2d2b996639f99` |
| host onion | `/onion3/d5pk3nhesk5wlvz4f5unbhcozxgtj7u6mhagauyprjrxndzpwbaoeiid:28080` |
| host p2p id | `12D3KooWQ9BXbCWQVXyUxR9wV4TgBq2ueeWaG1spopUhND7t6fPy` |
| VM p2p id | `12D3KooWMCdTYZEdVMktU51stZCiqYzNJ3kaqskFVtTxoUzmtyAD` |
| VM onion | `/onion3/pwhjqtjv3wjpwmz3kk6vuy5yiw7ienttuqljjrz4olax3yfrickk3lyd:28080` |
| **Bloque 3127 canónico (hash)** | `b5c20cfbdc9e2f56bb143334c03f7bcc9df60fbd7c2c7ed553b61ee2dd9740df` |
| prev (3126) | `f827341c9d35640c1f9a2fed9bfeb8306478f4c97d998b7f1fc51afd382fae6d` |
| Dead endpoints a limpiar del manifest | `rpc.zoka.network:3000`, `seed2`, `seed1` clearnet |
| RPC local del nodo | `127.0.0.1:3000` |
| Server de descarga p/VM | `http://10.0.2.2:8090` (Python HTTP sobre `C:\Users\mderramus\zb`) |

### Esquema de versiones
El **motor del nodo se queda en 1.7.3** (para no romper el protocol-freeze libp2p con la red existente); **ZSilent** avanza independiente (1.7.4 → 1.7.5…). Los cambios de esta hoja son aditivos/back-compat salvo la Fase 3 (consenso).

---

## FASE 0 — Desplegar y validar 1.7.4 — **casi completa**
- [x] Host + VM en 1.7.4.
- [x] iroh id host = ca7721 (prerequisito del tail-sync).
- [x] Ambos 3127, sincronizados, conectados por onion.
- [ ] **Prueba final del tail-sync** (bloqueada por la dificultad alta, ver Fase 2b): minar el 3128 en el host con la VM y verificar que la VM lo tome por iroh. Buscar en Logs de la VM: `iroh tail-sync: pulled N block(s) from seed ca7721...`.

---

## FASE 2a — Persistencia de la conexión (→ ZSilent 1.7.5) **[PRÓXIMA]**

**Objetivo:** que la conexión host↔VM **no se caiga** (hoy flapea por circuitos Tor idle).

1. **Agregar `ping::Behaviour` (keepalive estándar de libp2p) — el fix principal:**
   - `Cargo.toml:52`: agregar `"ping"` a las features de `libp2p`.
   - `src/network/p2p.rs`:
     - Campo `ping: ping::Behaviour` en `NodeBehaviour` (struct línea 713, derive `NetworkBehaviour`).
     - Init en el constructor (~1597): `ping::Behaviour::new(ping::Config::new())` (pinga ~15s, timeout 20s).
     - Manejar `NodeBehaviourEvent::Ping(..)` en el swarm loop (ignorar/log; el keepalive es automático).
   - **Efecto:** tráfico cada 15s → resetea el idle timeout de libp2p (300s) **y** mantiene caliente el circuito Tor → no más flapping.
2. **Limpiar endpoints muertos del manifest** (`docs/mainnet-network.v1.json`):
   - `public_rpc` → `[]` (remover `rpc.zoka.network:3000`).
   - Remover `seed2`.
   - `seed1`: dejar **onion + iroh**; el clearnet `/dns4/.../tcp/28080` es inalcanzable por NAT (evaluar dejarlo o nullear).
   - Efecto: para el churn/ruido de dials muertos que confunde en los logs.
3. *(Opcional)* Tuning de gossipsub para red chica (mesh_n bajo / flood-publish explícito) → baja `gossip_publish_err`.
- **Criterio de éxito:** conexión host↔VM estable por horas, sin flapping; `gossip_publish_err` no crece.

---

## FASE 2b — Dificultad (salud de red) **[IMPORTANTE — bloquea el test]**

Difficulty **23** quedó calibrada para cuando había más hashrate. Hoy con ~1 minero es desproporcionada: bloques de **horas** → bloquea la prueba de propagación y hace la red casi inusable.
- El **fast-mode de 1.7.4** ayuda (665 → ~5-8k H/s), pero sigue lento a diff 23.
- [ ] Confirmar que el fast mode arrancó en el host (leer hashrate al minar; light≈665, fast≈miles).
- **Opciones:**
  - (a) Dejar que el **retarget** baje sola la dificultad (lento; se acelera con fast mode).
  - (b) **Reset coordinado de dificultad** — cambio de consenso, con cuidado + backup + rollout coordinado.
- [ ] Decidir con el usuario: esperar retarget vs. reset.

---

## FASE 1 — Transferencias de balance (serverless)

Antes iban por RPC (muerto); ahora por gossip p2p + respaldo tail-sync iroh.
- [ ] Enviar X ZKA de la wallet del host a una address de la VM; verificar cadena completa (mempool → bloque → propaga → balance en la VM).
- [ ] Repetir con la VM desconectada para confirmar recuperación por tail-sync.
- **Sub-tarea:** `BLK1` manda `private_tx_bodies` **vacío** (ok para consenso, pero el follower no recupera la private tx para su wallet). Verificar transferencia **privada**; si no se recupera → backfill de bodies por iroh.
- **Criterio:** balance correcto en ambos nodos.

---

## FASE 3 — Checkpoint canónico 3127 ⚠️ **consenso (requiere aprobación + backup)**

**Objetivo:** fijar 3127 (`b5c20cfb...`) como canónico. Nodo cuyo bloque en 3127 ≠ ese hash → cadena inválida: sus bloques locales se rechazan y debe re-sincronizar a la nuestra.
- [ ] Hardcodear checkpoint `{3127: "b5c20cfb...dd9740df"}` en consenso (`economics`/`state::chain`), como mapa `{height:hash}` extensible.
- [ ] En `apply_block`/fork-choice: rechazar bloque en 3127 con hash != checkpoint, y cadenas que diverjan antes/en el checkpoint. Nodo divergente → rollback + re-sync.
- **⚠️ Riesgos:** semi-irreversible; verificar el hash dos veces; cambio de consenso (coordinar rollout); **BACKUP obligatorio** antes. **No mergear sin aprobación explícita.**

---

## FASE 4 — Omisiones / consideraciones (revisión crítica)

1. **Backup de la cadena canónica ANTES del checkpoint** (Fase 3 casi irreversible).
2. **Bodies de private tx** no viajan por tail-sync (Fase 1) → privadas podrían no recuperarse en followers.
3. **Tail-sync solo jala de seeds del manifest** → para multi-minero real, **peer-exchange de iroh ids** (que los peers anuncien su id) + **múltiples seeds iroh** en el manifest (si el host se apaga, se pierde la fuente).
4. **Versión/freeze**: si la Fase 3 sube la versión del nodo, rompe compat con 1.7.3 → coordinar (o mantener checkpoint aditivo sin bump).
5. **Comms**: anuncios **sin links de release** hasta versión estable de ZSilent; la descarga es evento aparte.
6. **Rotación de secretos** pendiente (tokens DO/Hostinger, password de wallet pegados en chat).
7. **Metodología de testing repetible** (2 wallets fijas, montos conocidos, script de prueba de propagación con desconexión).

---

## Orden de ejecución recomendado

1. **FASE 2a** — persistencia (`ping::Behaviour` + limpieza de manifest) → 1.7.5. **Próxima.**
2. **FASE 2b** — dificultad (confirmar fast mode; decidir retarget vs reset). Desbloquea el test.
3. **FASE 0** — validación final del tail-sync con el bloque 3128.
4. **FASE 1** — transferencias de balance.
5. **FASE 3** — checkpoint canónico (**solo con aprobación explícita + backup**).

> Fases 0-2 se auto-ejecutan. **Fase 3 (consenso) requiere confirmación del usuario** antes de mergear/desplegar.
