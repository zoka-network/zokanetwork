# Mapa de contexto — ZKAPriv sobre P2P (Android)

**Para:** el próximo agente que retome esto (Codex u otro).
**Fecha del corte:** 2026-08-13.
**Ramas:** `wallet-movil-pide-bloques-p2p` (repo `ZOKA`, el nodo) y
`wallet-lee-la-cadena-por-p2p` (repo `zkapriv`, la wallet Android).
**Ninguna fusionada a `main`.**

## Actualización Codex — 2026-08-13

- Nodo final desplegado por ZSilent y activo en `3000/28080`, con copia de
  recuperación `zoka.exe.pre-deadline-budget-20260813-113406.bak`.
  SHA-256 desplegado: `01BC7E65822676CA8008AF9B10BAA448012E493172CAE82234134CF1B61D744B`.
- Prueba observable por la onion contra ese binario, usando el lote real de
  Android: 250 resúmenes fueron 181.819 B en 9 s; los bloques completos del
  mismo tramo fueron 1.130.975 B en 21 s. Ahorro medido: **6,2x**.
- APK debug final instalada con `adb install -r`, sin borrar datos. El APK del
  dispositivo coincide exactamente con el local: SHA-256
  `C89954330F9D97A60F75411C69142B1168FC5F2E80EF98B282B7DEC4B655C560`.
  Se conservaron cache/estado Tor, `zoka-red/node_key`, trusted setup y la fecha
  original de instalación.
- El Galaxy carga `app_process32`. Su `.so` `armeabi-v7a` final es ELF32/ARM,
  mide 12.133.752 B, tiene SHA-256
  `3E1076A41F7D4686AA08E580EF32AD89E1493D44B6BFC7D7B4E09CC1E125D94C` y
  exporta `blocksDigestRange0`. El entry del APK coincide byte por byte.
- La `.so` ARM64 final mide 15.612.424 B y tiene SHA-256
  `361FF23D6FF06574C8D6BA14F22379B29F8D0A4390455823BFDE0FF2345E273F`.
- Nodo/P2P: cobertura exacta, `prev_hash`, continuidad, request IDs, colas
  acotadas, compatibilidad v1, failover secuencial y validación canónica de
  cuerpos. La integración de tres peers descarta una rama inválida, usa el
  segundo y no envía el rango al tercero. Cada tramo intenta como máximo tres
  peers para resúmenes y dos para bloques, reservando deadline al siguiente.
- Android: el scanner de saldo y el de gasto usan resúmenes; solo piden el
  bloque completo para coinbases propios. Coinbases ya no se truncan, los
  checkpoints son síncronos y los rangos parciales no avanzan el snapshot.
- Envío: contrato de publicación normalizado y outbox cifrada/reanudable. El
  mismo retry reanuncia el mismo cuerpo/hash, los envíos se serializan y las
  reservas no se liberan por un TTL ciego.
- Privacidad: feeds CoinGecko/CoinPaprika deshabilitados y proveedores HTTP
  retirados mientras no exista transporte Tor; no hay fallback clearnet.
- UI: un saldo incompleto/no fresco se señala y bloquea Max/Send. Se revalida
  importe + comisión, QR y el estado final tras autenticar. Una caché solo
  habilita gasto si altura y hash de punta coinciden con la red actual.
- Rendimiento Android: escaneo, snapshots cifrados, outbox y reservas se
  ejecutan fuera de Main; los fallos de checkpoint ya no se silencian.
- Verificación: `cargo check`, build release, pruebas P2P focalizadas e
  integración multi-peer, tests `network/security`, compilación UI y
  `assembleDebug` pasaron. Queda observar el escaneo desbloqueado y autorizar
  manualmente un envío real con destino/importe elegidos por el usuario.

---

## Cómo usar este documento

Este archivo es el estado real, no un plan optimista. Incluye lo que quedó
funcionando, lo que falta, **los errores que cometí y cómo detectarlos**, y lo
que sospecho que omitimos sin darnos cuenta.

**Instrucciones de trabajo:**

- Cuando termines un punto de las listas, **táchalo** (`- [x] ~~texto~~`) y
  agregá en una línea debajo qué hiciste y cómo lo verificaste.
- Cuando agregues algo que no estaba en la lista, sumalo con `+ NUEVO:` y
  explicá por qué hizo falta.
- Cuando propongas una mejora que no vas a hacer ahora, sumala en
  **Propuestas** al final, con el motivo por el que no se hizo.
- **Buscá colaterales.** Casi todos los errores de esta sesión fueron efectos
  laterales: un arreglo destapó el siguiente. Antes de dar algo por cerrado,
  preguntate qué otra cosa dependía de lo que acabás de cambiar.

---

## 1. El problema original

ZKAPriv (wallet Android nativa, Kotlin + Rust por JNI) hablaba con la cadena
por una **API REST contra dos nodos de DigitalOcean** que se dieron de baja en
julio de 2026. La wallet quedó muerta: apuntaba a `159.223.119.216:3000` y
`157.230.5.103:3000`, que ya no existen.

El objetivo era que la wallet **hable con la red directamente** — pedir bloques
por P2P y publicar transacciones por gossip — sin depender de que nadie
mantenga un servidor.

---

## 2. Lo que quedó hecho y verificado

### 2.1 La wallet habla P2P (repo `zkapriv`)

- La interfaz `ZokaNodeApi` no cambió. Se escribió **otra implementación**
  (`P2pZokaNodeApi`) y se cambió quién la provee en el módulo de Hilt.
  **Ninguna pantalla se tocó.**
- Lo que un cliente liviano no puede servir (saldo público, mempool,
  estadísticas) lanza `UnsupportedOperationException` con mensaje claro, en vez
  de devolver ceros que parecerían un saldo real.
- `sendFromSeed` (mandaba la frase semilla a un servidor) **no vuelve**: en
  autocustodia no corresponde y el servidor ya no existe.

### 2.2 Tor embebido (Arti 0.45)

`zkapriv/native/src/tor.rs` levanta Arti dentro del proceso y expone un SOCKS5
propio en localhost. **No se usa el proxy que trae `arti`** porque arrastra su
CLI y configuración por archivos, que en Android no aplican.

Costo: el `.so` de `armeabi-v7a` pasó de 4,4 MB a 11,5 MB.

**Verificado en el dispositivo** (Galaxy A13, SM-A135M, armeabi-v7a, Android
14): Arti bajó el directorio de la red Tor y dejó `files/tor-cache/dir.sqlite3`
y `files/tor-state/keystore`.

### 2.3 Tres fallos que SOLO aparecen en un teléfono real

Ninguno era detectable desde una PC. Cada uno tapaba al siguiente.

1. **Identidad en directorio de solo lectura** (`os error 30`). El nodo guarda
   `.zoka/node_key` *relativa al directorio de trabajo*, que en Android es `/`.
   Arreglo: `Node::new_with_tor_proxy_at(..., identity_path)` y `data_dir` en
   `MobileNetConfig`.
2. **`/etc/resolv.conf` no existe en Android** (`os error 2`). `with_dns()` lo
   lee al construirse y moría toda la creación del swarm. Arreglo:
   `dns_del_sistema_disponible()` elige entre `with_dns()` y
   `with_dns_config(ResolverConfig::new(), ...)` — **resolutor vacío, no uno
   público**: cablear el DNS de Google le contaría a un tercero que ese teléfono
   usa Zoka.
3. **El contador de pares mentía** (decía 0 mientras bajaba bloques). En un
   `select!`, cuando gana una rama la otra se descarta a mitad de camino, y la
   anotación estaba después del bombeo.

### 2.4 Altura de la red: preguntar, no esperar

La primera versión escuchaba anuncios de bloque durante 120 s. **Los bloques de
esta red tardan 138 s de mediana**, con picos de media hora: la espera era más
corta que el intervalo. Y con la cadena quieta no hay espera que alcance.

Ahora se les **pregunta** a los pares hasta dónde llegan, subiendo a saltos que
se duplican y afinando por mitades (`descubrir_altura` en `src/mobile_net.rs`).

**Colateral que esto destapó:** la espera inútil de 120 s venía tapando que
**no había reconexión**. Se marcaba a las semillas una sola vez, al arrancar.
Ahora `esperar_par` vuelve a marcar cada 15 s antes de pedir altura, pedir
bloques y publicar.

### 2.5 Altura servible ≠ altura anunciada

Gossip propaga anuncios de toda la malla: un par atrasado reenvía la punta que
minó otro **sin tener el bloque**. Medido: la semilla anunciaba 17325 y solo
servía hasta 17061. La wallet pedía hasta lo anunciado y **fallaba entera**.

`chain_height_json` devuelve `{"height":servible,"announced_height":anunciada}`.

### 2.6 El gasto doble que quedaba "pendiente" para siempre

Este fue el problema más caro de la sesión y tenía **cuatro capas**:

| Capa | Qué pasaba |
|---|---|
| La wallet elegía notas por su marca local (`!note.spent`) sin mirar la cadena | Construía envíos imposibles |
| Mi primer arreglo intentaba abrir la base que el nodo tiene tomada | **Código muerto que nunca corrió** |
| `spent_nullifier_height` reventaba en error si el valor no tenía el largo esperado | El error caía en silencio |
| `/private/tx/:hash` tenía dos caminos y arreglé uno | El otro devolvía el estado decorativo |

**Arreglos:**

- `reconciliar_notas_gastadas` en `src/private_wallet/mod.rs`: deriva el
  nulificador de cada nota y consulta el índice antes de elegir.
- Endpoint nuevo **`GET /private/nullifier/:tag`** → `{"spent":bool,"height":N}`.
  Existe porque la wallet **no puede abrir la base**: la tiene tomada el nodo
  mientras corre, o sea siempre.
- `private_transaction_chain_state()` distingue `Confirmada` / `GastoDoble` /
  `Gastada` / `Pendiente` mirando el bloque donde se gastó el nulificador.
- El campo `status` del cuerpo **es decorativo**: lo estampa quien crea la
  transacción y no se actualiza nunca. La API ahora lo reemplaza por el estado
  real.
- La cola lateral (`pending_private_queue/`) ya no borra en silencio: guarda el
  cuerpo antes de descartar, para que el nodo pueda explicar qué pasó.

**Verificado end-to-end:** el envío `b857d6de303dd07c69843fd2f1015a4e` eligió un
nulificador nuevo (`ac2cb1144b07f50ac3f8...`), el nodo lo aceptó en la cola y
un minero lo incluyó en el **bloque 17771**.

### 2.7 Escaneo de saldo incremental

Recorrer la cadena entera en cada refresco era razonable contra REST. Sobre Tor
son ~50 minutos, y **con el saldo caducando cada 30 s el escaneo nunca
alcanzaba a terminar antes de que lo reiniciaran**. Medido en el teléfono: tres
arranques, ningún final.

- Instantánea por dirección en `SecureStorage` (`balance_scan_<address>`).
- **Ancla contra reorganizaciones**: se pide el bloque de la última altura
  escaneada y se verifica que siga teniendo el mismo hash. Si cambió, se rehace.
  Sin eso, retomar sería una fuente silenciosa de saldos equivocados.
- **Puntos de control cada 1.000 bloques** con porcentaje en el log.
  **Verificado:** `retomando desde 2001 (instantánea en 2000)` en 4 segundos.

### 2.8 Protocolo de resúmenes `/zoka/wallet-digests/1`

**Medido contra la cadena real:** un bloque pesa 5.432 bytes y **3.069 son la
prueba de rango del coinbase** (56%). Una wallet los bajaba todos y los
descartaba.

| | Bloques enteros | Resúmenes |
|---|---|---|
| Por bloque | 5.432 bytes | 549 bytes |
| Cadena entera (17.780) | 92 MB, ~60 min | 9,3 MB, ~6 min |

**9,9x menos datos.**

La prueba de rango no se puede omitir siempre: entra en el hash que forma el
`output_id` y se verifica al gastar. De ahí el reparto — resumen para todos,
bloque entero solo donde aparezca un coinbase propio.

**Caída a bloques enteros** si el par no conoce el protocolo. Un fallo de red
dispara la misma caída, para que quien espera no quede colgado.

**Probado con dos nodos reales hablando por TCP**, no con llamadas a funciones:
`dos_nodos_se_pasan_resumenes_de_bloque` y
`un_par_sin_resumenes_deja_caer_a_bloques_enteros` en `src/network/p2p.rs`.

### 2.9 Arreglos colaterales encontrados sin buscarlos

- **Los precios nunca funcionaron.** `RpcEndpointInterceptor` reescribía el host
  de *toda* petición del cliente HTTP compartido, así que le pedía las
  cotizaciones al nodo en vez de a CoinGecko. Se quitó el interceptor: ahora que
  la cadena va por P2P no tiene nada que reescribir.
- **~4 MB de basura por APK.** `cargo-ndk` dejaba `.so` intermedios de `iroh` e
  `if_watch` en `jniLibs` y Gradle los empaquetaba. `build-android.ps1` ahora
  los descarta.

### 2.10 Herramienta de diagnóstico (usar esto, vale oro)

`examples/probar_cliente_movil.rs` corre **el mismo cliente móvil desde una PC**
contra la red real. Arti no escribe en logcat, así que en el teléfono un Tor
lento y uno fallido se ven igual.

```bash
third_party/tor/windows-x86_64/tor.exe --SocksPort 9251 --DataDirectory <dir>
cargo run --release --example probar_cliente_movil -- 127.0.0.1:9251 [<hash tx>]
ZOKA_SEED=/ip4/127.0.0.1/tcp/28080 cargo run --release --example probar_cliente_movil -- 127.0.0.1:9251
```

Con un hash le pide el cuerpo a los pares: **es la única forma de saber si una
tx privada propagó de verdad** (la red anuncia el hash y solo da el cuerpo a
quien lo pide).

---

## 3. Commits

**Repo `ZOKA`, rama `wallet-movil-pide-bloques-p2p`:**

```
17d333ea  Una wallet ya no baja la cadena entera para saber su saldo
b9212229  Una wallet ya puede preguntarle al nodo si su nota sigue existiendo
5172c781  La wallet le pregunta a la cadena antes de elegir que gastar
25670f91  La wallet pregunta la altura y se reconecta sola
5faace4b  La cola lateral ya no borra en silencio una transaccion muerta
689bf3a5  Un gasto doble ya no se reporta como confirmado ni espera para siempre
737b7ecf  El nodo ya no exige un resolutor de DNS del sistema para existir
1e47dd15  El cliente movil lee hasta donde sus pares pueden servir, y escribe donde puede
4c116de3  La wallet del telefono pide bloques a la red, no a un servidor
```

**Repo `zkapriv`, rama `wallet-lee-la-cadena-por-p2p`:**

```
3a8bc91  El escaneo de saldo recuerda hasta donde llego
a3d065a  Decirle al nodo donde escribir, y dejar de secuestrar las peticiones de precios
3a991f7  Recompilar la biblioteca nativa con Tor y la lectura de la cadena
87e3ad2  La wallet habla con la red, no con dos servidores que ya no existen
6be11d6  Exponer el cliente de red P2P a la wallet, y probarlo en el telefono
```

---

## 4. LO QUE FALTA

### 4.1 Bloqueado por verificación en dispositivo

- [ ] **Ver el saldo en el teléfono.** El envío está confirmado en el bloque
      17771 hacia
      `zka19BeuhbSSpwswDZRNwayd91CW23RJqrzf8YeiFJZC6roqFGTSiypxL8DGmPx6KnYuJpXYthYNBY6SJh2LhFyMQmjiNErvF`.
      Falta que el escaneo llegue hasta ahí. **Nunca lo vimos completar.**
- [ ] **Probar el envío DESDE Android.** Es el objetivo original y sigue sin
      probarse. La wallet arma y prueba la transacción en el teléfono (eso ya
      andaba antes); falta verificar que **propague** — comprobarlo pidiéndole
      el cuerpo a la semilla con la herramienta de 2.10.
- [x] ~~**Desplegar el `.so` y el APK con resúmenes.**~~
      Nodo final, `.so` ARM64/ARMv7 y APK instalados; hashes exactos anotados en
      la actualización superior. `adb install -r` conservó todos los datos.
- [x] ~~**Verificar que el nodo sirve un resumen REAL**, no solo que el binario
      tenga el protocolo adentro.~~
      Medido por onion después del despliegue final: 181.819 B de resúmenes
      contra 1.130.975 B de bloques completos para el lote móvil de 250 alturas.

### 4.2 Limitaciones conocidas de lo que escribí

- [x] ~~**`request_wallet_digests` usa `peers.first()` y no tiene failover.**~~
      Ahora rota peers y prueba uno por vez; no duplica el rango a todos. Una
      integración con tres peers verificó inválido → válido → tercero intacto.
- [x] ~~**`IncrementalPrivateChainScanner` (el que usa el envío) sigue pidiendo
      bloques enteros.**~~
      Usa resúmenes con continuidad y pide cuerpo completo únicamente para una
      altura donde reconoce coinbase propio, verificando hash/altura/campos.
- [ ] **`PrivateTxChainState::Gastada`** (nulificador gastado pero sin altura
      conocida) devuelve la etiqueta `"Spent"`, que ninguna wallet reconoce. Cae
      con gracia a la comprobación por bloques, pero conviene decidir qué
      debería mostrar.
- [x] ~~**La instantánea del escaneo se guarda con `.apply()`** (asíncrono).~~
      Los checkpoints/finales usan `commit()` y propagan fallos. El scanner de
      gasto persiste además al cerrar cada lote, antes de pedir cuerpos.

+ NUEVO: se corrigió el contrato P2P de publicación que devolvía
`{"published":true}` mientras Kotlin exigía `hash`; podía difundir una tx y
mostrar fallo. Ahora valida ambos y devuelve el hash construido.

+ NUEVO: el DTO de coinbase privado conserva los 16 campos canónicos que Rust
necesita para reclamar/probar la nota; antes el envío truncaba la prueba de
rango y fallaba en `serde`.

+ NUEVO: ningún rango parcial, duplicado, desordenado o de otra rama puede
avanzar el checkpoint. Nodo, adaptador y Kotlin validan cobertura/continuidad.

+ NUEVO: outbox cifrada con cuerpo/hash/reserva/requestKey antes del broadcast.
Un timeout o muerte de proceso reanuncia el mismo cuerpo y no construye otro
pago. Las reservas se restauran y solo se liberan con evidencia en cadena.

+ NUEVO: un cuerpo privado se deserializa y se liga al hash pedido/canónico
antes de cachearlo. Un peer corrupto ya no gana el race e impide el failover.

+ NUEVO: la wallet no consulta precios por DNS/clearnet. Se usan solo valores
locales cacheados hasta implementar un cliente HTTP dedicado sobre Arti.

### 4.3 Fuera de ZKAPriv pero afecta a todo

- [ ] **La wallet marca UNA sola semilla.** El manifiesto
      (`docs/mainnet-network.v1.json`) tiene una: la PC del usuario, que es
      seed1. Si se cae, la wallet se queda sin datos; si miente, nadie la
      contradice. **Un segundo seed onion es más barato que cualquier cambio de
      código.**
- [ ] **El reajuste de dificultad es demasiado grueso.** `retarget_difficulty`
      (`src/main.rs`) mueve **un bit por bloque**, y un bit duplica o divide el
      trabajo a la mitad. Con red chica oscila entre 14 s y 32 min por bloque
      (medido). Bitcoin ajusta cada 2016 bloques con una proporción suave.
      Está anotado como pendiente pre-lanzamiento.
- [ ] **El nodo de ZSilent sale con código 1 en modo `--sync-only`** (sin sesión
      de wallet abierta) y ZSilent reintenta hasta rendirse. Pasa con el binario
      original también — **no es una regresión**, pero deja el nodo caído hasta
      que el usuario inicia sesión. **Sin mensaje de error**: sale 1 y no dice
      por qué. Esto merece investigarse: es un nodo que no arranca y no explica.

---

## 5. LAS TRAMPAS DONDE CAÍ (leé esto antes de tocar nada)

### Trampa #1 — "compila y está en el binario" NO es "corre"

**Caí dos veces en la misma sesión.**

1. Escribí la reconciliación de notas dentro de
   `if let Ok(chain) = open_storage(path)`. La base la tiene tomada el nodo
   **siempre**, así que esa rama nunca se tomó. Código muerto. El usuario envió
   de nuevo y salió la misma transacción imposible.
2. Arreglé el estado en `/private/tx/:hash` pero el manejador tenía **dos
   caminos** (pool en memoria y almacenamiento) y el del pool devolvía antes,
   salteándose todo.

En los dos casos **verifiqué que el código estuviera** —incluso busqué los
textos dentro del `.exe` con `grep -a`— sin verificar que el camino real pasara
por él.

**Regla:** comprobá el **efecto observable**. Consultá la API, mirá el log del
dispositivo, medí los bytes. No alcanza con que compile.

### Trampa #2 — una sola medición es ruido

Diagnostiqué que mi binario rompía el arranque del nodo porque el viejo
"sobrevivió" una prueba y el mío no. **Era ruido**: el viejo falla igual. Actué
sobre esa conclusión, revertí, y dejé el nodo del usuario caído una hora.

También dije que la semilla estaba caída cuando era mi Tor recién arrancado, y
que la red necesitaba 9 horas para recuperarse cuando estaba minando en ritmo
(leí `miners_online: 1` como "sos el único minero", pero ese contador solo ve
los que le reportan a ese nodo).

**Regla:** repetí la medición y buscá el control. Antes de culpar a un cambio,
probá que sin el cambio no pasa.

### Trampa #3 — arreglar uno destapa el siguiente

Toda la sesión fue así: Arti → directorio de solo lectura → DNS inexistente →
espera de altura demasiado corta → falta de reconexión → escaneo que nunca
termina → volumen de datos. **Siete capas.**

**Regla:** cuando arregles algo, asumí que hay otro fallo detrás y buscalo
activamente en vez de declarar victoria.

### Trampa #4 — `strings` no sirve para `.so` de Android

Usé `strings` y no encontró los símbolos JNI; concluí mal. `llvm-nm -D` sí los
mostró. Para binarios de Windows, `grep -a -c "texto"` funciona bien.

---

## 6. LO QUE SOSPECHO QUE OMITIMOS SIN SABERLO

Esta sección es especulación fundada. **Verificá antes de actuar.**

- [ ] **¿El escaneo completo alguna vez terminó?** Nunca vimos un
      `scanFullBalance: DONE`. Puede que haya un fallo más allá de la fase de
      bloques (fase 2: pedir cuerpos de transacciones privadas) que no
      diagnosticamos porque nunca llegamos.
- [ ] **`missingPrivateTxCache.clear()` en cada escaneo.** Con resúmenes, los
      cuerpos de tx privadas siguen pidiéndose de a uno. Si la cadena tuviera
      muchas transacciones, eso volvería a ser el cuello. Hoy hay casi ninguna,
      así que el problema está oculto.
- [ ] **El caché de saldo dura 30 s** (`BALANCE_CACHE_TTL_MS`). Con el escaneo
      incremental eso ya no duele, pero si un refresco tarda más de 30 s se
      pueden encadenar escaneos. Revisar si el mutex alcanza.
- [ ] **Batería.** El cliente móvil bombea el bucle de eventos cada 250 ms
      mientras esté conectado, y mantiene circuitos Tor abiertos. `disconnect()`
      existe pero **nadie lo llama** cuando la app pasa a segundo plano.
- [ ] **Tamaño del APK: 98,8 MB.** Cuatro ABIs, y `libbarhopper_v3.so` (ML Kit,
      lector de QR) son 17 MB. Un APK por ABI o un App Bundle lo bajaría mucho.
- [ ] **¿Qué pasa si el teléfono cambia de red a mitad del escaneo?** Hay
      reconexión, pero no está probado el caso wifi→datos con circuitos a medio
      armar.
- [ ] **Reorganizaciones durante un escaneo largo.** El ancla se verifica al
      **empezar**, no durante. Un escaneo de 6 minutos con una reorg en el medio
      podría acumular datos de una rama muerta.
- [ ] **El `output_id` del coinbase viene del bloque, no se recalcula.** Si un
      par mintiera sobre ese campo en un resumen, la wallet contaría una nota
      que no puede gastar. Un cliente liviano confía en sus pares por diseño,
      pero conviene decidir si se verifica al pedir el bloque entero.

---

## 6bis. Actualización Claude — 2026-08-15

### El nodo liviano de ZKAPriv, terminado

**Hallazgo que cambió el marco:** no había que construirlo. Verificado con
`llvm-nm` sobre el `.so` de `armeabi-v7a`: **cero símbolos de RandomX y cero de
RocksDB**. La wallet ya no minaba, no guardaba la cadena ni servía bloques. Lo
que faltaba era encender el descubrimiento; lo que sobraba, suscripciones.

**`NodeProfile::Wallet`** (`src/network/p2p.rs`). El nodo se suscribía a seis
temas de gossip y la wallet usa tres. Estar suscrito obliga a **recibir y
reenviar**: en un teléfono con Tor eso son datos y batería gastados en
mensajes que se descartan.

| Se queda | Se va |
|---|---|
| `messages` (anuncios de bloque, tx privadas) | `offers` — P2P Exchange |
| `stem`, `mixer` (publicar con privacidad) | `pbft` — consenso |
| | `team_mining` |

No saca código: el crate es el mismo, solo deja de pedir lo que no va a mirar.
El perfil `Full` no cambia en nada.

**Descubrimiento encendido.** `bootstrap_kademlia()` ya existía y la wallet
nunca lo llamaba. Ahora, con un par en mano, le pide a la red que le presente a
los demás.

**Memoria de pares con rotación**, usando el `PeerStore` del nodo completo — el
mismo formato, la misma lógica, ningún archivo nuevo inventado. `dial_addrs()`
ordena por puntaje, última conexión exitosa y fallos: un par caído baja solo.
Las semillas del manifiesto van siempre al final de la lista, como garantía de
entrada para una wallet nueva. Se aprende cada dos minutos, no en cada vuelta
del bucle (que gira cuatro veces por segundo).

Vive en `<data_dir>/zoka-red/peer_store.json`. Sin `data_dir` cae a una ruta
relativa que en Android no se puede escribir: degrada a depender de las
semillas, no rompe.

### Compresión de resúmenes

Medido con datos de la forma real: **556 B → 51 B por bloque, 10,9x**. El peso
era texto repetido — cuatro hashes en hexadecimal y los nombres de los campos
JSON, una vez por bloque.

Backend **puro Rust** (`flate2` con `rust_backend`) a propósito: `zstd` y `lz4`
ya estaban en el árbol pero traen C, y meter una biblioteca en C en la
compilación cruzada para ARM de 32 bits es pedir un fallo que solo aparece en
el teléfono.

Un byte de formato al frente para que quien lee sepa qué recibió sin adivinar.
Formato desconocido → error → la wallet cae a bloques enteros.

El tramo de escaneo de Android pasó de 250 a 1.000 bloques: con la compresión
el cuello deja de ser el ancho de banda y pasa a ser el viaje de ida y vuelta.

### La guarda de frescura que bloqueaba Enviar (síntoma real del usuario)

El panel mostraba saldo y **Enviar quedaba deshabilitado siempre**. La guarda
exigía terminar el escaneo *exactamente* en la punta:

```kotlin
balance.scannedHeight == finalHeight && verifiedTipHash == finalTipHash
```

Es una carrera contra un blanco móvil: la red produce un bloque cada dos
minutos y el escaneo tarda. **Nunca se cumplía.**

Lo que la guarda protege es real y se conserva: que una reorganización durante
el escaneo convierta una rama muerta en saldo gastable. Pero eso se detecta
preguntando si el bloque donde terminamos **sigue siendo** el de esa altura, no
si es el último. Ahora el ancla distingue "quedamos atrás" (se alcanza, hasta
3 intentos) de "cambió la rama" (falla cerrado).

**Se corrigió una prueba que fijaba el defecto:**
`changedTipReturnsFailClosed...` montaba un crecimiento normal (`prevHash`
encadenaba limpio) y se llamaba "changedTip". Ahora monta una reorganización de
verdad y hay otra prueba para el crecimiento.

### Lo que falta para que esto sirva de verdad

- [ ] **Un segundo seed onion.** Todo el descubrimiento depende de encontrar a
      alguien alcanzable, y el manifiesto tiene **una sola** semilla: la PC del
      usuario. Es lo más barato con más impacto y no es código.
- [ ] **Soltar la red en segundo plano.** `disconnect()` existe y **nadie lo
      llama**: los circuitos de Tor quedan abiertos con la pantalla apagada.
- [ ] **Sincronización en segundo plano.** No hay `ForegroundService` ni
      `WorkManager`: el escaneo solo corre con la app abierta. Sin esto no hay
      aviso de fondos recibidos y la wallet es una herramienta de consulta.
- [ ] **APK de 98,8 MB.** Cuatro ABIs + 17 MB de ML Kit (lector de QR). Un App
      Bundle o splits por ABI lo dejan en menos de un tercio.
- [ ] **Cambio de red a mitad de escaneo** (wifi ↔ datos): hay reconexión, sin
      probar.
- [ ] **Idioma mezclado**: mensajes nuevos en inglés, usuarios hispanohablantes.
      Decidirlo antes de que haya cien cadenas.

### Verificado bien (no hace falta revisarlo)

Capturas de pantalla bloqueadas (`FLAG_SECURE`), comisión con valor de respaldo
si falla la estimación, PIN + biometría, respaldo de semilla en la instalación.

---

## 6ter. Actualización Claude — 2026-08-16

Todo lo de acá está medido en el teléfono del usuario (SM-A135M, armeabi-v7a),
no razonado. Donde digo un número, salió del registro.

### Hecho

- ~~**Enviar nunca se habilitaba.**~~ La guarda pedía `missingTxCount == 0` y 12
  de los 266 cuerpos de esta cadena ya no existen: el par que tiene la cadena
  entera responde `private tx body not found`, no un tiempo agotado.
  **Protegía contra el riesgo equivocado.** Los gastos se detectan por
  nulificador y los nulificadores vienen commiteados en el bloque — llegan en
  los resúmenes, sin pedir un solo cuerpo. Un cuerpo perdido solo puede ocultar
  dinero **entrante**: deja el saldo por debajo del real, nunca por encima, y
  gastar de menos es seguro. Ahora avisa en ámbar y deja enviar. Lo que sí
  bloquea es un saldo sin anclar a la cadena. Dos pruebas fijan las dos mitades.

- ~~**Cerrar la app costaba todo el escaneo.**~~ La instantánea se escribía
  **antes** de la fase de cuerpos: guardaba qué bloques se recorrieron pero no
  qué se encontró en ellos. Cada arranque rebajaba los 266 cuerpos para
  redescubrir lo mismo. Ahora el esquema es v2 y guarda el resultado
  (`ownedOutputs`, `processedTxHashes`, `ourOutputsByTx`, `txInputNullifiers`).
  Un cuerpo que falló por red **no** se marca procesado, y nada se guarda sin
  ancla de cadena.

- ~~**Cada arranque redescubría que 12 cuerpos no existen.**~~ La lista de
  irrecuperables vivía en memoria. Medido: el recorrido de bloques tardaba 2 s y
  esos reintentos 60 s — el 97% del tiempo. Ahora persiste, con el reintento
  periódico cada 10 escaneos intacto.

- ~~**El panel iba a ciegas.**~~ Franja discreta con las **dos** fases: medir
  solo los bloques la dejaba clavada en 99% durante todo el tramo lento, que es
  justo donde se va el tiempo. Un indicador que se congela en el tramo lento es
  peor que no tener ninguno.

- ~~**Estado de la red.**~~ Con el criterio de ZSilent: un conteo de pares sin
  fecha miente, porque el número se actualiza cuando el nodo bombea eventos y
  envejece si está ocupado. Tres estados — en vivo / último conocido / nunca
  leído — con línea de tendencia. Un solo par se marca en ámbar (`no fallback`).

- ~~**Esperas heredadas de HTTP.**~~ 350 ms por tanda + 150 ms entre tandas + 2 s
  de entrada venían del explorador web contra el nodo REST. La app habla P2P
  directo: **no hay limitador en ese camino**. Más de 20 s de espera contra un
  guardia que no está. El reintento por 429 queda por si algún par sí limita.

### La trampa más cara del día

**Recompilé Kotlin y di por hecho el resto.** Los `.so` eran de 08-15 16:08 y la
precarga de cuerpos se escribió el 08-16 10:14. Todos los APK que instalé ese día
llevaban la app nueva con **el motor de red viejo**, así que la precarga que
convierte 266 pedidos en 3 nunca corrió en el teléfono — y yo interpretaba los
tiempos como si sí.

**Regla: tras tocar `src/mobile_net.rs` o `zkapriv/native/`, correr
`native/build-android.ps1` ANTES de `assembleDebug`.** Comparar fechas de
`native/jniLibs/*/libzoka_mobile.so` contra el último commit del motor.

### La trampa conceptual

`SerializedNativeCommandGate` es un **mutex**: todos los pedidos al nodo se
serializan. La "concurrencia de 6" de `PRIVATE_TX_FETCH_BATCH_SIZE` no existía —
los seis esperaban en fila. **Subir la tanda no acelera nada**; solo amortiza las
pausas. Quien vea que va lento y suba ese número va a perder el tiempo.

### Lo que sigue abierto

- **Un solo par en la red.** Es el techo de todo lo demás: cada `Timeout while
  waiting for a response` del registro es eso. Un segundo seed onion sigue siendo
  lo más barato con más impacto, y no es código.
- **Enviar desde Android nunca se probó.** Es el objetivo original.
- `disconnect()` existe y nadie lo llama cuando la app pasa a segundo plano.
- Sin `ForegroundService`/`WorkManager`: no hay sincronización en segundo plano
  ni aviso de fondos entrantes.
- APK de 98,8 MB (4 ABIs + 17 MB de ML Kit). App Bundle o splits.
- Toda la interfaz está en inglés y el usuario trabaja en español.

## 7. COLATERALES Y ALTERNATIVAS A EVALUAR

Buscá activamente en estas direcciones. No las descarté por malas — no las
exploré.

- [ ] **Bootstrap por iroh en vez de escaneo bloque a bloque.** El nodo ya sirve
      instantáneas por iroh (QUIC + hole punching, sin puertos ni Tor) y un nodo
      fresco se pone al día en ~90 s. **¿Podría la wallet usar eso** en vez de
      recorrer la cadena? Cambiaría 6 minutos por segundos. Hay que ver qué
      implica para la privacidad y si el `.so` puede hablar iroh.
- [ ] **Filtros de bloque tipo BIP-157/158.** En vez de resúmenes por bloque, el
      nodo podría servir un filtro compacto y la wallet pedir solo los bloques
      que le interesan. Es el estándar en clientes livianos y **mejora la
      privacidad**: hoy pedir un bloque entero delata interés.
- [ ] **Un segundo seed onion.** Lo más barato con más impacto. Ver 4.3.
- [x] ~~**Repartir los resúmenes entre pares** (ver 4.2).~~
      Se descartó el reparto por altura: mezcla ramas y revela el rango a más
      peers. La implementación final rota peers y hace failover secuencial por
      tramo completo, que conserva continuidad y minimiza exposición.
- [ ] **¿El `range_proof` del coinbase tiene que estar en el bloque?** Es el 56%
      del peso de la cadena. Si pudiera derivarse o comprimirse, bajaría el
      costo para todos los nodos, no solo para wallets.
- [ ] **Comprimir la respuesta de resúmenes.** Son JSON. `zstd` o incluso
      serializar binario los bajaría bastante más.
- [ ] **Escaneo en un servicio de primer plano de Android** con notificación de
      progreso, en vez de depender de que la app esté abierta y la pantalla
      encendida.

---

## 8. Entorno y datos concretos

| | |
|---|---|
| Teléfono | Galaxy A13 (SM-A135M), **armeabi-v7a de 32 bits**, Android 14 |
| ADB | `/c/ADB/adb.exe` |
| Paquete | `com.zoka.zkapriv.benchmark` |
| Compilar `.so` | `zkapriv/native/build-android.ps1` — **NO usar `2>&1`**, PowerShell convierte el progreso de cargo en errores y aborta |
| Nodo del usuario | Lo lanza ZSilent; **es seed1** (`--external-addr /onion3/d5pk3nh...`) |
| Binario del nodo | `C:\Users\...\AppData\Local\ZSilent Core\app\resources\bin\zoka.exe` |
| Respaldos | `zoka.exe.respaldo-20260812`, `zoka.exe.previo-20260813` |
| Tor para pruebas | `third_party/tor/windows-x86_64/tor.exe` |
| Semilla onion | `/onion3/d5pk3nhesk5wlvz4f5unbhcozxgtj7u6mhagauyprjrxndzpwbaoeiid:28080` |

**Restricciones del usuario:**

- No tocar cosas críticas del sistema ni perder lo que ya funciona.
- **Es un protocolo de anonimato total** — cualquier decisión que filtre datos a
  terceros (DNS público, telemetría) está descartada aunque funcione.
- Hay gente que usa línea de comando: si algo se rompe ahí, también hay que
  arreglarlo.
- **No commitear `discord-bot/`** (tiene `.env` con el token).
- Nunca publicar frases semilla ni claves privadas.
- Preferencia explícita: **preguntar y dar recomendación** antes de decisiones
  grandes.

**Transacciones de referencia:**

| Hash | Qué pasó |
|---|---|
| `81acbe6fcd212495065f06ff56507913` | Gasto doble, nulificador `427543926f...` ya gastado en el bloque 318 |
| `515b773ccb0ecdd5e4324b6b3be8118d` | El mismo gasto doble otra vez (mi arreglo no corría) |
| `b857d6de303dd07c69843fd2f1015a4e` | **Confirmada** en el bloque 17771 — el arreglo funcionando |

---

## 9. Propuestas del agente

*(Agregá acá lo que propongas y no hagas, con el motivo. Dejá la fecha.)*

- **Segundo seed onion (prioridad máxima):** el código ya tiene failover, pero
  el manifiesto solo ofrece seed1. Sin una segunda infraestructura no existe
  redundancia ni contraste real en producción.
- **Autenticar el cliente liviano:** `prev_hash` evita mezclar ramas, pero un
  único peer aún puede fabricar una cadena autoconsistente. Reutilizar headers
  validados/checkpoints y pruebas Merkle o filtros compactos tipo BIP-157/158.
- **Lifecycle real de Arti/nodo:** integrar `ProcessLifecycleOwner`, cancelación
  con deadline y shutdown/join reiniciable. Hoy `disconnect()` no apaga el
  runtime Tor y puede gastar batería en segundo plano.
- **Persistencia transaccional:** la outbox se recupera de una ventana entre
  sus dos commits, pero Room/archivo cifrado + MAC permitiría outbox/reserva en
  una sola transacción y snapshots grandes sin bloquear UI.
- **Distribución Android:** generar App Bundle o APK por ABI. El universal
  instalado ronda 120,7 MB; el Galaxy solo necesita ARMv7.
- **Reducir el grafo móvil:** RocksDB/iroh hacen que una recompilación ARMv7
  tarde decenas de minutos aunque la wallet liviana no abra una cadena local.
  Separar features nativas móviles para no enlazar componentes innecesarios.
- **Actualizar AGP/Kotlin:** el build funciona, pero AGP 8.1 solo declara
  compatibilidad probada hasta compileSdk 33 y el proyecto compila con 36.
