# Fix: replicación de cuerpos de tx privadas entre nodos

> Brief para implementar (pensado para Codex u otro agente que arranca **en frío**).
> Incluye el panorama completo, qué hacer, qué **no** hacer, y una sección final con
> **cómo lo implementaría yo (Claude)** a nivel código.

---

## 1. Contexto / panorama general

Zoka es una blockchain con transacciones privadas (Groth16, nullifiers). Cada bloque
committea en su `merkle_root`:

- `private_tx_hashes` — los hashes de las tx privadas del bloque.
- `private_nullifiers` — los nullifiers gastados. **Esto es consenso** (protege contra
  doble gasto y está anclado por PoW).

Aparte, el **cuerpo completo** de cada tx privada (outputs cifrados, proof, etc.) se
guarda **fuera del bloque**, en storage local bajo el prefijo `priv_tx:`, y se sirve
por `GET /private/tx/{hash}`. **El cuerpo NO es consenso** — solo sirve para que
wallets/clientes lean los outputs.

Red actual: 2 droplets — **BOOT** `159.223.119.216:3000` (seed/bootstrap, full) y
**VPS2** `157.230.5.103:3000` (RPC).

---

## 2. El bug a arreglar

**Síntoma:** `GET /private/tx/{hash}` devuelve **404 en VPS2** para tx que SÍ están
committeadas (BOOT las tiene, VPS2 no). Ejemplo real verificado: bloque 3977, tx
`e796d7727f19f6614dace610816cdd11` → **200 en BOOT, 404 en VPS2** (pero VPS2 sí tiene
los `private_nullifiers` del bloque 3977).

**Por qué importa:** una wallet que escanea VPS2 no ve el cuerpo del gasto → puede
**sobre-reportar el saldo disponible** (peligroso: dejaría intentar gastar fondos ya
gastados).

> La app móvil ZKAPriv **ya tiene un mitigador** (cae a otro RPC ante 404, y deriva
> los gastos de los `private_nullifiers` committeados). Este fix ataca la **causa raíz
> del lado del nodo**, para que la red sea consistente sin depender del workaround del
> cliente.

---

## 3. Causa raíz (precisa)

Los cuerpos **solo se propagan por el camino de mempool/pending**, no con el bloque
minado:

- `post_block_submit` (`/sync/block`) en `src/api/mod.rs:~4306` arma el
  `BlockResponseMessage` con **`private_tx_bodies: vec![]`** (`src/api/mod.rs:~4333`).
  → un nodo que recibe un bloque minado/sincronizado obtiene hashes + nullifiers pero
  **NO los cuerpos**.
- El cuerpo solo llega vía el path de pendientes:
  `publish_private_tx_body_to_active_rpcs` (`src/main.rs:~3502`) + el pull NAT-friendly
  `pull_pending_private_txs_from_rpc` (`src/main.rs:~2858`), que corre periódico en el
  loop del fullnode (`src/main.rs:~7288`, cada 5s).
- Si un nodo **nunca tuvo esa tx pendiente** (se minó antes de verla en su mempool),
  se queda **sin el cuerpo para siempre**.

---

## 4. Objetivo

Que un nodo, después de aplicar/recibir un bloque, **pulle de un peer los cuerpos
faltantes** de las `private_tx_hashes` de ese bloque (los que no tenga ya en
`priv_tx:`), los **valide** y los guarde, para servirlos en `GET /private/tx/{hash}`.

---

## 5. Enfoque

Espejar el patrón del pull de pendientes, que **ya existe y funciona**:

- Estudiar `pull_pending_private_txs_from_rpc` (`src/main.rs:2858`) y su invocación
  periódica en el loop (`src/main.rs:~7288-7295`).
- Crear una función análoga, p.ej. `pull_missing_committed_private_tx_bodies(...)`, que:
  1. Para los `private_tx_hashes` de bloques recientes, chequee cuáles **no** tiene en
     storage (`storage.get_private_transaction_bytes(hash)` = `None` ⇒ falta — mismo
     check que `have_committed`, `src/main.rs:2866-2871`).
  2. Para cada faltante, `GET /private/tx/{hash}` a los `rpc_sync_endpoints` (los mismos
     peers del pull de pendientes) con `rpc_http_get_json` (helper existente).
  3. **Valide** el cuerpo (ver §7) antes de guardarlo.
  4. Lo **guarde en el store de cuerpos committeados** (`priv_tx:`) usando la **misma
     ruta de escritura** que `post_private_tx_body` (handler de `POST /private/tx`,
     `src/api/mod.rs:~4415`).
  5. Idempotente + throttled.
- Engancharlo como **tarea periódica separada** en el loop del fullnode, al lado del
  pull de pendientes (`src/main.rs:~7288`), con su propio `next_*_at`.

---

## 6. ❌ Qué NO hacer (para no romper nada)

1. **NO tocar el camino de consenso.** Nada de modificar `apply_block_inner`
   (`src/state/chain.rs`), `validate_pow_block`, el merkle root, ni la validación de
   bloques/nullifiers. El fix es **puramente aditivo y best-effort**: si el pull falla,
   el nodo queda igual que hoy.
2. **NO abrir un segundo handle de RocksDB en modo escritura** sobre el mismo path que
   el nodo ya tiene abierto → RocksDB es **single-writer**, lo romperías (lock error).
   Para **escribir** los cuerpos, usá el **handle de storage que el nodo ya tiene en
   proceso** (el mismo de `post_private_tx_body`), NO un `RocksDbStorage::new(path)`
   nuevo para escribir. *(El pull de pendientes abre uno solo para LEER y degrada con
   `.ok()`; escribir es distinto.)*
3. **NO meter los cuerpos en el `Block` ni en `/sync/block`.** Dejá
   `private_tx_bodies: vec![]` como está. Cambiar el formato del bloque = hard fork.
4. **NO bloquear el loop principal** esperando los pulls. Best-effort, timeouts cortos
   (5s como el pull de pendientes), sin reintentos agresivos.
5. **NO confiar ciegamente en el cuerpo del peer** — validar (ver §7).
6. **NO cambiar APIs públicas, formatos de respuesta, ni el comportamiento de endpoints
   existentes.**

---

## 7. 🔐 Seguridad (obligatorio)

Antes de guardar un cuerpo bajado de un peer, **validar que corresponde al hash pedido**:
recomputar/leer el hash canónico del cuerpo (el `tx_hash` con el que el nodo indexa en
`priv_tx:`) y verificar que **coincide** con el hash solicitado. Si no coincide →
**descartar** (un peer malicioso no debe poder inyectar un cuerpo falso bajo un hash
válido). Reusar la validación que ya hace `post_private_tx_body` al recibir un cuerpo.

---

## 8. Pruebas (antes de deployar)

- `cargo build --release` (o `--features randomx` si aplica) — compila limpio.
- `cargo test` foco en `state::chain`, `storage`, private tx / sync — sin regresiones.
- Test unit nuevo (si es viable): guardar un cuerpo por la nueva ruta y leerlo con
  `get_private_transaction_bytes`; y un test que **rechace** un cuerpo cuyo hash no matchea.

---

## 9. Deploy (escalonado, reversible)

- Deploy vía GitHub Actions **`deploy-mainnet.yml`** (compila `zoka` en runner + SSH a
  los droplets). No compilar/deployar a mano salvo necesidad.
- **Escalonado:** primero a **un solo droplet** (VPS2, el afectado), verificar (§10), y
  recién después el otro.
- **Reversible:** puro agregado → revertir = quitar la función + su llamada. Sin
  migración de datos, sin cambio de formato.

---

## 10. Verificación (criterio de éxito)

1. En VPS2:
   `curl -s -o /dev/null -w "%{http_code}" http://157.230.5.103:3000/private/tx/e796d7727f19f6614dace610816cdd11`
   → debe pasar de **404 a 200** tras unos segundos del pull.
2. La cadena **sigue sincronizando**: `GET /chain/height` en BOOT y VPS2 avanzan y
   coinciden; sin errores nuevos en logs; sin loops de CPU.
3. Una tx privada **nueva** post-deploy: su cuerpo aparece en **ambos** nodos.
4. Nada existente roto: `/blocks/...`, `/private/pending`, minería, sync OK.

---

## 11. Archivos clave

| Archivo / ref | Rol |
|---|---|
| `src/api/mod.rs:~4306` `post_block_submit` | `/sync/block`; deja `private_tx_bodies: vec![]` (4333) — **no tocar formato** |
| `src/api/mod.rs:~4415` `post_private_tx_body` | `POST /private/tx` — **reusar su escritura + validación** del cuerpo |
| `src/api/mod.rs:~4419` `get_private_tx_by_hash` | `GET /private/tx/{hash}` — el lector |
| `src/main.rs:2858` `pull_pending_private_txs_from_rpc` | **patrón a espejar** |
| `src/main.rs:~7288-7295` loop del fullnode | **enganchar acá** la nueva tarea periódica |
| `storage.get_private_transaction_bytes(hash)` | lector de cuerpo committeado (su inverso = guardar) |

---

## 12. Cómo lo implementaría yo (Claude) — detalle de implementación

> Esto es la receta concreta que seguiría. Los nombres marcados con ⚠️ son a
> **confirmar leyendo el código** (no inventarlos): el método exacto de guardado y la
> función de hash canónico salen de `post_private_tx_body`.

### 12.1. La función (mirror del pull de pendientes, pero escribiendo a `priv_tx:`)

```rust
/// Best-effort, OFF the consensus path. Pulls committed private-tx bodies we are
/// missing from peers, validates them, and stores them so GET /private/tx/{hash}
/// can serve them. Returns how many bodies were fetched+stored this pass.
fn pull_missing_committed_private_tx_bodies(
    storage: &RocksDbStorage,        // the node's IN-PROCESS handle — NOT a new open
    rpc_endpoints: &[String],
    hashes: &[String],               // private_tx_hashes from recently-applied blocks
) -> usize {
    let mut fetched = 0usize;
    for hash in hashes {
        // 1) skip if we already have the body (same check have_committed uses)
        if storage.get_private_transaction_bytes(hash).ok().flatten().is_some() {
            continue;
        }
        // 2) try each peer until one serves it
        for rpc in rpc_endpoints {
            let json = match rpc_http_get_json(rpc, &format!("/private/tx/{hash}"), 5) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // 3) parse into the SAME type post_private_tx_body accepts ⚠️
            //    (the body lives under the field that get_private_tx_by_hash returns)
            let body: PrivateTransaction = match parse_private_tx_body(&json) { // ⚠️
                Some(b) => b,
                None => continue,
            };
            // 4) SECURITY: the body must hash to the requested hash ⚠️
            if canonical_private_tx_hash(&body) != *hash {  // ⚠️ reuse node's hashing
                continue; // reject mismatched/forged body
            }
            // 5) store via the SAME write path post_private_tx_body uses ⚠️
            if storage.save_private_transaction_body(&body).is_ok() {  // ⚠️ confirm name
                fetched += 1;
                break; // got it from one peer
            }
        }
    }
    fetched
}
```

### 12.2. El hook en el loop (mirror de `next_private_pull_at`)

En `src/main.rs`, junto al pull de pendientes (`~7288`):

```rust
// declarar cerca de next_private_pull_at:
let mut next_body_pull_at = tokio::time::Instant::now();
let mut last_body_scan_height: u64 = 0;  // para no re-escanear todo cada ciclo

// dentro del loop, al lado del bloque del pull de pendientes:
if tokio::time::Instant::now() >= next_body_pull_at {
    let tip = /* altura actual desde el storage del nodo */;
    // STEADY STATE: solo bloques nuevos desde la última pasada.
    // BACKFILL: en el primer ciclo (last_body_scan_height == 0) escanear hacia
    //   atrás una ventana (p.ej. últimos 5_000 bloques o todo si es chico) para
    //   tapar huecos históricos como el del bloque 3977.
    let from = if last_body_scan_height == 0 {
        tip.saturating_sub(BODY_BACKFILL_WINDOW)   // const nueva, p.ej. 5_000
    } else {
        last_body_scan_height + 1
    };
    let recent_hashes = collect_private_tx_hashes_in_range(&storage, from, tip); // ⚠️ helper
    let n = pull_missing_committed_private_tx_bodies(
        &storage, &rpc_sync_endpoints, &recent_hashes,
    );
    if n > 0 {
        // log a nivel INFO: "backfilled N private-tx bodies from peers"
    }
    last_body_scan_height = tip;
    next_body_pull_at = tokio::time::Instant::now() + Duration::from_secs(12);
}
```

### 12.3. Decisiones de diseño que tomaría

1. **Handle de storage, no path.** Le paso a la función el **handle in-process** del
   nodo (el mismo que usa para persistir cuerpos al procesar bloques), no un
   `RocksDbStorage::new(path)`. Esto es lo único que podría romper la DB si se hace mal
   (single-writer). Si en el punto del loop solo hay `path` y no handle, obtengo el
   handle del `node`/`state_manager` que ya está abierto.
2. **Backfill + steady-state.** El bug 3977 es **histórico**, así que un pull que solo
   mire bloques nuevos no lo arregla. Hago un **backfill** acotado en el primer ciclo
   (ventana hacia atrás) y después solo bloques nuevos (`last_body_scan_height`) para
   no rescanear toda la cadena cada 12s.
3. **Validación de hash sí o sí (§7).** Reuso exactamente la derivación/validación de
   hash de `post_private_tx_body` — no la reescribo, para que sea byte-idéntica.
4. **Idempotente + barato.** El check `get_private_transaction_bytes` evita re-bajar lo
   que ya tengo; corto al primer peer que lo sirve; timeout 5s; intervalo 12s.
5. **Cero consenso.** No toco `apply_block_inner` ni nada de validación. Si querés que
   el backfill arranque "al aplicar un bloque", igual lo dejo como tarea periódica
   separada (no dentro de `apply_block_inner`) para no acoplarlo al camino crítico.
6. **Logs claros** (cuántos cuerpos se bajaron, de qué rango) para verificar en prod.
7. **Test unit** del store+read y del rechazo por hash mismatch (§8).

### 12.4. Orden de trabajo que seguiría

1. Leer `post_private_tx_body` (`src/api/mod.rs:~4415`) → sacar (a) el tipo del cuerpo,
   (b) cómo lo guarda, (c) cómo valida/deriva el hash. Esos son los 3 ⚠️.
2. Leer cómo el loop obtiene el handle de storage + la altura tip + cómo leer
   `private_tx_hashes` de un rango (la API ya expone `/blocks/{h}/private_tx_hashes`,
   así que el método interno existe).
3. Escribir la función + el hook + las consts (`BODY_BACKFILL_WINDOW`, intervalo).
4. `cargo build` + `cargo test`.
5. Deploy a **VPS2 primero** (`deploy-mainnet.yml`), verificar §10 punto 1 y 2.
6. Deploy a BOOT. Verificar §10 puntos 3 y 4.

### 12.5. Riesgo residual

Mínimo, y acotado a "servir cuerpos" (lo único ya roto). El único punto sensible es el
**handle de DB** (usar el del nodo, no abrir uno nuevo para escribir) y la **validación
de hash**. Con esos dos cubiertos, el peor caso es "no baja un cuerpo" = igual que hoy.
No puede forkear ni corromper la cadena, porque los nullifiers (lo que sí es consenso)
ya están en el bloque y este fix no los toca.
