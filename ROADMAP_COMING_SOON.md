# ZSilent Core — Roadmap: Explorer, P2P Exchange, Mining Pool

Última actualización: 2026-07-26
Complementa a `ROADMAP.md` (Fases A–D, wallet/core). Cubre las tres secciones que aparecieron en
el sidebar de ZSilent en v1.9.1 como "Coming Soon" (`ui/screens/Screens.kt`) y que hoy no tienen
funcionalidad real detrás.

Orden de ejecución: **Etapa 0 (limpieza) → Explorer → P2P Exchange → Mining Pool**.

---

## LEEME PRIMERO — handoff

Este documento es el estado de verdad del trabajo. **Leelo entero antes de tocar código.**

### Dónde está todo

| Qué | Estado |
| --- | --- |
| **Etapa 0** (limpieza de UI) | ✅ Completa, mergeada y pusheada a `main` de `zsilent-core`. |
| **Etapa 1** (Explorer) | ✅ Completa, **verificada contra nodo real** (mainnet, altura 12943). |
| **Etapa 2** (P2P Exchange) | 🔶 ~97%. Backend en `etapa2-p2p-reservation-integration` y UI en `zsilent-core:etapa2-p2p-ui-integration`. 73 tests de `p2p_exchange::` y 28 tests de ZSilent en verde; implementación y gate completos, falta pase visual y prueba real. |
| **Etapa 3** (Mining Pool) | 🔶 ~99%. Implementación backend y UI completa. Backend en `etapa2-p2p-reservation-integration` (`d5486db9`, `cab6023d`, `d290328`, `344989b`, `63da315`); UI en `zsilent-core:etapa2-p2p-ui-integration` (`d78adbb`, `ac2f855`). 12 tests de dominio + tests dirigidos del worker/coinbase/fees y 32 tests de ZSilent en verde. Verificada además con dos fullnodes reales aislados en testnet/RandomX: join, shares, bloque canónico y failover. Faltan pase visual, dificultad normal/mainnet y release conjunto. |

### Por dónde seguir, en este orden

1. ~~**Enchufar `select_notes` a `create_exchange_offer`** (`src/api/mod.rs`) y marcar las notas
   reservadas en la wallet.~~ ✅ Conectado en `etapa2-p2p-reservation-integration`: la oferta
   rechaza fondos insuficientes, guarda los ids elegidos, marca las notas con el id de oferta y
   revierte la reserva si el store rechaza la inserción. Verificado con 4 tests del handler y
   los 66 tests de `p2p_exchange::`; **todavía no verificado contra nodo real**.
2. ~~**Cliente HTTP de Tron** que use `tron::trc20_transfers_path` **por el proxy SOCKS** y
   aplique `tron::transfer_settles_offer`.~~ ✅ Transporte Rust implementado con `socks5h`,
   TronGrid, API key opcional y fallbacks configurables. Se niega a construirse sin Tor.
   Verificado contra un servidor HTTP simulado detrás de un proxy SOCKS real de test; no se
   consultó Tron mainnet.
3. ~~**§2.6** presencia del vendedor y **§2.7** liberación automática + reputación.~~
   ✅ Integrado: heartbeat/TTL, claim decidido atómicamente por el vendedor, consulta Tron
   solo del vendedor, liberación desde las notas exactas y denuncia persistida por gossip.
   Verificado con tests unitarios/integración simulada; **no contra dos nodos ni Tron mainnet**.
4. ~~**§2.8** revisión de seguridad — gate obligatorio antes de habilitarlo a usuarios
   reales.~~ ✅ Backend y UI revisados: recuperación/rebroadcast durable, expiración segura,
   aceptación autoritativa del claim, binding de wallet, consentimiento persistido y denuncia
   con disclosure explícito. **Falta ejecutar el flujo real de monto mínimo.**
5. ~~Implementar el backend de **Etapa 3** (§3.1–§3.5).~~ ✅ Modelo, gossip dedicado,
   worker de shares, bloque real con coinbase rotativo, detección/expulsión y failover
   implementados. Verificado con 12 tests de `team_mining::`, tests dirigidos del worker,
   coinbase privada remota y fee claims, más `cargo check --bin zoka --no-default-features`.
   Verificación distribuida adicional: dos procesos reales, gossip/join, dos shares,
   bloque 1 con el mismo hash en ambos y failover a los 90,5 s. Usó RandomX de testnet con
   `--unsafe-rehearsal-difficulty-bits 1`; **no verifica dificultad normal ni mainnet**.
6. ~~Implementar §3.6 en ZSilent y ejecutar los tests de UI.~~ ✅ Cliente RPC, opt-in
   persistido y pantalla operativa completos; `gradlew test` pasó 32/32. **Falta el pase
   visual conjunto; la suite no verifica render ni interacción real con dos nodos.**
7. Release conjunto y pruebas reales pendientes: P2P Exchange con monto mínimo/Tron mainnet;
   para Team Mining quedan pase visual y dificultad normal/mainnet (la sesión local de dos nodos
   ya pasó). No mergear/reiniciar el nodo de uso ni mover fondos sin la aprobación correspondiente.
   No confundir esos pendientes con “Node too old” del binario viejo.

### Reglas de trabajo

- **Las decisiones de la sección "Decisiones cerradas" no se reabren.** Son 32, varias
  involucran fondos reales y costaron pensarlas. Si algo parece mal, decilo — no lo cambies
  por tu cuenta.
- **Andá tachando y anotando acá a medida que avanzás.** Marcá `- [x]` lo que completes,
  agregá qué encontraste, y **si tomás una decisión nueva, escribila en la tabla**. Este
  archivo es lo único que sobrevive entre sesiones: lo que no esté acá, se pierde.
- **Si algo no se verificó, decilo.** "Compila" y "funciona" no son lo mismo. En este
  documento cada ✅ significa que se probó, no que se escribió.
- **Rama nueva + commit por subetapa.** No mergear a `main` sin aprobación del usuario.

### Gotchas que te van a hacer perder tiempo

- El binario en `target/release` es del **23/7** y **no** tiene los endpoints
  `/p2p-exchange/*`. Por eso la pantalla de P2P dice "Node too old". Hay que recompilar
  (`cargo build --release`) y reiniciar el nodo. Confirmado nuevamente el 2026-07-26:
  `/p2p-exchange/status` y `/team-mining/status` devolvieron 404 mientras
  `/blocks/latest` respondió 200 en altura 12952; es el binario viejo esperado, no un bug
  de las pantallas nuevas.
- **El nodo del usuario corre en `127.0.0.1:3000`.** Los endpoints del explorer son de solo
  lectura: **no hace falta contraseña** para verificar contra él.
- `cargo test --lib` completo (860+ tests) **excede 10 minutos**. Filtrá:
  `cargo test --lib p2p_exchange::`.
- Los guardarraíles `deny` (`rm -rf`, `git reset --hard`, `git push --force`) viven en
  `.claude/settings.json` y **solo aplican a Claude Code**. Otras herramientas no los respetan.

### Deudas conocidas — no son olvidos

- La firma de gossipsub autentica el **nodo**, no la **wallet**: alguien puede publicar una
  oferta con la `zka1` de otro. Aceptado para v1; `Offer::signing_preimage()` ya está listo
  para atarlo cuando se cierre.
- **P2P Exchange todavía no se probó contra dos nodos reales.** Team Mining sí tuvo una sesión
  distribuida local con dos procesos/fullnodes, wallets/DB/puertos aislados y RandomX de testnet
  a dificultad de ensayo 1: convergieron membresía/backup, se aceptaron shares, ambos aplicaron
  el mismo bloque y el backup asumió tras la caída real del coordinador. Esto **no** reemplaza
  dificultad normal/mainnet, una corrida prolongada ni la validación visual desde ZSilent.
- En la observación inmediata del bloque, el coordinador ya mostraba `confirmed_blocks=1` y el
  miembro todavía `0`; el bloque canónico sí estaba en ambos. La sesión terminó al comprobar el
  mismo hash, así que la convergencia posterior del `TurnUpdate` no quedó medida.
- El lock de ZKA es contabilidad de wallet, **no un escrow on-chain**. Un cliente modificado
  puede gastar una nota "reservada". Es inherente al diseño v1 y está avisado en la UI.
- ✅ **Bloqueante de release resuelto en §2.8:** la wallet cifrada funciona como journal de la
  tx ya construida; tras un crash se recupera y rebroadcastéa el mismo hash, sin reconstruir
  otro spend. Permanece no verificado mediante restart de un nodo real.

---

## Decisiones cerradas (no reabrir)

### Transversales

| Tema | Decisión |
| --- | --- |
| Código muerto | Borrar el stack de UI paralelo **antes** de empezar (Etapa 0). |
| Tema | Un solo tema oficial: el verde pastel actual. Borrar `DarkScheme` lila y el parámetro `darkTheme` muerto. |
| Idioma | Toda la UI nueva en inglés, igual que el resto de la app. |
| Versionado | Release conjunto nodo+ZSilent al cerrar cada etapa, probados juntos. |
| Rollout | Cada sección sale detrás de un flag opt-in en Settings, marcada como experimental. |

### Explorer

| Tema | Decisión |
| --- | --- |
| Conectividad | 100% local — solo RPC del nodo propio. Nunca `zokascan.cc` ni terceros. |
| Base de código | Reutilizar la pantalla real existente y re-skinearla. No reescribir. |
| Alcance | Completo: bloques + transacciones + búsqueda unificada. |
| Nodo apagado / sincronizando | Estado explícito y honesto, nunca datos silenciosamente desactualizados. |

### P2P Exchange

| Tema | Decisión |
| --- | --- |
| Par | Solo ZKA → USDT, oferta unidireccional. Sin árbitro humano. |
| Red USDT | Tron (TRC-20). Proveedor TronGrid con fallback configurado. |
| Descubrimiento | Gossip P2P nativo, extendiendo `ZokaMessage`. No polling RPC. |
| Privacidad | La consulta a la API de Tron **siempre** sale por Tor. |
| Quién verifica | Solo el vendedor (es quien tiene la clave para liberar). |
| Vendedor offline | La oferta solo es visible/tomable con el nodo del vendedor online. |
| Liberación | Automática al detectar pago verificado. Sin botón manual. |
| Incumplimiento | Se registra y se comparte por gossip, con prueba de pago adjunta. |
| Monto incorrecto | De menos no libera; de más libera igual. |
| Persistencia | Ofertas y reservas persistidas en disco. |
| Anti-spam | Monto mínimo por oferta + tope de ofertas abiertas por address. |
| Comisión | Ninguna. El 100% del USDT va al vendedor. |
| Tope de monto | Sin tope en v1 (riesgo asumido, §2.8). |
| Expiración | 45 min desde que se toma la oferta (configurable). |
| Cancelación | El vendedor puede cancelar una oferta abierta aún no tomada. |
| Testing | Mainnet con montos mínimos. Sin fase de testnet. |
| Consentimiento | Pantalla de riesgos con aceptación explícita, una sola vez. |

### Mining Pool

| Tema | Decisión |
| --- | --- |
| Modelo | "Team mining" opt-in, equipos auto-organizados. Sin pool oficial de Zoka. |
| Custodia | **Ninguna.** No hay wallet de equipo: el coinbase rota entre las addresses de los miembros. |
| Reparto | Por turnos según shares aportadas — el coinbase mismo es el pago. |
| Consenso | Sin cambios. El coinbase sigue teniendo un receptor único. |
| Bloque hijackeado | No se puede impedir a nivel protocolo. Se detecta y se expulsa al miembro. |
| Coordinador caído | Coordinador de respaldo toma el rol automáticamente. |
| Dificultad de shares | Automática, ajustada al hashrate de cada minero. |
| Comisión | Ninguna. |
| Tamaño de equipo | Sin límite duro en v1. |

### Decisiones nuevas 2026-07-26 (implementación de Team Mining)

| Tema | Decisión |
| --- | --- |
| Transporte | Tópico gossipsub separado `zoka/team-mining/v1` con `TeamAdvertise`, `TeamJoin`, `ShareSubmit` y `TurnUpdate`. No se reutiliza `/mining/*` ni el tópico general. La firma sigue siendo la identidad del nodo; el binding wallet↔PeerId se aprende al unirse. |
| Identidad de equipo | `team_id` aleatorio de 128 bits. Invitación autocontenida `ZTM1-<team_id>-<checksum>`; no agrega infraestructura de identidad. |
| Respaldo | El primer miembro aceptado queda como respaldo. Tras 90 s sin coordinador se promociona, incrementa `epoch`, deja inactivo al coordinador anterior y elige el siguiente respaldo determinísticamente. |
| Estado autoritativo | El coordinador transfiere el `TeamState` completo (contadores, template y replay journal) al respaldo. Cada mutación lleva `revision` monotónica; un update firmado pero atrasado no puede hacer rollback de shares. |
| Cadencia de shares | Objetivo de una share cada 15 s por miembro, ajuste cada 8 shares e inicio en `network_difficulty - 8` (mínimo 1). |
| Reparto de fees | El miembro asignado recibe tanto el coinbase como los fee claims privados del bloque. El scanner de wallet recupera ambos; el coordinador no cobra una comisión implícita. |
| Exclusión con minería solo | Activar Team Mining suspende el worker de minería solitaria del mismo fullnode para no duplicar CPU ni competir con el template del equipo. |
| Leave | Un leave aceptado borra el estado local de equipo y apaga el worker. El coordinador no puede irse sin handoff; debe entrar el failover. |
| Consentimiento de Team Mining | El opt-in de la sección se persiste por separado. La pantalla mantiene visibles los riesgos y exige un acknowledgement local antes de crear o unirse a un equipo; no se envía ni se comparte por gossip. |

---

## Por qué este orden

- **Etapa 0** primero: hay dos stacks de UI paralelos en el repo. Limpiar antes evita editar el
  archivo equivocado durante las tres etapas siguientes.
- **Explorer**: la mayoría del backend del nodo ya existe y la pantalla ya está maquetada. Menor
  esfuerzo y riesgo — sirve para asentar el patrón de trabajo y el ciclo de release conjunto.
- **P2P Exchange**: le da liquidez real a la red. Es la de mayor riesgo porque hay fondos en
  juego, por eso lleva un gate de seguridad obligatorio (§2.8) antes de salir de "Coming Soon".
- **Mining Pool**: no depende de las otras dos, no requiere cambio de consenso, y tras la decisión
  de coinbase rotativo tampoco tiene custodia — puede esperar sin bloquear nada.

Cada subsección numerada está escrita para poder entregarse sola a una sesión de desarrollo.

---

## Etapa 0 — Limpieza previa ✅ COMPLETADA

Rama `etapa0-limpieza-ui` (3 commits). `compileKotlin` y `test` en verde. **Sin mergear a
`main`** — pendiente de tu revisión.

### 0.1 Borrar el stack muerto ✅

31 archivos eliminados vía `git rm` (reversible). El cluster muerto resultó ser más grande
de lo estimado: no solo `design/` + `app/` + `ui/screens/dashboard/`, sino también
`ui/screens/onboarding/`, `ui/screens/welcome/` y los 8 componentes `ui/components/Z*.kt`.

Verificado cerrado antes de borrar: `app/ZSilentApp.kt` era el **único** punto de entrada al
cluster y nadie lo importaba; los componentes `Z*` solo se referencian entre sí; ningún test
lo toca; el stack vivo tiene su propio QR y su propio sidebar (inline en `ui/ZSilentApp.kt`,
que es lo que parcheó `b697e0f`).

- [x] `design/` completo — sistema de diseño paralelo sin usar.
- [x] `app/` completo (`ZSilentApp.kt`, `AppState.kt`, `Navigation.kt`).
- [x] `ui/screens/dashboard/`, `ui/screens/onboarding/`, `ui/screens/welcome/`.
- [x] `ui/components/Z{Button,Card,QrCard,SeedWordChip,Sidebar,TextField,TopBar,WarningBox}.kt`.
- [x] Compila y los tests pasan.

### 0.2 Consolidar el tema ✅

- [x] Eliminado el parámetro `darkTheme` de `ZSilentTheme` (lo ignoraba).
- [x] Borrado `DarkScheme` (lila `#C084FC`) y sus ~30 constantes huérfanas — 5 ya no las
      usaba ni el propio `DarkScheme`.
- [x] `LightScheme` → **`ZenithScheme`** (el nombre viejo describía como "Light" una paleta
      casi negra).
- [x] Corregido el call site en `Main.kt`.
- [x] Eliminado `toggleTheme()`, que ninguna UI llamaba.
- [x] **No** se tocó el campo persistido `darkTheme` de `PersistedSetup`/`AppModels`: se
      escribe en los archivos de setup de los usuarios, así que sacarlo es un cambio de
      serialización, no una limpieza. Queda como campo vestigial inofensivo.

### 0.3 Resolver la duplicación de `ExplorerScreen` ✅

- [x] Eliminado el placeholder sin argumentos de `Screens.kt`.
- [x] Router → `ExplorerScreen(uiState, appState)`.
- [x] Quitado el import huérfano `Icons.Outlined.Explore`.
- [ ] **Pendiente:** smoke test visual con la app corriendo (compila, pero no se verificó en
      pantalla). El Explorer ya muestra datos reales, aunque su botón "Open" sigue sin hacer
      nada hasta la Etapa 1.

---

## Etapa 1 — Explorer ✅ COMPLETADA Y VERIFICADA

Mergeada y pusheada a `main`. Tests en verde.

**Verificado contra nodo real (2026-07-26, mainnet altura 12943):** los modelos coinciden
campo por campo. Genesis efectivamente sirve `difficulty_bits: null` — el tipo nullable era
necesario, no defensivo, porque `0` es lo primero que cualquiera busca. Altura fuera de rango
da 404 → "no encontrado"; `/p2p-exchange/offers` da 404 en el binario viejo → "Node too old"
en vez de afirmar que no hay ofertas.

Esa corrida encontró dos errores de fixture que **ningún test unitario podía detectar**
(ambos decodifican igual): `reward_zka_display` es `"12"` pelado, no `"12.00000000 ZKA"`, y
la recompensa son 12_000_000 atoms, no 1_200_000_000. Ya corregidos.

### 1.1 Extender `ZokaRpcClient` ✅

- [x] `blockByHeight(height)` → `GET /blocks/:height`
- [x] `blockByHash(hash)` → `GET /blocks/hash/:hash`
- [x] `blockTransactions(height)` → `GET /blocks/:height/transactions`
- [x] `mempoolTransaction(hash)` → `GET /mempool/tx/:hash`
- [x] `latestBlock()` → `GET /blocks/latest`
- [x] Modelos `BlockSummary`, `BlockTransactions`, `MempoolTransaction`.
- [x] Helper `getOrNull` — 404 devuelve null, fallo de transporte lanza excepción.
- [ ] ~~`transactionByHash(hash)` → `GET /transactions/:hash`~~ **descartado:** ese endpoint
      solo mira el mempool, así que duplicaría `mempoolTransaction`. Ver 1.3.

### 1.2 Conectar el lookup de bloque ✅

- [x] "Open" consulta el nodo de verdad (antes era `onClick = {}`).
- [x] Detalle: hash, padre, state root, timestamp, nonce, dificultad, tamaño, recompensa,
      conteos de tx públicas/privadas/nullifiers.
- [x] Estado de carga durante la consulta.

### 1.3 Detalle de transacción ⚠️ PARCIAL — límite del nodo

**El nodo no tiene índice `tx_hash → bloque`.** Verificado en `src/storage/` y `src/state/`:
no existe. `GET /transactions/:hash` solo consulta el mempool y devuelve 404 con "Check
confirmed blocks" si la tx ya está minada.

Consecuencia: **una transacción confirmada no se puede buscar por hash**, ni desde el Explorer
ni desde ningún cliente. Solo se la ve si ya sabés en qué bloque está.

- [x] Detalle de tx pendiente (hash, timestamp, estado).
- [x] Lista de hashes de tx dentro de un bloque.
- [x] La UI explica el límite en vez de decir "no encontrado" a secas, que sugeriría que la
      transacción nunca existió.
- [ ] **Diferido:** construir el índice en el nodo. Es trabajo de storage en Rust con backfill
      de la cadena existente y crecimiento por bloque — merece su propia etapa, no un apéndice
      apurado de esta.

### 1.4 Búsqueda unificada ✅

- [x] Un input: dígitos → altura; resto → hash de bloque, luego tx pendiente.
- [x] Estado vacío que distingue "no existe" de "todavía no sincronizado".

### 1.5 Estados de nodo apagado / sincronizando ✅

- [x] Nodo apagado: aviso explícito y buscador deshabilitado.
- [x] Sincronizando: avisa que los bloques sobre el tip local no aparecerán todavía.
- [x] Nunca se muestran datos desactualizados sin aclararlo.

### 1.6 QA y cierre de etapa

- [x] Flag opt-in en Settings → Experimental, apagado por defecto. La sección se filtra del
      sidebar (no se muestra deshabilitada) y el router cae al Dashboard si se apaga estando
      dentro.
- [x] **Riesgo cubierto:** `PersistedSetup` se construye desde cero en 7 sitios en vez de
      copiarse, así que un campo que algún sitio olvide pasar vuelve a su default en silencio.
      Los 7 llevan `explorerEnabled` y `SetupStoreExplorerFlagTest` fija el round-trip y el
      camino de upgrade (archivo viejo sin el campo → flag apagado).
- [x] `ExplorerModelDecodingTest` fija los modelos contra los payloads de los serializadores
      del nodo, incluido genesis con `difficulty_bits: null`.
- [x] Verificado por lectura de código: el Explorer solo usa `appState.localRpc`. Ninguna
      llamada sale del nodo propio.
- [ ] **Pendiente — verificación contra nodo real.** Todo lo anterior es build + tests
      unitarios. Nunca se ejecutó contra un nodo vivo: falta probar genesis (altura 0), último
      bloque, tx en mempool, hash inexistente, altura fuera de rango, nodo apagado y nodo a
      media sincronización. Requiere levantar `zoka fullnode`.
- [ ] **Pendiente — smoke test visual.** No se levantó la app; no está verificado en pantalla.
- [ ] Release conjunto nodo+ZSilent (requiere aprobación explícita).

---

## Etapa 2 — P2P Exchange (ZKA → USDT)

### 2.0 Verificar tolerancia del protocolo P2P ✅ RESUELTO

**Veredicto: es seguro agregar la variante.** Medido, no asumido — rama `etapa2-p2p-exchange`
en el repo ZOKA, commit `796c086b`, 3 tests en verde.

- [x] `ZokaMessage` se serializa en **JSON** (`serde_json`), así que una variante desconocida
      es un error de deserialización común y corriente, no un panic.
- [x] Los **6** sitios de `from_bytes` en `p2p.rs` tratan el `Err` igual: `warn!` y seguir. El
      mensaje se descarta, **el peer se conserva**.
- [x] Nada marca al emisor como incompatible: eso solo ocurre vía
      `mark_peer_protocol_version`, que lee el string de protocolo de *identify* y nunca mira
      el payload.
- [x] **No hay peer scoring** configurado en gossipsub (`with_peer_score` ausente), así que
      tampoco hay penalización de reputación.

**Dos hallazgos que el plan no anticipaba:**

1. **Existe un protocol freeze.** `PROTOCOL_FREEZE_ID = "/zoka/1.9.0-mainnet-recovery"`
   (`p2p.rs:74`), y `ZOKA_PROTOCOL_FREEZE_ENFORCE` **por defecto es true**: los binarios
   publicados desconectan peers con un id de protocolo distinto. Agregar la variante **no**
   requiere tocar ese id — y no hay que tocarlo, porque bumpearlo particiona la red. Si alguna
   vez hace falta, existe el mecanismo de grace window (`=0`) usado en rollouts previos.

2. **Tópico separado, decidido por otra razón.** Como los nodos viejos descartan la variante
   igual, un tópico aparte no cambia quién ve las ofertas (solo nodos nuevos, en ambos casos).
   Pero por el tópico `general` **cada oferta emitiría un `warn!` en todos los nodos viejos de
   la red** — spam de logs proporcional al volumen de ofertas. Con un tópico propio los nodos
   viejos ni se suscriben y no ven nada. → **Usar tópico separado.**

> **Estado al 2026-07-26 (fin de sesión).** Rama `etapa2-p2p-exchange` (repo ZOKA), pusheada.
> **73 tests de `p2p_exchange::` en verde**, más `network::message` (20) y `network::p2p` (39)
> sin regresión. UI y flag del Explorer están en `main` de zsilent-core, también pusheado.
>
> **Hecho:** 2.0 (compat P2P), 2.1 (modelo `Offer`), 2.2 (gossip + tópico propio +
> `signing_preimage`), 2.3 (`select_notes` + adaptador de wallet), 2.4 (reglas de pago Tron),
> 2.5 (store persistente + reconciliación), endpoints RPC (`/p2p-exchange/*`), 2.8 (pantalla
> de riesgos) y 2.9 (UI de las 3 secciones).
>
> **Pendiente — sobre todo integración:**
>
> 1. ✅ `select_notes` ya está conectado a `create_exchange_offer`; reserva las notas en la
>    wallet y llena `reserved_note_ids`. Verificado por tests del handler, no contra nodo real.
> 2. ✅ `TronClient` consume `trc20_transfers_path` exclusivamente por SOCKS (`socks5h`) y
>    `settling_transfer` aplica las reglas existentes y ya está conectado al loop automático.
> 3. ✅ §2.6 presencia del vendedor y §2.7 liberación + reputación integradas. La durabilidad
>    de release señalada en §2.8 quedó resuelta en tests; falta verificar con nodos reales.
> 4. Pase visual sobre las 3 secciones (acordado para el cierre de la etapa).
>
> **Deudas anotadas, no olvidadas:**
>
> - La firma de gossipsub autentica el **nodo**, no la **wallet**: alguien puede publicar una
>   oferta con la `zka1` de otro. Aceptado para v1, `signing_preimage()` listo para atarlo.
> - **Nada se verificó nunca contra un nodo real.** Todo es compilación + tests unitarios.
>   Desbloquea: crear la wallet del nodo (`zoka node --password <la tuya>`).
> - El binario del nodo en `target/release` es del 23/7 y **no** tiene los endpoints nuevos;
>   por eso la UI de P2P dice "Node too old" hasta recompilar.
>
> Nota: `cargo test --lib` completo (860+ tests) excede 10 min; usar filtros
> (`cargo test --lib p2p_exchange::`).

### Decisiones cerradas 2026-07-26 (segunda ronda)

| Tema | Decisión |
| --- | --- |
| Clave de firma | **Confiar en la firma de gossipsub** para v1. libp2p ya firma cada mensaje con la identidad del nodo (`MessageAuthenticity::Signed`). **Límite aceptado:** autentica el *nodo*, no la *wallet* — impide que un peer falsifique mensajes ajenos en tránsito, pero **no** impide que alguien publique una oferta con la `zka1` de otro. El campo `signature` y `signing_preimage()` quedan listos para cuando se ate a la wallet. |
| Fondos insuficientes | **Rechazar la oferta.** Si no hay notas suficientes para reservar, la creación falla. Ninguna oferta llega a la red sin respaldo. |
| Id de oferta | **128 bits aleatorios** (`new_offer_id()`). El id entra en el preimagen de firma, así que una colisión sería un vector de falsificación, no una molestia. |
| Endpoint Tron | **TronGrid público por defecto + override en Settings** (URL y API key opcional). Sin fricción inicial, con salida si rate-limitea. |
| Presencia de oferta | **Heartbeat firmado cada 30 s + TTL receptor de 90 s.** Al caer el vendedor deja de reanunciar y la oferta desaparece al vencer el TTL. No se emite un estado `offline`: el protocolo no tiene ese estado y un peer ya desconectado no puede garantizar un último update. Esto controla visibilidad, no muta el lifecycle persistido. |

### Decisiones nuevas 2026-07-26 (gate de seguridad)

| Tema | Decisión |
| --- | --- |
| Aceptación de claim | El POST del comprador solo devuelve `claim_requested`. **No debe pagar** hasta recibir el `Claimed` autoritativo firmado por el peer vendedor; evita que dos compradores interpreten su request simultáneo como aceptado. |
| Expiración segura | Cumplidos los 45 min, la reserva solo se libera después de una consulta Tron exitosa que no encuentre pago. Si Tor/Tron está caído, queda bloqueada. `PaymentDetected` nunca expira: reintenta release hasta confirmar. Un pago calificante visto antes de ese cheque final se honra aunque haya cruzado el deadline. |
| Journal de release | La wallet cifrada y su `private_transactions` son el journal durable. Las notas apuntan al hash de la tx construida; al reiniciar se rebroadcastéa **esa misma tx**, nunca se crea otra con los mismos inputs. |
| Entrega eventual de estados | `Claimed`/`PaymentDetected` se reanuncian cada 30 s mientras estén activos; `Released`/`Expired` se repiten durante 24 h. Así un crash entre persistencia y gossip no pierde el estado autoritativo. |
| Denuncia | Debe corresponder a offer/seller/buyer conocidos, llevar hash Tron válido, llegar firmada por gossipsub y esperar 5 min después del deadline. La gracia evita acusaciones permanentes por dos ciclos de polling/gossip que se cruzaron. |

### 2.1 Modelo de datos `Offer` ✅

Implementado en `src/p2p_exchange/mod.rs`. Decisiones que conviene no revisitar:

- **Enteros, nunca floats.** ZKA en atoms, USDT en micro-USDT (6 decimales, TRC-20). Un float
  redondearía precios en silencio y —peor— haría que dos nodos discrepen sobre si un pago
  coincide, que es el único juicio del flujo sin humano que lo apele.
- La tolerancia del 0.5% compara con multiplicación cruzada en vez de dividir primero, para que
  no se redondee a cero en ofertas chicas.
- `validate()` es lo que corre un nodo receptor antes de retransmitir, así que rechaza ofertas
  auto-inconsistentes (una `Open` que ya nombra comprador, una `Claimed` sin deadline).
- La dirección TRON se valida por forma (34 chars, empieza en `T`, base58). Detecta direcciones
  ETH pegadas por error, no una bien formada pero equivocada — en ese caso el pago nunca llega
  y la oferta expira.

### 2.1-bis Modelo original (referencia)

- [x] Campos implementados: `id`, `seller_address`, `zka_atoms`, `micro_usdt`,
      `usdt_receive_address` (TRC-20), `status` (`Open`/`Claimed`/`PaymentDetected`/
      `Released`/`Expired`/`Cancelled`), `created_at`, `claim_expires_at`, `claimed_by` y
      `reserved_note_ids`. La firma aceptada para v1 vive en el envelope de gossip, no en
      `Offer`.
- [x] Sin campos de comisión: `usdt_amount` es íntegro para el vendedor.
- [x] Serializable para viajar por el canal P2P — definirlo pensando en eso desde el inicio.
- [x] Persistido en disco (§2.5), no solo en memoria.

### 2.2 Gossip de ofertas ✅

Tópico propio `zoka/offers/v1` (`OFFERS_TOPIC_NAME`), variantes `OfferAnnounce` y
`OfferStatusUpdate` en `ZokaMessage`. Los updates llevan **solo id + estado nuevo**, nunca el
cuerpo de la oferta: reenviarlo dejaría que un peer altere montos o dirección de cobro
disfrazado de cambio de estado. `validate_inbound_message` rechaza: announce con oferta ya
tomada, claim sin comprador o sin deadline, y cualquier update que reabra una oferta (un
"reopen" resucitaría una oferta cuyos ZKA ya se gastaron).

La autenticación vigente es la firma de gossipsub con identidad de nodo, según la decisión
cerrada para v1. El binding criptográfico nodo↔wallet sigue como deuda conocida; TTL y
rebroadcast ya están implementados.

### 2.3/2.5 Store persistente ✅

`src/p2p_exchange/store.rs`. Claim es compare-and-set (el segundo comprador recibe
`NotClaimable`). Escritura por archivo temporal + rename; archivo corrupto es error, no reset
silencioso. `reconcile()` **reporta notas reservadas sin oferta viva detrás**, pero ya no vence
claims a ciegas durante el arranque: el runtime primero hace el chequeo Tron final. Las
transiciones mutadas en memoria hacen rollback si falla la persistencia.

### 2.2-bis Gossip (checklist original)

- [x] Mensajes `OfferAnnounce` (publicar) y `OfferStatusUpdate` (tomada/expirada/cancelada).
- [ ] Firmar los anuncios con la clave del vendedor: sin esto cualquiera publica ofertas ajenas
      o cancela las de otros.
- [x] Anti-spam local: el nodo reserva las notas antes de encolar el anuncio y no publica si
      la reserva/persistencia falla. Un receptor no puede probar criptográficamente el lock
      de otra wallet; queda cubierto solo por piso+tope y la identidad de nodo aceptada para v1.
- [x] Monto mínimo por oferta + tope de ofertas abiertas simultáneas por address.
- [x] TTL / rebroadcast: heartbeat cada 30 s, presencia remota por 90 s y expiración del claim
      a los 45 min.
- [x] Solo mostrar como tomables ofertas de vendedores online (§2.6).

### 2.3 Lock de ZKA sobre notas privadas

- [x] Reutilizar `reserved` / `reserved_by_tx` de `PrivateNote`
      (`src/private_wallet/mod.rs`, Fase A1 de `ROADMAP.md`). No construir un mecanismo nuevo.
      `create_exchange_offer` selecciona notas de la wallet local, las reserva atómicamente con
      el id de oferta y hace rollback si falla `OfferStore::insert_local`.
- [x] Rechazar la creación si la address no corresponde a una wallet local o si las notas
      disponibles no cubren `zka_atoms`.
- [x] Enchufar wallet manager + offer store al arranque normal del fullnode. Se agregó un
      `ExchangeRuntime` único compartido por los endpoints, con store en
      `<network-dir>/p2p_exchange/offers.json`.
- [x] Persistir cifrado el cambio `reserved` de la wallet **antes** de publicar la oferta, y
      persistir también el rollback si el store la rechaza. Antes de seleccionar, el handler
      refresca la wallet desde disco para no pisar cambios concurrentes del proceso de minería.
- [x] Reconciliar al arranque: liberar reservas huérfanas y persistir la wallet reparada.
      Un claim vencido se conserva hasta que Tron responda y confirme que no hubo pago.
- [x] La UI actual omite `seller_address`; el nodo completa la address de su wallet seleccionada,
      tal como anticipaba `CreateOfferRequest`, en vez de rechazar el request real de ZSilent.
- [x] Verificado: recarga desde disco conserva `reserved`/id de oferta; un fallo de persistencia
      del store no deja una oferta/transición fantasma en memoria; 73 tests `p2p_exchange::` en verde y
      `cargo check --bin zoka` en verde. **Todavía no verificado contra el nodo real.**
- [x] Al expirar/cancelar: liberar las notas reservadas. El barrido periódico es el único que
      vence claims, para que un endpoint de lectura no cambie el estado sin liberar la wallet.
      La cancelación solo acepta ofertas locales todavía `Open`.
- [x] Al completarse: construir la transferencia privada usando **exactamente**
      `reserved_note_ids` y enviarla a la address canónica del comprador. Verificado con una
      transacción criptográfica real en test y con un pago Tron simulado.

### 2.4 Verificación de pago USDT vía Tor

- [x] Cliente HTTP implementado en **Rust/reqwest**, porque el handoff vigente exige consumir
      `tron::trc20_transfers_path` y aplicar `tron::transfer_settles_offer` sin duplicar esas
      reglas económicas en Kotlin. Usa `socks5h`, por lo que conexión y DNS salen por el proxy
      que ZSilent ya pasa al nodo mediante `--tor-proxy`; no existe fallback directo.
- [x] Manejar Tor caído como estado explícito: sin Tor no hay verificación, y el usuario tiene
      que verlo — no fallar en silencio ni caer a conexión directa.
- [x] TronGrid público como proveedor primario, API key opcional y lista ordenada de endpoints
      compatibles de fallback (`ZOKA_TRON_API_URL`, `ZOKA_TRON_API_KEY`,
      `ZOKA_TRON_FALLBACK_URLS`).
- [x] Solo el vendedor consulta. El loop filtra por `seller_address` local; el comprador recibe
      `OfferStatusUpdate` por gossip.
- [x] Matching: contrato USDT + address de cobro + monto + timestamp posterior al claim. Nunca
      solo por monto.
- [x] Pago de menos (fuera de tolerancia) → no libera; pago de más → libera igual.
- [x] Tolerancia de redondeo 0.5% y ventana de expiración de 45 min fijadas por tests.
- [x] Verificado: 17 tests de `p2p_exchange::tron`, incluidos proxy SOCKS, API key, decodificación
      de respuesta, fallback y negativa sin Tor. **No verificado contra TronGrid real.**
- [x] Exponer URL/API key/fallback en Settings. ZSilent los persiste y el `NodeController`
      los pasa al proceso mediante las tres variables de entorno. Verificado por round-trip
      del setup y por el test suite completo de ZSilent; no contra TronGrid real.

### 2.5 Persistencia y arranque

- [x] Ofertas y reservas sobreviven a un reinicio del nodo (verificado recargando ambos archivos
      en tests; no mediante reinicio de un nodo real).
- [x] **Al arrancar, reconciliar:** toda nota marcada `reserved` sin una oferta viva que la
      respalde tiene que liberarse. Sin esto un reinicio en mal momento deja fondos trabados
      para siempre.
- [x] Al reiniciar, reanunciar por gossip las ofertas todavía abiertas.
      También se reanuncian estados activos y los terminales recientes para cerrar ventanas
      crash-durability entre persistencia y publicación.

### 2.6 Presencia del vendedor

- [x] Una oferta remota solo aparece en el feed/tomable mientras recibió un heartbeat firmado
      del peer vendedor en los últimos 90 s.
- [x] Al desconectarse deja de reanunciar. Por la decisión nueva de presencia no se intenta
      emitir un update imposible desde un peer caído: el receptor la oculta al vencer el TTL.
- [x] La UI del comprador refresca el feed cada 5 s, por lo que una oferta deja de aparecer
      cuando el endpoint la elimina al vencer su presencia. Cubierto por implementación y
      compilación; **falta verificar visualmente con dos nodos**.

### 2.7 Claim atómico y liberación

- [x] Al tomar una oferta, marcarla `claimed` atómicamente: dos compradores no pueden tomarla
      a la vez. El comprador recibe `claim_requested` y espera el ack autoritativo del vendedor
      antes de pagar.
- [x] Liberación **automática** al detectar el pago verificado. No existe botón "liberar" que el
      vendedor pueda no apretar. Verificado de extremo a extremo con Tron simulado → tx ZKA
      construida → canal de broadcast; no contra TronGrid ni dos nodos reales.
- [x] Registro de incumplimiento persistido: si un vendedor no libera después del deadline,
      más 5 min de gracia, el comprador puede registrar la denuncia y el score local cuenta
      al vendedor.
- [x] Denuncia compartida por gossip, autenticada por la firma de nodo aceptada para v1 y
      **con un hash de tx USDT hexadecimal de 32 bytes como prueba**. Sin prueba no se propaga.
- [x] Advertir al comprador que denunciar expone públicamente su transacción USDT. ZSilent
      muestra el aviso, exige un checkbox y solo entonces envía el acknowledgement explícito
      junto con un hash hexadecimal de 64 caracteres.

### 2.8 Seguridad y consentimiento — gate obligatorio

No sale de "Coming Soon" sin esto.

- [x] Pantalla de riesgos con aceptación explícita la primera vez que se entra a la sección
      (mismo patrón que `CustodyWarningScreen` del onboarding). La aceptación se persiste.
- [x] Explicar en esa pantalla, en inglés claro: el lock es de software y no criptográfico; no
      hay árbitro ni reembolso; el vendedor podría incumplir; sin tope de monto.
- [x] **Riesgo asumido y preservado:** v1 sale sin tope de monto por oferta. El peor caso de un vendedor
      malicioso (cliente modificado que gasta notas "reservadas" tras cobrar el USDT) no tiene
      techo. Si aparece el primer abuso real, poner un tope es la mitigación inmediata a mano.
- [x] **Riesgo asumido y comunicado en la UI:** se prueba directo en mainnet con montos mínimos,
      sin fase de testnet.
      Mantener los montos realmente bajos hasta que el flujo completo se haya ejecutado varias
      veces sin incidentes.
- [x] Revisión de seguridad dedicada del backend (código + flujo económico) completada antes de
      habilitar para usuarios
      reales. Sin tope de monto y sin testnet, este gate es la única barrera antes de fondos
      reales. Cerró: journal/rebroadcast de release, chequeo Tron antes de expirar, ack de claim,
      rollback de transiciones, binding de wallet/reportes, gracia de denuncia, opt-in y
      consentimiento visual. Falta ejecutar el flujo real de monto mínimo.

Hallazgo abierto para esa revisión:

- [x] Tx ZKA de liberación y rebroadcast durable: la tx completa queda en la wallet cifrada,
      los inputs apuntan a su hash, `Released` conserva la reserva hasta confirmación y el loop
      recupera/reenvía el mismo hash. Verificado por round-trip serializado y rebroadcast en
      tests; **todavía no mediante restart de un nodo real**.

### 2.9 UI

- [x] Reemplazar `P2PExchangeScreen()`.
- [x] Publicar oferta: monto ZKA, monto USDT, address TRC-20 de cobro. Los decimales se
      convierten exactamente a atoms/micro-USDT; una séptima cifra se rechaza, no se redondea.
- [x] Listado de ofertas disponibles + tomar/cancelar oferta. El comprador ve
      `claim_requested` como request pendiente y espera el `Claimed` autoritativo antes de pagar.
- [x] Estado del intercambio: bloqueado → pago detectado → liberado / expirado, con countdown,
      polling cada 5 s y denuncia posterior al período de gracia.
- [x] Estado real de verificación Tor/Tron expuesto por el nodo; publicar queda bloqueado cuando
      no está disponible.
- [x] Flag opt-in en Settings, apagado por defecto, más consentimiento de riesgo persistido.
- [ ] Release conjunto nodo+ZSilent y prueba visual/real. La implementación UI quedó en
      `zsilent-core:etapa2-p2p-ui-integration` (`3f6b866`, `3ea8515`); `gradlew test` pasó
      28/28. **Eso no verifica el flujo contra dos nodos ni Tron mainnet.**

---

## Etapa 3 — Mining Pool (team mining sin custodia)

**Sin wallet de equipo.** El coinbase rota entre las addresses de los miembros según las shares
aportadas: cada bloque candidato se arma pagando a quien le toca. El coinbase *es* el pago — no
hay tx de reparto ni fondos en manos de un coordinador.

Esto elimina el riesgo de custodia por completo, a cambio de uno distinto: un miembro puede
encontrar un bloque que le paga a otro y reescribirlo para cobrarlo él. Ver §3.4.

### 3.1 Modelo de equipo

- [x] Estructura: `team_id`, miembros (address de cada uno), coordinador actual, coordinador de
      respaldo.
- [x] Invitación por código de equipo. Sin infraestructura de identidad nueva.
- [x] Sin límite duro de tamaño en v1.
- [x] Confirmar que no se toca el consenso: `validate_coinbase_shape` /
      `validate_coinbase_amount` (`src/state/chain.rs`) siguen exigiendo un receptor único —
      la rotación respeta eso por diseño.

Implementado en `src/team_mining/mod.rs` y expuesto por
`/team-mining/{status,team,join,leave,start,stop}`. El runtime persiste en
`<network-dir>/team_mining/team.json`. La validación de coinbase real vive hoy en
`src/mining/block_validator.rs` (la referencia original a `src/state/chain.rs` era conceptual);
no se cambió ninguna regla de consenso.

### 3.2 Protocolo de shares

- [x] Endpoint nuevo para enviar soluciones parciales al coordinador. No hay nada reutilizable
      de `/mining/*` (es solo para minería solitaria).
- [x] Dificultad de share **automática por minero**, ajustada a su hashrate para un ritmo parejo
      de envío. Un minero débil tiene que poder aportar; uno fuerte no debe saturar.
- [x] Validar las shares recalculando contra el header candidato: un miembro no puede inflar su
      aporte con shares falsas.

El “endpoint” distribuido quedó como mensaje `ShareSubmit` en el tópico dedicado, no como HTTP
central. Cada fullnode corre un worker `spawn_blocking`, cancela templates stale/stop y el
coordinador recalcula el PoW exacto antes de acreditar. Verificado con 12 tests de dominio y el
test dirigido `team_share_worker_solves_the_exact_coordinator_template`.

### 3.3 Rotación del coinbase

- [x] El coordinador arma cada template asignando el receptor según quién acumuló más shares sin
      cobrar todavía.
- [x] Llevar la cuenta de lo aportado vs. lo efectivamente cobrado por cada miembro, para que la
      rotación converja a proporcional en el tiempo.
- [x] Al confirmarse un bloque del equipo, descontar lo cobrado del saldo de turnos de ese
      miembro.
- [x] Comunicar en la UI que el reparto es **estadístico**: a corto plazo es desparejo, converge
      con el tiempo.

Una solución de red ya encontrada no salta validaciones: el coordinador recomputa hash/template,
reutiliza `run_mining_batch_with_progress`, pasa por `validate_pow_block` y
`state::chain::apply_block`, y recién entonces avanza el turno. La coinbase privada se construye
para una wallet remota sin cargar sus claves; el test prueba que solo el destinatario puede
reclamarla. Los fee claims privados siguen el mismo turno y el scanner evita duplicados.
La sesión distribuida de QA ejecutó el ensamblado completo con RandomX de testnet y dificultad
de ensayo 1. El coordinador aceptó 2 shares, minó el bloque 1 y ambos nodos expusieron el mismo
hash canónico `80439241d0cdd3352636969063196374eaa3759b45d9776e4d19ac615aa73210`.
**No se ejecutó con dificultad normal ni en mainnet.**

### 3.4 Detección de bloques hijackeados

- [x] Al confirmarse un bloque encontrado por el equipo, verificar que el receptor del coinbase
      sea el que el coordinador había asignado.
- [x] Si no coincide: marcar al miembro y expulsarlo del equipo.
- [x] Dejar claro en la UI, al unirse, que esto es detectable pero **no impedible** a nivel
      protocolo — un bloque válido que te paga a vos es válido para toda la red. El modelo asume
      equipos entre conocidos.

La comprobación corre tanto para el bloque ensamblado localmente como para un `BlockResponse`
que acaba de quedar canónico. Si el source firmado está ligado a un miembro, queda inactivo y
expulsado. Verificado por `coordinator_expels_source_bound_member_after_coinbase_hijack`.
Límite v1: si el source no puede atribuirse a un miembro conocido se registra el hijack, pero no
se adivina un culpable.

### 3.5 Coordinador de respaldo

- [x] Designar un miembro de respaldo al formar el equipo.
- [x] Si el coordinador se cae, el respaldo toma el rol automáticamente.
- [x] Transferir el estado de shares al respaldo para que nadie pierda lo aportado.
- [x] Sin riesgo de custodia agregado: no hay clave de equipo que compartir, solo el rol de
      coordinación.

Heartbeat 30 s, TTL 90 s y promoción por `epoch`. El full-state transfer incluye contadores,
replay ids y template; `revision` impide que gossip firmado fuera de orden restaure un estado
viejo. Además de los tests de promoción, transferencia y anti-rollback, se detuvo el proceso
coordinador en una red real de dos nodos: a los 90,5 s el backup pasó a coordinador,
`epoch` avanzó de 0 a 1 y el coordinador anterior quedó `active=false`.

### 3.6 UI

- [x] Reemplazar `MiningPoolScreen()`.
- [x] Toggle "team mining" (off por defecto).
- [x] Código de invitación, miembros activos, hashrate combinado.
- [x] Shares propias vs. del equipo, y turno estimado para cobrar.
- [x] Reusar el lenguaje visual de las `StatusCard` de la minería solitaria.
- [x] Flag opt-in en Settings.
- [ ] Release conjunto, pase visual y prueba real con dos nodos.

La pantalla consulta exclusivamente el RPC local cada 5 s y distingue nodo apagado, endpoint
ausente y error de transporte; un 404 del binario viejo se muestra como actualización requerida,
no como equipo vacío. Crear/unirse exige aceptar el disclosure de payouts estadísticos y hijack;
iniciar/detener, potencia, membresía, templates, shares y turnos usan el estado real del nodo.
Implementado en `zsilent-core:etapa2-p2p-ui-integration` (`d78adbb`, `ac2f855`).
`gradlew test` pasó 32/32, incluido decoding de payloads Rust y persistencia/upgrade del opt-in.
**No se levantó la app para verificar render/interacciones.** La sesión de dos nodos de §3.7
usó los endpoints directamente, no ZSilent.

### 3.7 QA distribuido y arranque

- [x] Dos fullnodes reales con `ZOKA_HOME`, wallets, DB, API y puertos P2P separados.
- [x] Descubrimiento por gossip, join, dos miembros y primer miembro aceptado como backup.
- [x] Workers reales con RandomX de testnet, 2 shares aceptadas y bloque canónico igual en ambos.
- [x] Caída real del coordinador: promoción a los 90,5 s, incremento de `epoch` y miembro viejo
      inactivo.
- [x] Corregido stack overflow del fullnode debug en Windows: el future grande ahora se aloja en
      heap (`63da315`). Verificado con `cargo build --bin zoka`, arranque aislado de 10 s y las
      dos sesiones distribuidas posteriores.
- [ ] Repetir minería con dificultad normal/mainnet y una sesión prolongada.
- [ ] Medir la convergencia del `TurnUpdate` post-bloque en el miembro; al instante de cortar la
      sesión el bloque ya era canónico en ambos, pero `confirmed_blocks` seguía 1/0.
- [ ] Pase visual desde ZSilent contra los endpoints nuevos.

Los ensayos usaron `--no-network-config` porque el manifiesto público de testnet todavía declara
el protocol freeze viejo `/zoka/1.0.0`, incompatible con el binario actual
`/zoka/1.9.0-mainnet-recovery`. Esto no alteró el protocolo entre los dos procesos iguales; ambos
se conectaron por seed local explícito. Queda como deuda de release actualizar el manifiesto de
testnet de forma coordinada, no cambiar el freeze unilateralmente desde esta etapa.

---

## Apéndice — referencias técnicas verificadas

Endpoints ya montados en el nodo (`src/api/mod.rs`), reutilizables sin cambios:

```text
GET  /blocks/latest
GET  /blocks/:height
GET  /blocks/:height/raw
GET  /blocks/:height/transactions
GET  /blocks/hash/:hash
GET  /blocks/range/:from/:to
GET  /transactions/:hash
GET  /mempool/pending
GET  /mempool/tx/:hash
GET  /accounts/:address
GET  /accounts/:address/proof
GET  /network/miners
POST /network/miners/heartbeat
```

Piezas a reutilizar (no reinventar):

```text
src/private_wallet/mod.rs
  reserved: bool / reserved_by_tx: Option<String>   (PrivateNote)
  release_reserved_notes(tx_hash)

src/state/chain.rs
  COINBASE_SENDER = "zoka:coinbase:v1"
  validate_coinbase_shape()    — exige exactamente 1 coinbase, 1 receptor
  validate_coinbase_amount()   — exige monto == subsidio esperado
  economics::COINBASE_MATURITY_BLOCKS

src/network/message.rs
  enum ZokaMessage (línea 33) — punto de extensión para OfferAnnounce

src/network/p2p.rs
  gossipsub con MessageAuthenticity::Signed
  tópicos: general / pbft / stem / mixer

zsilent-core/.../core/tor/TorSupervisor.kt
  proxy SOCKS para las consultas a la API de Tron
```

Stack de UI **vivo** en zsilent-core:

```text
Main.kt
  → ui/ZSilentApp.kt          (router, línea 124 = Explorer)
  → ui/theme/Theme.kt         (LightScheme, verde pastel #A8E6B0)
  → ui/screens/Screens.kt     (2943 líneas, incluye los placeholders)
  → ui/screens/ExplorerScreen.kt  (real, 106 líneas, maqueta sin RPC)
  → ui/components/            (ScreenScaffold, Panel, StatusCard, ZCard…)
```

Stack **muerto** a borrar en Etapa 0:

```text
design/                      (Colors.kt, Shapes.kt, Spacing.kt, Typography.kt, ZSilentTheme.kt)
app/ZSilentApp.kt            (112 líneas — router paralelo sin usar)
ui/screens/dashboard/        (incluye un 3er ExplorerScreen de 27 líneas)
ui/theme/Theme.kt            → DarkScheme (paleta lila #C084FC, sin usar)
```

Dependencias disponibles (`build.gradle.kts`):

```text
io.ktor:ktor-client-cio:3.0.1          — HTTP, soporta proxy SOCKS para Tor
kotlinx-serialization-json:1.7.3       — modelos de respuesta RPC y API
kotlinx-coroutines-core:1.9.0
compose.material3 + materialIconsExtended
com.google.zxing                        — QR (ya usado en ReceiveScreen)
```
