# Hoja de ruta: la red actual pasa a testnet, y mainnet nueva post-cuántica

Creada: 2026-10-11. Estado: plan acordado en lo general; decisiones abiertas al final.

---

## LEEME PRIMERO

Lo que decidió el usuario el 2026-10-11:

1. **La red actual (Zenith) pasa a ser la testnet.** Nadie pagó dinero por ZKA, así
   que la mainnet nueva arranca de cero: **no hay reclamo ni migración de saldos**.
2. **El P2P Exchange sale entero**: de ZSilent, del nodo y de la documentación, ya
   (no espera a la mainnet).
3. **VPS gratis** (con tarjeta solo para verificar la cuenta) para el explorador y para
   ZKAPriv, **además** de Tor, no en su lugar. **En espera: el usuario avisa cuándo.**
4. **Lo post-cuántico se implementa ahora** y se prueba en la testnet antes de lanzar
   la mainnet.
5. **La mainnet nueva se llama Zoka Quantic.**
6. **Premine de 5.000.000 ZKA para el equipo, gastable desde el bloque 0**, dentro del
   tope de 23.000.000. La minería emite los 18.000.000 restantes: 12 ZKA por bloque
   durante 1.051.200 bloques (12.614.400 ZKA) y 6 ZKA durante 897.600 bloques más.
   Con bloques de 120 s, la emisión termina a los ~7,4 años.
7. **El whitepaper se reescribe ya** con el enfoque nuevo (versión 2.0, Zoka Quantic).

Reglas de trabajo para este plan:

- Nada cambia el consenso de la red actual sin una **altura de activación** explícita.
- Una rama por etapa, un commit por subetapa, compilar y correr las pruebas al cerrar
  cada etapa (acuerdo de trabajo de julio).
- **No exagerar lo post-cuántico** en ningún texto público. Ver "Qué se puede prometer".

---

## Qué protege cada pieza hoy

Relevado en el código el 2026-10-11. Lo que importa es la última columna: si una
computadora cuántica futura puede romperlo **hacia atrás**, con la cadena que cualquiera
puede guardar hoy.

| Pieza | Dónde | Qué hace una computadora cuántica | ¿Hacia atrás? |
|---|---|---|---|
| Direcciones `zka1` (clave de escaneo + clave de gasto, Ristretto) y salidas ocultas | `canonical_payment_address`, `derive_stealth_payload` en `src/private_wallet/mod.rs` | Quien conozca una dirección puede reconocer todas las salidas que recibió | **Sí** |
| Monto cifrado para el destinatario | `encrypt_private_note_payload_v2` (clave derivada del intercambio de claves) | Quien conozca la dirección puede leer los montos | **Sí** |
| Comisión cifrada para el minero | `PrivateFeeClaim` (`ephemeral_pubkey` + `ciphertext`) | Igual que el monto | **Sí** |
| Key images + firma en anillo CLSAG | `RingSignatureBackend::ClsagV1`, `verify_ring_signature_with_backend` | Recalcula la key image de cada salida y ve cuál gastó cada transacción | **Sí** |
| Nullifiers | `blake3(commitment ‖ spending_key)`, `src/transaction/nullifier.rs` | Es un hash: resiste. **A verificar:** si la clave pública de gasto de la dirección es `spending_key·G`, quien rompa esa clave también calcula los nullifiers | Depende |
| Compromisos Pedersen + Bulletproofs | `src/zk/range_proofs.rs` | El monto queda oculto igual (ocultamiento perfecto), pero se podrían crear monedas sin que se note | No, solo hacia adelante |
| Groth16 sobre BLS12-381 | `src/zk/groth16_backend.rs` | La prueba no revela nada (conocimiento cero perfecto), pero se podrían falsificar pruebas | No, solo hacia adelante |
| Ed25519 | Firma de snapshots (`src/snapshot.rs`), identidades de red | **No protege fondos.** Se podría falsificar un snapshot de arranque | No |
| Prueba de trabajo (RandomX), SHA-2/3, BLAKE3, AES-256-GCM, Argon2 | — | Resisten | — |
| Transporte (libp2p Noise, iroh, Tor) | — | Tráfico grabado hoy se podría descifrar, pero casi todo lo que viaja ya es público | Poco valor |

Los bloques no llevan firma: el `seal` del encabezado es el hash de la prueba de trabajo
(`src/mining/block_validator.rs`). Las transacciones son todas privadas; el tipo
`Transaction` público queda prácticamente para el coinbase.

### El principio que ordena todo

**Lo que protege la privacidad tiene que ser post-cuántico desde el génesis**, porque se
rompe hacia atrás: alguien puede guardar la cadena hoy y leerla en quince años.
**Lo que protege la validez** (que nadie cree monedas ni gaste lo ajeno) se rompe solo
hacia adelante, así que se puede migrar con un fork el día que haga falta, siempre que
los formatos indiquen qué esquema usan.

### Corrección pendiente en textos públicos

`zokaweb/docs/whitepaper.md` (líneas 75-77 y 436-437) y `zokaweb/docs/litepaper.md`
(152-153) dicen que la privacidad pasada sobreviviría a una computadora cuántica. **Hoy
no es así**, por las key images y por el intercambio de claves de las direcciones.

### Qué se puede prometer al terminar

"Privacidad post-cuántica: direcciones, montos y el grafo de gastos siguen ocultos aunque
mañana exista una computadora cuántica." **No** "a prueba de computación cuántica": la
validez (Groth16, Bulletproofs, Pedersen) sigue siendo clásica hasta que haya un
reemplazo maduro (pruebas basadas en hashes, tipo STARK).

---

## Etapas, en el orden en que se ejecutan

### Etapa 0 — Cerrar la actualización de seguridad en curso

- [x] Tor 0.4.9.13 (Expert Bundle 15.0.24) instalado en ZSilent; guía, script y CI
      actualizados. Respaldo de 0.4.9.12 en `ZSilent Core\backup-tor-0.4.9.12-20261011`.
- [x] Dependencias del nodo (rustls, anyhow, chacha20, spin); 995 + 76 pruebas en
      verde; `zoka.exe` instalado en ZSilent (SHA256 `7DBCF418…`, respaldo
      `zoka.exe.bak-20261011`).
- [x] npm: Zveil y bot sin alertas; sitios de 39 a 16 (las que quedan exigen versión mayor).
- [ ] Motor móvil: `.so` de los dos ABIs, commit, APK (instalar solo cuando el usuario diga).
- [ ] Tor 0.4.9.14 cuando exista un Expert Bundle que la traiga (15.0.24 trae 0.4.9.13).
- [ ] Push de las ramas `seguridad-octubre-2026` (ZOKA, zkapriv, zokaweb, zokaexplorer,
      zoka-guard-vault) y `tor-0.4.9.13` (zsilent-core).

### Etapa 0b — Sacar el P2P Exchange entero

Pedido del usuario el 2026-10-11: "sacalo todo".

0b.1 Relevar antes de borrar: qué toca `src/p2p_exchange/` (~2.900 líneas) en el resto
     del nodo (API, CLI, gossip, notas reservadas de la wallet) y si algo del exchange
     quedó en bloques de la cadena actual. Si hay datos en bloques, la validación de
     esos bloques viejos tiene que seguir funcionando en la testnet.
0b.2 Nodo: módulo, rutas de la API, comandos del CLI, mensajes de red y pruebas.
     Las notas marcadas `reserved` por ofertas viejas tienen que volver a quedar
     gastables.
0b.3 ZSilent: pantallas, navegación y llamadas a la API.
0b.4 Documentación y sitio: todo lo que mencione el exchange o Tron.

### Etapa 0c — Whitepaper 2.0 (Zoka Quantic)

Reescribirlo con el enfoque nuevo, marcando cada cosa como hecha, en testnet o en
diseño, igual que la 1.0. Corrige la promesa falsa sobre la privacidad pasada y
declara el premine. El PDF del sitio se genera con xhtml2pdf; publicarlo en
`zoka.network` es una decisión aparte del usuario.

### Etapa 1 — Base post-cuántica

1.1 Librerías candidatas: `ml-kem` 0.3.x y `ml-dsa` 0.1.x de RustCrypto. Son jóvenes y
    no tienen auditoría conocida: anotarlo como riesgo, no ocultarlo. Para combinar
    X25519 con ML-KEM usar una construcción especificada (X-Wing); el crate `x-wing`
    sigue un borrador viejo, así que evaluar si conviene implementar el combinador
    nosotros siguiendo la especificación, que es corto.
1.2 Que compile en los cinco destinos: Windows x86_64, Linux x86_64, **Linux aarch64**
    (para el VPS de Oracle), Android arm64-v8a y **armeabi-v7a** (el teléfono del usuario).
1.3 **Medir en el teléfono de 32 bits** el desencapsulado ML-KEM: el escaneo prueba
    cada salida de la cadena, así que este número decide si hacen falta etiquetas de
    vista para descartar rápido.
1.4 Con esas mediciones, cerrar D1 y D2.

### Etapa 2 — La red actual pasa a ser la testnet

2.1 **El chain id no se puede renombrar.** El hash sintético del génesis sale de
    `MAINNET_CHAIN_ID` (`src/economics.rs`), así que cambiarlo rompe la cadena. Los
    clientes nuevos presentan `zoka-mainnet-zenith-v1` como testnet y retiran
    `zoka-testnet-v1`, que nunca estuvo operativa.
2.2 Directorios de datos (`.zoka/mainnet` → `.zoka/testnet`) en el CLI y en ZSilent,
    **sin perder wallets**: copiar, verificar y recién después cambiar la ruta.
2.3 Marco de activación por altura para las funciones post-cuánticas, como ya existe
    para `PRIVATE_TX_V2_I1_ACTIVATION_HEIGHT`.
2.4 Los clientes viejos van a seguir llamándola "mainnet". No rompe nada; el anuncio lo
    explica.
2.5 Anuncio: los ZKA actuales pasan a ser de prueba y la mainnet nueva arranca de cero.

### Etapa 3 — Direcciones y cifrado híbridos

3.1 Versión nueva de dirección: clave de escaneo + clave de gasto + clave pública
    ML-KEM, con su propio prefijo y checksum.
3.2 Un solo secreto compartido híbrido (intercambio de claves clásico + ML-KEM,
    combinados con un KDF que incluya el ciphertext y las claves públicas) para:
    la clave de un solo uso, la etiqueta de vista, el monto cifrado, la comisión
    cifrada del minero y el coinbase privado (`PrivateCoinbaseOutput`).
3.3 Cada salida lleva el ciphertext ML-KEM (1088 bytes con ML-KEM-768).
3.4 Escaneo en el nodo (ZSilent) y en el motor móvil (ZKAPriv).
3.5 Activación por altura en la testnet y prueba de punta a punta: envío ZSilent →
    ZKAPriv y ZKAPriv → ZSilent, en el teléfono real.

### Etapa 4 — VPS para la testnet (en paralelo con la 3) — EN ESPERA

**No arrancar hasta que el usuario avise** (pedido del 2026-10-11). Él crea las
cuentas; el resto lo armo yo.

4.1 **Oracle Cloud Always Free** (ARM, hasta 4 núcleos y 24 GB entre todas sus
    máquinas): nodo que **no mina**, seed, y generador de los datos del explorador.
    Oracle recupera las máquinas que ve ociosas: vigilarlo.
4.2 **Google Cloud e2-micro** (1 GB, solo regiones de EE.UU.): segundo seed. Medir la
    RAM antes; RandomX en modo liviano usa unos 256 MB.
4.3 Explorador: el nodo del VPS genera el JSON y lo sube a Cloudflare Pages con un
    cron, como `zoka.network`. Se deja de depender de GitHub Actions.
4.4 ZKAPriv: conexión directa a los VPS (iroh/QUIC) para sincronizar rápido, con Tor
    como modo privado. El VPS ve la IP del teléfono, no qué notas son suyas, porque el
    teléfono baja todos los bloques y escanea localmente.
4.5 Reglas: **nunca minar en un VPS gratis** (es lo que más prohíben sus términos y lo
    que terminó con GitHub); dos proveedores distintos; la red tiene que seguir andando
    si los dos desaparecen.

### Etapa 5 — Gastar sin key images

Objetivo: que una computadora cuántica futura no pueda reconstruir qué salida gastó
cada transacción. Es la etapa más grande y **arranca investigando**.

5.1 Verificar si el circuito Groth16 actual ya prueba, por sí solo, la propiedad de la
    nota y que el nullifier está bien calculado, sin depender de la firma en anillo.
5.2 Clave de nullifier que **no** se pueda deducir de las claves públicas de la
    dirección (derivada del seed con una función pseudoaleatoria, no un punto de curva).
5.3 Mainnet nueva: Groth16 obligatorio en cada gasto, sin key images ni CLSAG.
5.4 Costo: una prueba en cada envío (unos 14 s en el teléfono con el perfil v1).
5.5 Cambia los circuitos, así que la ceremonia de la Etapa 7 tiene que ser posterior.

### Etapa 6 — Firmas híbridas de snapshots

6.1 `src/snapshot.rs`: Ed25519 + ML-DSA-65, las dos obligatorias.
6.2 Opcional: firmar los releases con el mismo esquema.

### Etapa 7 — Lo que hay que cerrar antes del génesis

Después del génesis cualquier cambio de estos exige un fork.

7.1 libp2p a la versión mayor nueva (arrastra las alertas sin arreglo: hickory-proto,
    lru 0.12, core2, instant).
7.2 Emisión: los bloques salen demasiado rápido (dificultad trabada en el piso,
    ~2 bloques por minuto). Fijar tiempo de bloque y dificultad inicial.
7.3 Premine en el génesis: 5.000.000 ZKA a una dirección del equipo, contado dentro
    del suministro emitido para que `block_reward_for_height` corte la minería en
    18.000.000. Monto y dirección públicos en el génesis, para que cualquiera lo
    verifique.
7.4 Alinear versiones: crate, `NETWORK_MIN_CLIENT_VERSION` y manifiesto no coinciden.
7.5 Bug de ZSilent que ofrece notas ya gastadas (saldo inflado).
7.6 **Ceremonia multiparte del trusted setup** con los circuitos finales. Si los
    parámetros los genera una sola persona, esa persona puede crear monedas.
7.7 Revisión externa, al menos de las etapas 3 y 5.
7.8 Revisar el whitepaper 2.0 contra lo que de verdad quedó implementado.

### Etapa 8 — Génesis y lanzamiento

8.1 Zoka Quantic: chain id nuevo (propuesta: `zoka-quantic-v1`) y génesis nuevo.
8.2 Todo lo post-cuántico activo desde el bloque 0, sin alturas de activación.
8.3 Seeds: VPS + onion + la PC del usuario.
8.4 Anuncio que explique por qué este relanzamiento es distinto del de Zenith (junio
    2026) y el compromiso de que no habrá más reinicios.

---

## Decisiones abiertas

| # | Decisión | Recomendación |
|---|---|---|
| D1 | ML-KEM-768 o ML-KEM-512 | 768: es la elección estándar (la usa TLS); 512 ahorra unos 400 bytes por dirección y 300 por salida |
| D2 | Largo de las direcciones: con 768 rondan los 1.700 caracteres (1.200 con 512) | Aceptarlo y apoyarse en QR y copiar/pegar; las alternativas exigen un directorio de claves, que filtra privacidad |
| D5 | Tiempo de bloque y dificultad inicial (la curva ya está decidida, ver LEEME) | Revisar con números en la Etapa 7 |

Decididas el 2026-10-11: nombre **Zoka Quantic**; premine de **5M para el equipo,
gastable de inmediato, dentro de los 23M**; el exchange **sale entero ya**; los VPS
**esperan** a que el usuario avise.

## Riesgos

- Es el **segundo relanzamiento** en el año: si no se explica bien, espanta a los que quedan.
- **Premine del 21,7 % sin bloqueo:** el whitepaper 1.0 prometía "sin premine". Hay que
  declararlo de frente (monto, dirección y que es gastable de inmediato); escondido o
  minimizado es lo que más rápido destruye la confianza en una moneda nueva.
- Librerías post-cuánticas **jóvenes y sin auditoría** conocida.
- **Tamaño:** cada salida suma más de 1 KB; pesa en la cadena y en los datos del teléfono.
- **Escaneo más lento** en el teléfono de 32 bits (se mide en la Etapa 1).
- Los VPS gratis pueden suspenderse sin aviso (precedente: GitHub, agosto 2026).
- La validez sigue siendo clásica: ver "Qué se puede prometer".
