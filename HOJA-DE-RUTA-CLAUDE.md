# Zoka Quantic: hoja de ruta para continuar con Claude

Actualizada: **30 de octubre de 2026**, Argentina (**testnet v2 lanzada**: bloque 1 fijado y la PC como seed1; nodo, minero CLI y ZSilent 4.0.x publicados; **ZokaScan nuevo en línea** en zokascan.cc con su API en api.zokascan.cc).
Proyecto: `C:\Users\mderramus\ZOKA`.

## 30/10: ZokaScan en línea, con saldo por clave de vista (el usuario: «que se puedan buscar los balances con privacidad… API mínima… online… charts»)

Decisiones del usuario: túnel desde la PC, nodo 4.0.2, «clave de vista en el
navegador» y gráficos de «red y nodos». Publicación aprobada con «Sí, todo».

- **Clave de vista** (`cbd59770`, rama `quantic-v2-explorer-api`, pusheada a
  GitHub y GitLab):
  - `zoka-pq-pool`: `ViewKey` (`zqtestview2…`, 130 bytes y suma de control)
    con la semilla X-Wing, la clave de salida y el dueño. Abre lo recibido y
    reconoce las transferencias propias, porque toda salida de una
    transferencia va sellada también con la clave de salida del que envía.
    Por eso da el saldo exacto: recibido − (salidas + comisión) de las
    propias. No puede gastar: el nulificador necesita la clave de gasto.
  - Módulo `scan`: `ScanPage` (postcard) y `Viewer` (saldo, gastable,
    inmaduro y la regla de madurez del libro mayor).
  - CLI: `quantic-pq-wallet show-view-key`.
- **API pública nueva:**
  - `/scan?from=H&count=N`: 100 bloques de salidas cifradas, unos 280 KB.
    Inmutable bajo el checkpoint, y con un balde propio (`fa73edb0`, 1 por
    segundo, ráfaga de 10) para no saturar la subida de la PC.
  - `/supply` y `/supply/total` (texto plano).
  - `/blocks/stats`: hasta 720 bloques.
  - `/network/history`: una muestra cada 5 minutos durante 24 h, guardada en
    la base.
  - `/`: la lista de endpoints.
- **`crates/zoka-pq-scan-wasm`:** el mismo código compilado con wasm-pack
  (`--target web`, 145 KB). El explorador lo lleva copiado en
  `src/lib/scan-wasm/`; hay que recopiarlo si cambia el formato.
- **Explorador** (zokaexplorer `explorer-quantic-v2`, `9ab7a89` y `6c24953`,
  pusheada):
  - páginas `/wallet` (Mi billetera), gráficos en `/network` (cada uno con
    vista de tabla) y `/api`;
  - CSP en `public/_headers`: solo scripts propios y WebAssembly, y
    conexiones solo a api.zokascan.cc;
  - el buscador entrega la clave en memoria, nunca en la URL.
- **Pruebas:**
  - 43 pruebas STARK, incluida una de la API pública con saldo por escaneo, y
    17 del pool;
  - el WASM en Node contra la cadena real: 531 bloques en 0,5 s;
  - Mi billetera en Edge, por DevTools, con la CSP, en local y en
    zokascan.cc: escanea hasta la punta, sin errores y sin la clave en la URL.
- **En producción:**
  - Pages `zokaexplorer`, rama `main`, despliegue `d7df94df`;
  - el túnel `zokascan-api` (`0eefc2be-759a-45c5-a2fc-1062407752c0`), con el
    CNAME `api.zokascan.cc` creado por `cloudflared tunnel route dns`.
- **En la PC, en `%APPDATA%\Zoka\.zoka\`** (todo abierto con explorer.exe):
  - `explorer-node\` (el `zoka.exe` de esta rama y `explorer-node.cmd`, API
    pública 127.0.0.1:9181);
  - `cloudflared\` (cloudflared 2026.10.1, con hash y firma verificados,
    `config.yml`, `zokascan-api.json` con las credenciales, que son secretas,
    y `tunnel-zokascan.cmd`);
  - `cert.pem` del login de Cloudflare, en `~/.cloudflared/`.
- **PUBLICADO el 30/10, a la tarde** (el usuario: «sí, todo ahora»). Antes
  el usuario vio en ZSilent 4.0.2 los renglones «Scan private key» y «Spend
  private key» vacíos. Eran las dos claves hex de Zenith: `exportSecrets` de
  Quantic solo pedía la frase.
  - **Nodo 4.0.2:** `main` = `v4.0.2` = `6ccc0c96`, en GitHub y GitLab. Trae
    el minero CLI con `view-key` y `show-view-key.cmd`, y las guías con la
    sección 6. Paquetes: Windows `9f113091…` y Linux `051079ab…` (Debian 11
    en WSL). Los dos probados con `view-key`.
  - **ZSilent 4.0.3** (zsilent-core `main` = `v4.0.3` = `9c89406`):
    - «Show view key» pide `show-view-key` y explica que no gasta, y que en
      Quantic no hay clave de gasto aparte: gastar pide la frase;
    - las 81 pruebas pasan, incluida la de punta a punta contra el nodo v2
      real;
    - MSI `e2e8ea7d…`; extraído, trae el `zoka.exe` 4.0.2 y no trae Tor.
  - **Releases en GitHub:** las dos, bajadas de vuelta y con `sha256sum -c`
    OK. La 4.0.1 y la ZSilent 4.0.2 quedaron marcadas como reemplazadas.
  - **Web:** zokaweb `abc62c4`, desplegada y pusheada. ZokaScan ya no figura
    «offline», hay guía de la clave de vista y descargas 4.0.3/4.0.2.
  - **zokaexplorer:** `main` avanzó a `6c24953`, lo que está en producción.
- **ZSilent 4.0.4** (zsilent-core `main` = `v4.0.4` = `897b766`, pusheada;
  release en GitHub sin crear todavía). El usuario pidió: «saquemos lo de
  Tor… copiar la dirección y truncarla… instalala, comiteá y subí».
  - **Sin indicadores de Tor:** el nodo v2 no tiene transporte por Tor y
    ningún control lo prendía. Salen la pastilla de la barra, la de This
    device con su aviso, «Privacy & Tor» (ahora «Network privacy»), la
    política de About, la pastilla y la tarjeta de Logs, y la línea de la
    intro. La página Network explica qué se ve. `TorSupervisor` queda en el
    código, apagado.
  - **CopyableKeyValueRow:** corta al medio (18 + 12) y el botón de copiar
    queda siempre a la vista; se copia el valor entero.
  - 82 pruebas, con capturas de Ajustes y de las filas. El MSI `df5ea7ff…`
    trae el nodo 4.0.2 y no trae Tor.
  - **Instalada en la PC** a las 10:57: la 4.0.3 a las 10:26 y la 4.0.4
    después. XMRig se cerró las dos veces al apagarse ZSilent y se relanzó.
  - **Modo Tor real, planificado:** un transporte propio de iroh 1.3
    (`unstable-custom-transports`) que lleva los paquetes de QUIC por Tor
    hasta la .onion de la semilla. Arti embebido como cliente y `tor.exe`
    0.4.9.14 para el servicio .onion de la semilla. El canal PQ de iroh va
    adentro, porque Tor no es postcuántico. Nodo solo saliente, sin UDP,
    relays ni IP publicada.
- **Falta:**
  - Release en GitHub de ZSilent 4.0.4 (y la web a 4.0.4), con el OK del
    usuario.
  - Los papers describen la clave de vista solo como la clave X-Wing; la
    `ViewKey` agrega la clave de salida y el dueño. Corregirlo en la próxima
    edición.
  - El beacon de Cloudflare Web Analytics queda bloqueado por la CSP. Es a
    propósito: no conviene un script de terceros en una página que recibe
    claves. Apagar Web Analytics en el panel o dejarlo así.
  - Si /scan crece mucho: una regla de caché de Cloudflare (pide permiso de
    zona) o un proxy con Cache API en Pages.

## 29/10, tarde (2): revisión de ZSilent y 4.0.2 (el usuario: «controlá si zsilent está funcionando bien»)

- **Bien:**
  - el nodo de la 4.0.1 tiene el Stratum, el cobro, la autoridad, la API en 9140
    y 270 MB;
  - el contador del Stratum cuadra con la cadena, y una rama lateral (244) no
    sumó;
  - la emisión cierra (5.002.988 ZKA en la altura 250);
  - un nodo nuevo con solo las semillas compiladas bajó los 254 bloques en
    unos 20 s y aceptó el checkpoint 226;
  - la copia de la wallet guarda 4 logs viejos.
- **Error encontrado (4.0.0 y 4.0.1):** `Main.kt` arma el cliente del nodo local
  con `networkSettings.localRpcUrl`, que salía de `desktop.localRpcUrl` del
  manifiesto. El v2 no lo trae: la URL quedaba vacía y `status()` fallaba antes
  de conectar (`netstat`: ninguna conexión de la app al 9140). La app no
  mostraba altura, pares ni Stratum, y el saldo se refrescaba solo al entrar.
  El nodo (que usa el getter con la caída a 9140) y la minería andaban. Lo
  destapó el arreglo del nombre del manifiesto (`7b8547f`): el de Zenith traía
  `desktop.localRpcUrl` con el puerto 3000.
- **Arreglo:** zsilent-core `2db8911`, ZSilent 4.0.2 con el nodo 4.0.1. La
  configuración cae en `http://127.0.0.1:9140`. La prueba nueva falla con el
  código de la 4.0.1 y pasan las 81. El MSI `12ae31e6…` está en
  `target/zsilent-4.0.2/build/release/4.0.2` y se verificó extrayéndolo.
- **Falta, con el OK del usuario:** el release 4.0.2, la web (ZSilent 4.0.2) y
  la instalación en la PC.
- **A mirar:** la ventana usa ~16 % de un núcleo dibujando sin parar (hilos de
  AWT y de Direct3D de skiko). Puede ser una animación de «esperando al nodo»
  que con la 4.0.2 se apague.

## 29/10, tarde: 4.0.1 y papers 2.5 (Claude; el usuario: «arreglá eso inmediatamente»)

Los tres pendientes chicos de la 4.0.0, arreglados:

- **Stratum** (`28083c02`): `Backend::found` devuelve si el bloque cuenta. El
  backend STARK lo importa en el momento y solo cuenta el canónico; una segunda
  solución del mismo trabajo queda como rama lateral y ya no suma a
  `blocks_found`. La prueba de punta a punta lo cubre, y pasan también las del
  Stratum de la v1.
- **ZSilent sin Tor** (zsilent-core `0f8a614`): la app nunca arrancaba Tor y el
  paquete viajaba en el MSI. Se sacó del armado y de `prepare-release.ps1`. El
  MSI pasó de 154 a 132 MB.
- **Papers 2.5** (zokaweb `cdc23aa`, rama `web-papers-2.5`, sin publicar):
  - whitepaper y litepaper reescritos para la v2 (STARK, iroh, límites);
  - los PDF actuales se llaman `ZOKA_Quantic_Whitepaper.pdf` y
    `ZOKA_Quantic_Litepaper.pdf`, con la versión adentro (pedido del usuario);
  - `public/_redirects` manda los nombres `_v2.0.pdf` a los nuevos.
- **Armado y verificado:**
  - nodo 4.0.1 `zoka.exe` `4264f8f5…`;
  - minero CLI 4.0.1, Windows `e045b6fc…` y Linux `4df6cb07…`, probado en
    WSL;
  - MSI `ZSilent-Core-4.0.1.msi` `77fdc92b…`, extraído: binario idéntico y sin
    Tor.
- **Pusheado:** `main` y las etiquetas `v4.0.1` (nodo en `e18fe03c` y
  zsilent-core en `0f8a614`), a GitHub y GitLab.
- **PUBLICADO** (el clasificador lo frenó primero; el usuario dio el OK, «Sí,
  todo ahora»):
  - releases 4.0.1 en GitHub (`zokanetwork` con los dos paquetes CLI y
    `zsilent-core` con el MSI). Bajados de vuelta, `sha256sum -c` da OK. La
    4.0.0 quedó marcada como reemplazada;
  - la web (zokaweb `fe3635c`), verificada en vivo: JS, PDF y redirecciones
    301 de los nombres viejos. `.gitattributes` fija LF para
    `public/_redirects` (git lo pasaba a CRLF al hacer checkout);
  - ZSilent 4.0.1 **instalada en la PC** (código 0; nodo `4264f8f5…`, sin la
    carpeta de Tor). Al abrirla, el usuario vuelve a entrar para que el nodo
    arranque con el Stratum.

## 29/10: lanzamiento de la testnet v2 (Claude, rama `quantic-v2-genesis`)

Quinto punto del orden acordado. El usuario: «encargate de todo vos desde
aquí». El procedimiento y el registro completo están en
[docs/LANZAMIENTO-TESTNET-V2.md](docs/LANZAMIENTO-TESTNET-V2.md).

- **Bloque 1** a las 08:35 (-03:00), con el premine en la wallet del equipo,
  que el usuario llamó `zsilent`: hash
  `69249437d8c35b7e4bf28ec524491fe18384b9869670fadff546fef29148fdb2`. Se minó
  con XMRig por el Stratum del nodo de ZSilent 4.0 de desarrollo, la opción A.
  `85cc0c19` lo fija junto con seed1, la PC (endpoint `e3bf05e2…`).
- **La semilla, hasta instalar el MSI:** la app de desarrollo (`gradlew run` en
  zsilent-core) corre el nodo con la API en 127.0.0.1:3000 y el Stratum en
  3333. XMRig va con 3 hilos y `-u equipo`, que cobra el `--mine-to`. Cerrar la
  app para la red.
- **Probado:**
  - un nodo en una carpeta nueva, solo con las semillas compiladas, sincronizó,
    siguió la cadena y aceptó los checkpoints 2 y 33 de la PC;
  - el minero CLI de Windows, en una ruta con espacios: el nodo, XMRig y la
    corta publicada (la semilla la conocía); su XMRig de prueba encontró el
    bloque 57, que es canónico en la semilla.
- **Arreglado en el camino:**
  - `zsilent-core` `7b8547f`: `--mine-to` sin el Stratum, el manifiesto de
    Zenith y el Tor del arranque automático;
  - `5ada09f5`: versión 4.0.0, RocksDB con 5 logs viejos en vez de 1000, el
    saldo legible y el minero CLI v2 (sin Tor, con XMRig);
  - `8e63f913`: los comandos v2 usan RandomX liviano. Antes cada comando de
    wallet con bloques nuevos armaba el dataset de 2 GB (unos 50 s con todos
    los núcleos); ahora tarda 2,3 s y el nodo usa 270 MB.
- **Armado, sin publicar:**
  - nodo `zoka.exe` 4.0.0 `dd259065…`;
  - MSI `ZSilent Core-4.0.0.msi` en `target/zsilent-4.0.0/build/release/4.0.0`
    (`e55caae9…`), armado en un worktree de zsilent-core con el Tor de
    `third_party/tor/windows-x86_64-0.4.9.14`. Se verificó extrayéndolo: el
    binario y el manifiesto son los de la fuente;
  - minero CLI en `target/cli-miner/4.0.0`.
- **PUBLICADO el 29/10** (el usuario: «Sí, todo»):
  - `main` y las etiquetas `v4.0.0` (anotadas) en GitHub y GitLab, por avance
    rápido: nodo `v4.0.0` = `8e63f913` y zsilent-core `v4.0.0` = `7b8547f`;
  - releases en GitHub: `zokanetwork` v4.0.0, con los dos paquetes CLI y
    `SHA256SUMS.txt`, y `zsilent-core` v4.0.0, con `ZSilent-Core-4.0.0.msi`.
    Bajados de vuelta, los hashes coinciden. El `SHA256SUMS.txt` del nodo salió
    con una línea CRLF que `sha256sum -c` no leía; se subió corregido y el
    armado quedó arreglado (`8ae8995b`). La 3.0.4 y la 3.0.5 quedaron marcadas
    como reemplazadas;
  - la web zoka.network (`zokaweb` `58f7600`), con Tech y Docs de la v2:
    STARK, iroh sin Tor, XMRig por el Stratum, `zqtest2` y los puertos
    9140/9180/3333. Se verificó en vivo. El `dist` anterior quedó en
    `target/zokaweb-dist-0805a5c-en-vivo`;
  - ZSilent 4.0.0 **instalada en la PC** (`msiexec /passive`, código 0, nodo
    `dd259065…`). La app de desarrollo se cerró a las 13:05.
- **Falta:**
  - que el usuario entre a ZSilent 4.0: hasta entonces su nodo de pre-login
    sirve la cadena pero sin Stratum, y XMRig espera;
  - el aviso en Discord: no hay bot ni conector, lo pega el usuario;
  - los papers (whitepaper y litepaper 2.4) todavía describen la v1.
- **Después:** Android, RPC para pools y la revisión externa. Pendientes
  menores:
  - el Stratum cuenta también los bloques laterales;
  - el paquete de Tor sigue en el MSI aunque la v2 no lo usa.

## 28/10, noche (5): ZSilent v2 (Claude, ramas `quantic-v2-zsilent` y `quantic-v2`)

Quinto punto del orden acordado. **Nada pusheado, armado ni instalado:** la
3.0.5 sigue siendo la publicada y la que usa la PC.

- **Nodo** (rama `quantic-v2-zsilent`, sobre la de cortas):
  - `06d897c2`: `zoka quantic-pq-wallet`, con las 13 acciones y el JSON de la
    `quantic-wallet` v1 sobre wallets v2:
    - la frase BIP-39 es la de la semilla;
    - la copia `<dir>/wallet-mirror` se sincroniza con el nodo local por
      `--peer`;
    - la API va por `--api`, con un cliente HTTP mínimo de loopback.
  - `c5ce56b7`: `/blocks/<altura>/summary`, `tip_timestamp` y
    `peers.best_height` en `/status`, y `?local=1` en `/address`.
- **ZSilent** (rama `quantic-v2` de zsilent-core, sobre la 3.0.5):
  - `5bda4c7`: los servicios. `NodeController` arranca `quantic-pq-node
    serve`; la identidad se cifra con un secreto al azar guardado al lado.
    Si encuentra las claves de la autoridad en la carpeta de la v1, pasa
    `--authority`. `WalletService` usa `quantic-pq-wallet` y `ZokaRpcClient`
    lee `/status`. Ya no levanta Tor, no consulta `mine-status` ni sincroniza
    por RPC.
  - `d94197b`: las pantallas.
    - Setup propio por red v2, así que el onboarding se hace de nuevo y ahí se
      crea la wallet del equipo.
    - Sin la sección Mining: la testnet v2 se mina con XMRig por el Stratum.
    - Tablero, Node y Privacy sin Tor. Node muestra checkpoint, directorio e
      identidad del nodo.
    - El tablero contaba los pares por la lista de la v1 y con el nodo v2 iba
      a dar siempre "sin pares".
  - `3616def`: el armado prueba el binario con `quantic-pq-wallet` y pide
    `--features randomx,quantic-pq`. La versión queda propuesta como 4.0.0.
    El paquete de Tor sigue (no se usa).
- **Validado:**
  - el comando con un nodo regtest real: pago de 12,5 ZKA a una corta (127 s
    sin optimizar), un segundo envío con el primero pendiente se rechaza, y el
    historial de los dos lados queda bien;
  - pruebas de Rust del comando y de la API;
  - ZSilent: 80 pruebas, `V2NodeWalletE2ETest` con un nodo regtest real
    (opcional, con `ZSILENT_V2_E2E_ZOKA`) y capturas de las pantallas v2
    (`QuanticV2ScreensScreenshotsTest`).
- **Falta para el lanzamiento:**
  - el génesis v2 (wallet del equipo con ZSilent 4.0, identidad de la PC al
    manifiesto, el ancla de ASERT);
  - armar el MSI con el nodo `--features randomx,quantic-pq`;
  - decidir si el paquete de Tor se queda;
  - probar la app entera en la PC (ventana real).

## 28/10, noche (4): direcciones cortas v2 (Claude, rama `quantic-v2-direcciones-cortas`)

Cuarto punto del orden acordado. Rama sobre `quantic-v2-checkpoints`,
**pusheada** con el OK del usuario (`96327230` en GitHub y GitLab).

- **Formato** (`c33ee7ae`): `ShortAddress` en `zoka-pq-pool`. La corta v2 ya
  se calculaba (`zqtest2…`, 57 caracteres) pero no se podía leer. El formato
  no cambió y queda fijado en un vector de prueba.
- **Directorio** (`src/quantic/stark_directory.rs`): la opción A de la v1
  sobre las sesiones de la v2, que no tienen gossip.
  - Cada sesión saliente empuja las direcciones publicadas en el nodo (otra
    vez cada 6 h) y las que guardó hace poco.
  - Cada sesión le pregunta al par por las cortas que alguien espera.
  - Sello Argon2id de la v1 con sal propia.
  - Topes: un millón de registros, 30 por minuto por conexión, 20 por minuto
    empujados por sesión y búsquedas de 20 s.
- **API:** `POST /address` y `GET /address/<corta>`, con el mismo JSON que la
  v1; `/status` cuenta el directorio.
- **Stratum:** acepta la corta si el nodo la conoce.
- **CLI:** `quantic-pq address --short`; `send --to <corta>` avisa que hay que
  resolverla.
- **Validado:**
  - 9 pruebas nuevas (formato, directorio, red con el tope por conexión, API
    y nodo) y 2 ampliadas (Stratum y CLI); las 5 del directorio v1 siguen
    pasando con el sello separado;
  - la suite de nodo, cadena y red: 250 de 251. La que falla,
    `pq_pinned_localhost_sync_private_tx_…`, se pasa del límite de 120 s
    armando una prueba STARK sin optimizar. Falla igual en `e57d4701`, sin los
    cambios de esta etapa: es la velocidad de la PC esa noche, no una
    regresión. Antes pasaba.
  - tres procesos reales en `target/v2-cortas-2026-10-28/run1`:
    - publicar en B tardó 0,6 s y B se la empujó a A;
    - C, que nunca la había visto, la resolvió a través de A en 0,2 s;
    - una corta desconocida da 404 a los 20 s en C, y al instante en A (sin
      pares salientes);
    - publicar una corta da 422, y `send --to <corta>` avisa;
    - stderr vacíos.
- **Ojo con los worktrees:** compartir `CARGO_TARGET_DIR` entre dos copias del
  repo hace que cargo reutilice artefactos de la otra copia (los crates locales
  se identifican por la ruta relativa). Pasó con la comparación contra
  `e57d4701`: se arregló con `cargo clean -p zoka-pq-pool --profile dev`.
- **ZSilent:** sigue la 3.0.5, que es de la v1. La actualización hace falta
  recién con el lanzamiento de la v2, porque la 3.0.5 no habla con el nodo v2.
- **Sigue:** ZSilent v2 y el génesis.

## 28/10, noche (3): checkpoints de la PC en el nodo STARK (Claude, rama `quantic-v2-checkpoints`)

Tercer punto del orden acordado (decisión 3 del usuario). Rama sobre
`quantic-v2-red-publica`, **pusheada** con el OK del usuario (`e57d4701` en
GitHub y GitLab).

- **Claves:** la testnet v2 usa las mismas claves de autoridad que la v1 (la
  misma PC, y cada firma lleva el chain id). El usuario no tiene que crear
  nada; lo confirmó el 28/10.
- **Nodo:** `serve --authority DIR`. `DIR` es la carpeta de la red de la v1
  donde están `checkpoint-authority.key` y `checkpoint-authority-mldsa.key`.
  Se firma con las reglas de la v1: cada 30 bloques, 6 por debajo de la punta,
  con algún par en sesión y ninguno adelante.
- **Red y API:**
  - `HEAD` informa la altura del checkpoint, y cada sesión trae o entrega el
    más nuevo antes de mover bloques;
  - `GET`/`POST /checkpoint`;
  - `checkpoint-sign` para corregir a mano;
  - cuarentena de la minería mientras la cadena contradice el checkpoint.
- **Validado:** 30 pruebas, la suite de 256 y tres procesos reales con claves
  descartables de `zoka checkpoint-keygen`:
  - la autoridad sola no firmó;
  - con un par firmó el 4;
  - un nodo con una cadena propia de 30 pasó a la de 11 de la autoridad;
  - una firma a mano llegó a los tres;
  - una falsificada dio 422.

  Carpetas en `target/v2-checkpoint-2026-10-28/` (la `run2` es la buena; la
  primera se cortó por un error del guion de prueba).
- **Lanzamiento:** ZSilent v2 tiene que arrancar el nodo de la PC con
  `--authority` apuntando a esa carpeta.
- **Sigue:** direcciones cortas v2, ZSilent v2 y el génesis.

## 28/10, noche (2): red pública v2 (Claude, rama `quantic-v2-red-publica`)

Segundo punto del orden acordado. La rama `quantic-v2-formato-final` quedó
pusheada a GitHub y GitLab con el OK del usuario (`92a2de9e`, verificada con
`ls-remote`). La nueva, sobre ella, también quedó pusheada con su OK
(`b503f158`, verificada en los dos remotos):

- **Arreglo de TLS que también toca a la v1** (`610cf464`): el proveedor de
  rustls que exigía solo X25519MLKEM768 también lo usan el HTTPS de los relays
  y pkarr de iroh, y los relays de n0 cortaban con `HandshakeFailure`. Un nodo
  con `--relay N0` nunca quedaba en línea, y lo mismo pasaba con el
  `NodePreset` de la v1 en Quantic. Ahora se ofrece el híbrido primero y un
  hook `after_handshake` cierra toda conexión de iroh que no lo negoció.
  Dependencia nueva `noq-proto` 1.3 (ya estaba en el lock).
- **Red pública** (`f367e385`):
  - descriptor v3 que fija la huella de 32 bytes de la clave ML-DSA;
  - manifiesto `docs/quantic-testnet-v2-network.v1.json` embebido (pre-génesis,
    sin semillas);
  - peticiones `PEERS`/`ANNOUNCE`: cada nodo se anuncia solo a sí mismo, y antes
    de compartir un par lo llama y lo autentica;
  - tabla de pares acotada y guardada en la base;
  - relay N0 por omisión en la testnet;
  - `quantic-pq-node identity`, que imprime la entrada de semilla;
  - `sync` y `broadcast` usan las semillas;
  - `/status` informa los pares.
- **Validado:**
  - 19 pruebas de red, 238 de `quantic::`, `state::chain`, `economics` y
    `network::`, y `cargo check` sin features y con `quantic`;
  - tres procesos reales en loopback: C conocía solo a B (vía manifiesto),
    encontró a A y siguió su cadena;
  - dos procesos por los relays reales de n0: la semilla fijada solo por
    EndpointId y huella, A en línea en 1 s, y B siguió a A.

  Carpetas de prueba en `target/v2-red-2026-10-28/` (la `n0` es la corrida que
  falló antes del arreglo).
- **Para el lanzamiento:**
  1. el usuario crea la identidad de la PC con `quantic-pq-node identity
     --network testnet`;
  2. su `seed_entry` pasa al manifiesto, que es consenso de red, no de cadena;
  3. el binario se vuelve a compilar.
- **Queda abierto:**
  - no hay Tor: QUIC va sobre UDP, así que haría falta otro transporte (TCP);
  - las semillas no tienen un modo solo de presentación de pares: con muchos
    nodos se llenan;
  - para la mainnet hace falta un relay propio;
  - un atacante con muchos nodos reales puede ocupar las 64 sesiones entrantes.
- **Sigue:** checkpoints de la PC en el nodo STARK, direcciones cortas v2,
  ZSilent v2 y el génesis.

## 28/10, noche: formato final de la v2 (Claude, rama `quantic-v2-formato-final`)

Primer punto del orden acordado («estado por diferencias»). Al revisarlo
aparecieron otros dos problemas de formato, también de consenso, y se
resolvieron juntos antes del génesis v2. Especificación en
[QUANTIC-V2-FORMATO.md](docs/QUANTIC-V2-FORMATO.md). Pusheada el 28/10 con el
OK del usuario.

- **Faerie Gold** (seguridad): `rho` lo elegía el emisor y dos notas podían
  compartir nullifier. Ahora `rho` sale del primer nullifier (AIR) o de la
  altura del coinbase; `rcm` oculta. `b9399b83`.
- **Coinbase sin prueba:** de ~947 KB a menos de 4 KB por bloque. `b9399b83`.
- **Estado compacto:** registro de cientos de bytes por bloque y `state_root`
  de tamaño constante; ramas laterales validadas con el índice canónico de
  nullifiers. `9f49e743`.
- Validado: 14 + 105 pruebas en release, pago por CLI de punta a punta y XMRig
  real (25/0 aciertos, 5 bloques, base de 119 KB contra 7,4 MB).
- **Sigue**, en el orden acordado: red pública v2 (semillas, manifiesto,
  descubrimiento de pares y Tor/relay), checkpoints de la PC en el nodo STARK,
  direcciones cortas v2, ZSilent v2 y después el génesis. Queda abierto el
  tamaño de las pruebas de pago (1,9 MB).

## 28/10, tarde: testnet v2 con RandomX y XMRig (Claude, rama `quantic-stark-xmrig`)

El usuario pidió continuar lo que Codex dejó a medio hacer. Codex había
commiteado el núcleo STARK (`ed6304d5`) y anunciado como siguiente paso
«conectar la minería de XMRig al nuevo formato privado»; no llegó a escribirlo.
Hecho, en la rama `quantic-stark-xmrig` (sobre `quantic-protocolo-postcuantico`),
nada pusheado ni instalado:

- **Perfiles STARK**: además de la regtest v2 existe `zoka-quantic-testnet-v2`
  (RandomX, la ASERT de la v1, direcciones `zqtest2`). `--network
  regtest|testnet` en `quantic-pq` y `quantic-pq-node` (regtest por omisión).
  Base, wallet, identidad y descriptor guardan su cadena. **Sin génesis fijo**
  (`QUANTIC_PQ_TESTNET_GENESIS = None`): fijarlo es la decisión de lanzamiento.
- **Stratum para XMRig** sobre plantillas STARK: `quantic-pq-node serve
  --network testnet --stratum HOST:PUERTO`. El login es la dirección v2
  completa. El Stratum de la v1 quedó con un `Backend` y sin cambios de uso.
- **Probado con XMRig 6.25.0 real** (el de `Downloads`), nodo local aislado,
  wallet descartable: 20 aciertos aceptados, 0 rechazados, 4 bloques. Las
  corridas anteriores destaparon tres fallas del Stratum compartido (también
  de la v1), arregladas: plantillas rehechas sin parar con la cadena vacía,
  trabajo repetido que hacía reconectar a XMRig y 37 s de espera del primer
  acierto. Detalle en
  [IMPLEMENTACION-QUANTIC-V2-2026-10-28.md](docs/IMPLEMENTACION-QUANTIC-V2-2026-10-28.md)
  y uso en [QUANTIC-STARK-USO.md](docs/QUANTIC-STARK-USO.md).
- Defender no cortó XMRig en estas corridas: el permiso del ThreatID 2147978346
  que puso el usuario el 23/10 parece seguir activo (lo quita él).
- **Decisiones del usuario (28/10, respuesta «sí» a las cuatro):**
  1. la testnet v2 **reemplaza** a la v1 apenas esté lista (no conviven);
  2. el génesis v2 se crea cuando estén la base de estado y la red pública
     (la wallet del equipo v2 la crea él en ese momento; recordar el ancla de
     ASERT en la hora del génesis);
  3. **la PC sigue como nodo autoritario** en la v2: hay que llevar los
     checkpoints firmados al nodo STARK;
  4. push de `quantic-protocolo-postcuantico` y `quantic-stark-xmrig` a GitHub
     y GitLab: **hecho** (`ed6304d5` y `2b95fb53`, verificados con `ls-remote`).
- **Falta**, en este orden sugerido: almacenamiento por diferencias en vez de
  copiar el ledger entero por bloque (cambia `state_root`: es consenso y hay
  que hacerlo antes del génesis), ZSilent sobre el flujo v2, semillas y
  manifiesto de la v2, RPC para pools, y Android.

## 28/10: implementación del pool privado STARK (validación local completada)

El usuario autorizó complementar el desarrollo de Claude e implementar la
blockchain privada con refuerzo postcuántico. Rama de trabajo:
`quantic-protocolo-postcuantico`. No se migraron la testnet pública ni wallets
instaladas. El código nuevo usa `quantic-pq` y la cadena local separada
`zoka-quantic-regtest-v2`.

- `crates/zoka-pq-pool` ahora contiene una transferencia real: propietario
  derivado mediante hash, notas y árbol Poseidon2, nullifiers, montos de 64
  bits, balance dentro del AIR, pruebas STARK con ocultamiento, cifrado X-Wing,
  direcciones v2 y ledger con emisión, comisiones y maduración. El antiguo
  benchmark de permutaciones continúa siendo solo un benchmark.
- `src/quantic/stark.rs` integra este formato en el validador real del nodo,
  PoW, RocksDB y reorganizaciones. La primera prueba completa pasó (22,5 s).
  Tiene mempool acotado. El estado completo por bloque aún es una limitación
  de escalabilidad; no presentarlo como almacenamiento listo para mainnet.
- CLI `quantic-pq`: wallet cifrada/restauración, escaneo, reservas, transferencias
  por archivo, minado e importación/exportación de bloques. Se conserva el
  flujo anterior `pq-regtest` (send mina automáticamente); son schemas de
  wallet distintos. Se endureció ese flujo contra XOR, notas duplicadas y DB
  de otra red. Sus nuevas regresiones pasaron en la corrida final.
- `stark_api.rs`, `stark_net.rs` y `stark_node.rs` están implementados y
  conectados al comando `quantic-pq-node`. API loopback, transporte iroh con
  TLS híbrido obligatorio y autenticación ML-DSA fijada por descriptor y
  ligada al exporter TLS; sincronización de forks por lotes, retransmisión de
  pagos, minero y copias locales para la wallet. Comparten límites de trabajo
  y bloqueo de estado; la cancelación no libera un verificador todavía activo.
  Guía: [QUANTIC-STARK-USO.md](docs/QUANTIC-STARK-USO.md).
- TLS iroh de Quantic exige intercambio híbrido. Snapshots/importaciones
  exigen firmas híbridas de claves confiables; la autoridad compilada sirve
  como fallback cuando el manifiesto no lista firmantes. Servir bloques
  sigue disponible sin clave de autoridad. El protocolo v2 añade autenticación
  ML-DSA al servidor; la identidad iroh subyacente sigue siendo Ed25519.
- La revisión del transporte corrigió un estancamiento al enviar forks largos
  con pagos pendientes. También se corrigió `wallet/storage.rs`: un archivo
  que comienza con `ZOKAENC1` pero está truncado ya no cae al descifrado XOR.
  Los formatos antiguos sin ese encabezado conservan su compatibilidad.
- Medidas verificadas: prueba 1→2 de 1.868.731 bytes (~6,4 s); forma máxima
  8→8 de 8.341.990 bytes (87,15 s mientras había otras compilaciones).
  Pasaron las pruebas del circuito, inflación con hashes recalculados,
  ciphertexts, doble gasto, maduración y rechazo de bytes sobrantes. El
  estimador clásico arroja 128 bits bajo una hipótesis explícita de hash;
  no acredita 128 bits cuánticos ni reemplaza revisión externa.
- `cargo check --offline --manifest-path crates/zoka-pq-pool/Cargo.toml --lib`
  pasó para `armv7-linux-androideabi` y `aarch64-linux-android`. No se midió
  todavía el prover en un teléfono ni se sustituyeron los JNI existentes.
- Informe de revisión asistida:
  [REVISION-STARK-CODIGO-2026-10-25.md](docs/REVISION-STARK-CODIGO-2026-10-25.md).
  No es auditoría externa ni certificación de resistencia cuántica.
- El 28/10 pasaron 10 pruebas del pool (99,83 s), 48 de Quantic incluidas
  API/transporte/nodo/CLI y compatibilidad anterior (354,66 s), 42 de TLS,
  snapshots y almacenamiento de wallets (27,55 s), 15 del CLI de snapshots
  (0,12 s) y 3 de configuración de red. Pasaron también `cargo check --offline
  --features quantic-pq --tests`, `cargo check --offline --lib` sin la feature,
  y la compilación release de `zoka` con `quantic-pq`.
- La prueba posterior con dos procesos reales detectó un fallo específico de
  Windows al enviar: bloquear el archivo de wallet impedía leerlo por otro
  handle del mismo proceso. Se corrigió a un sidecar `.stark-lock` persistente
  con ruta canónica y se añadió regresión. Pasaron las cuatro pruebas del CLI
  de wallet (10,83 s), el rebuild y el envío real de 1 ZKA entre dos nodos:
  ambos llegaron a altura 3 y el receptor vio 1.000.000 átomos disponibles.
  Las bases conservan el mismo hash al reabrirse por sus rutas absolutas.
  Los procesos temporales quedaron detenidos. Junto con la regresión de rutas
  descrita abajo, son 120 pruebas distintas.
- La comprobación posterior detectó que `main` cambiaba siempre a la carpeta
  general de Zoka, también para las rutas explícitas v2. Se corrigió para que
  estos comandos conserven el directorio del invocador y omitan la migración
  automática y lectura de configuración pública. Pasaron su regresión, el
  rebuild final y la repetición con dos procesos y rutas relativas: pago de
  1 ZKA, altura 3 y mismo hash al reabrir ambas bases. No hubo pérdida de la
  cadena original. La prueba de
  rutas anterior creó dos bases vacías bajo la carpeta general de Zoka, en
  `target/quantic-validation-2026-10-28/smoke-a9e03ac149664bb0bbbc59aba52daf7e`;
  su eliminación fue rechazada por la revisión automática ("blocked by policy")
  y se conservaron. No reintentar con otra vía ni tocar las bases del usuario.
- La testnet existente y XMRig siguen en el protocolo v1. Regtest v2 usa el
  PoW CPU de pruebas; esas pruebas no validan RandomX/XMRig v2 ni la GUI móvil.
- Informe del código y sus comprobaciones:
  [IMPLEMENTACION-QUANTIC-V2-2026-10-28.md](docs/IMPLEMENTACION-QUANTIC-V2-2026-10-28.md).

Esta entrada sustituye el estado de implementación de la revisión del 24/10
que figura abajo. No afirmar que el proyecto está completo ni que mainnet está
habilitada basándose solamente en estas pruebas locales.

## 24/10: revisión del desarrollo postcuántico y XMRig

Pedido del usuario: revisar qué queda del desarrollo de Quantic teniendo en
cuenta lo construido por Claude, la versión 3.0.5 y el uso de XMRig.
Informe actual: [docs/REVISION-quantic-2026-10-24.md](docs/REVISION-quantic-2026-10-24.md).

- **Versiones comprobadas:** ZSilent 3.0.5 incluye el nodo 3.0.4. Coinciden
  Cargo.toml, notas del MSI y metadatos del ejecutable instalado. CLI: 3.0.4.
- **Ya implementado:** X-Wing en las notas, firmas híbridas de checkpoints y
  snapshots, TLS híbrido preferido en iroh (con compatibilidad clásica),
  cadena/testnet Quantic, ZSilent y PoW/Stratum compatible con XMRig.
- **Pendiente principal:** el pool STARK de transferencias. El ejecutable
  `crates/zoka-pq-pool` mide permutaciones Poseidon2; todavía no tiene las
  restricciones de una transferencia ni está integrado al consenso. El gasto
  actual usa Ironwood/Orchard, Halo 2 y firmas de curva.
- **Orden:** arquitectura de pruebas/agregación, transferencia completa,
  integración al nodo y wallets, rendimiento y pruebas adversariales,
  nueva testnet y revisión externa antes de mainnet.
- **Otros pendientes:** transporte libp2p clásico, interfaz Android,
  RPC para pools externos y prueba de xmrig-proxy; P2Pool propio es posterior.
  Revisar el ancla de ASERT y propagación/tamaños antes de mainnet.
- **Detalle verificado de snapshots:** las firmas híbridas existen, pero el
  bootstrap iroh solo exige firma con `--iroh-require-signed-snapshot`; por
  omisión confía en la identidad iroh del seed y revalida los bloques. El
  manifiesto de testnet tiene `snapshot_signers: []`. Falta cerrar esta política
  para una autenticación postcuántica obligatoria de la sincronización.
- **Validación de esta revisión:** 82 pruebas pasadas (14 primitivas, 40
  biblioteca Quantic, 26 nodo y 2 de extremo a extremo). Benchmark STARK: 199,6
  KiB, 205,7 ms de prueba y 14,5 ms de verificación; son permutaciones aisladas,
  no una transferencia completa. No se ejecutó minería nueva ni un teléfono.
- Las listas antiguas de abajo conservan tareas ya terminadas. Para saber qué
  falta, usar el informe de esta revisión y comprobar el código; no contar
  casillas abiertas como porcentaje de avance.

## 24/10: corrección de identidad confirmada por el usuario

La marca correcta es **ZOKA QUANTIC**, no Zoka Network. El usuario aclaró que
la idea central es una blockchain con **conocimiento cero y privacidad por
defecto, con implementación postcuántica**. Esta aclaración sustituye la
identidad descrita en la entrada anterior del plan web, conservada abajo
como historial.

Publicado en **https://zoka.network**: portada «Zero knowledge by default»,
marca, navegación, pie, metadatos, Tech y papers coherentes. Se explica la
criptografía postcuántica ya implementada (cifrado de notas X-Wing y firmas
híbridas de snapshots/checkpoints), manteniendo explícito que Halo 2 sigue
siendo clásico y el nuevo pool totalmente postcuántico está en desarrollo.
Redes: **Zoka Quantic Testnet** y **Zoka Quantic Mainnet**.

Whitepaper y litepaper **2.4**, regenerados conservando sus URLs. Código web
`0805a5c`, en `main` de GitHub y GitLab. Cloudflare Pages de producción:
`https://2d2c13ab.zokaweb.pages.dev`.

Validado: TypeScript, 7 pruebas, lint sin errores (7 avisos preexistentes),
build, portada en 320/390/1440 px y primeras páginas de ambos PDFs. En la web
pública se comprobaron marca, mensaje, Tech, papers y enlace a ZokaScan;
JS, CSS y PDFs coinciden por SHA-256 con el build local. Sin errores JS.

## 24/10: terminado el plan web `ethereal-booping-wall`

El usuario pidió retomar el trabajo web que Claude dejó en el plan
`ethereal-booping-wall.md`. Implementado y publicado en **https://zoka.network**:

- Marca **Zoka Network**, privacidad de punta a punta como mensaje principal,
  Zenith como base del proyecto, Quantic Testnet en vivo y Zoka Mainnet
  «coming soon», sin cambiar IDs ni reglas del nodo.
- `Downloads/pcquantic.mp4` reemplaza el globo: copia H.264 de 1.804.417 bytes,
  sin pista de audio, con poster, pausa y respeto por movimiento reducido.
  El original no se modificó.
- Home: descarga directa de ZSilent 3.0.5 y enlace a **https://zokascan.cc**.
  El usuario confirmó esa URL y que el explorador se reparará después; la web
  avisa que está temporalmente fuera de servicio. No se tocó el explorador.
- Download contiene las tres opciones (ZSilent, CLI, Android en desarrollo),
  con enlaces a sus guías. Docs tiene **12 guías** y los papers. Las URLs
  `/#docs/cli-miner`, etc., soportan compartir, recargar, atrás y adelante.
- Tech explica las capas de privacidad, Halo 2/Ironwood, X-Wing, direcciones,
  libp2p/Gossipsub/Kademlia, Tor, iroh, minería y checkpoints, con límites y
  estado real de implementación.
- Whitepaper y litepaper **2.3**, 17 y 5 páginas, regenerados y revisados;
  se conservaron las URLs de PDF existentes. Fuentes tipográficas locales,
  con sus licencias: sin solicitudes a Google Fonts al cargar el sitio.

**Código:** `zokaweb` commit `4e7f90c`, en `main` de GitHub y GitLab;
rama de trabajo conservada `web-privacidad-24oct`. **Cloudflare Pages:**
despliegue de producción `https://99a2b7f6.zokaweb.pages.dev`.

**Validación:** 7 pruebas Vitest; TypeScript de app y configuración; lint sin
errores (7 avisos Fast Refresh preexistentes); build de producción. Navegador
en 320, 390, 768 y 1440 px: 12 guías, historial, foco, video y movimiento
reducido; sin errores JS ni peticiones externas. Descargas GitHub HTTP 200.
Tras publicar, HTML, JS, CSS, video, poster, fuentes y PDFs en zoka.network
coinciden con el build local; comprobado también en navegador público.

## Punto de partida

**23/10, lo último: génesis nuevo de la testnet, XMRig mina por Stratum y
ZSilent 3.0.2 armada (sin instalar ni publicar).** Ramas: nodo
`quantic-direcciones-cortas`, ZSilent `direcciones-cortas`. Nada pusheado.

- **Direcciones cortas** (`ee759eb1`, `d85851b1`, `a9d3c00e`, `405f1c71`): 57
  caracteres en la testnet (`zqtest1…`), con un QR normal. El nodo tiene un
  directorio que resuelve la corta a la completa: se publica con
  `POST /quantic/address` (sello Argon2id) y viaja por gossip. Se paga a una
  corta con `quantic-wallet send --to <corta>`. Diseño en
  `docs/QUANTIC-DIRECCIONES-CORTAS.md`.
- **PoW v2, compatible con XMRig (consenso, `dadb49dd`)**: blob de 76 bytes con
  la forma del de Monero, nonce de 32 bits en la posición 39, extranonce en los
  32 bits de arriba del nonce del encabezado, hash comparado en little endian.
- **Génesis nuevo** `b7d58bf48ea5c2916922f484a138258a35f72b8e3799816ff7011d6c6a0c2aca`
  (2026-10-23 01:38 UTC, `97d2a528`), con el premine en `zsilent-2` y la misma
  autoridad (Ed25519 + ML-DSA-65). El del 22/10 (`1d204e81…`) se retiró
  (`1608c129`). **La PC todavía corre 3.0.1 con el génesis viejo:** antes de
  instalar la 3.0.2 hay que apartar (renombrar)
  `%APPDATA%\Zoka\.zoka\quantic-testnet\blockchain.db`, con aprobación del
  usuario y ZSilent cerrado por él.
- **Stratum en el nodo** (`--stratum host:puerto`; `acbe6cbe`, `996ca646`):
  `login` con dirección completa, corta o cualquier otra cosa (cobra la de
  `--quantic-mine-to`); un login que parece dirección de otra red recibe un
  error. Tope de 256 conexiones. `GET /quantic/stratum`. **Probado con XMRig
  6.26.0 oficial** contra una testnet local aislada: 6 aciertos aceptados, 0
  rechazados, 3 bloques, y la wallet cobró por su corta. Diseño y camino a los
  pools grandes en `docs/QUANTIC-POOL-DE-MINERIA.md` (`50f7d734`). Lo siguiente
  sería el RPC `get_block_template`/`submit_block` para el software de pools de
  Monero. El P2Pool de Monero no sirve tal cual; uno propio, más adelante.
- **Defender y XMRig:** lo mata por comportamiento (`Trojan:Win64/HashvaultMiner.A`,
  ThreatID 2147978346), y una exclusión de carpeta no alcanza. Para la prueba,
  el usuario corrió como administrador un permiso temporal. Quitarlo exige
  `Remove-MpPreference -ThreatIDDefaultAction_Ids 2147978346
  -ThreatIDDefaultAction_Actions 6`: sin la acción da el error 0x57, y así le
  quedó puesto hasta que lo saque él. XMRig se borró. Mientras se tocaba
  Defender, el clasificador de permisos bloqueó incluso lecturas: elevar o
  tocar Defender lo hace el usuario.
- **ZSilent** (`ffe9c8a`, `7095b92`, `b805d49`): se fue todo el minado en equipo
  de Zenith (pantalla, Ajustes Experimental, estado, RPC, modelos y pruebas), y
  hay una sección nueva, **Stratum**. Enciende el Stratum del nodo (solo esta PC
  o también la red local, puerto 3333), reinicia el nodo con `--stratum` y
  muestra mineros, aciertos y bloques. Arma la línea de XMRig con la corta
  publicada y avisa del antivirus. 76 pruebas.
- **Defender y `zoka.exe`:** el 02/07/2026 Defender marcó un `zoka.exe` bajado de
  GitHub como `PUA:Win32/Caypnamer.A!ml`. Esa detección (274600) está
  **permitida en la PC**, así que la PC no sirve para probar si un instalador
  nuevo salta: hay que probarlo en un Windows limpio. Desde la 3.0.2, `zoka.exe`
  y `zokahd.exe` llevan sus datos de versión (`build.rs`, `3b1d3cea`), y
  `prepare-release.ps1` no empaqueta un binario sin ellos. Lo que de verdad lo
  resuelve es la firma de código y avisarle a Microsoft del falso positivo en
  cada versión: decisión y cuenta del usuario.
- **3.0.2:** suite con `quantic` 1000+92+2+3 (antes del `build.rs`). MSI
  `zsilent-core/build/release/3.0.2/ZSilent Core-3.0.2.msi`, SHA-256
  `aa3f30818544171ef5638a27092b8907434b0ed6faf4d9699bb967bc1fdd7868`. El
  `zoka.exe` de adentro (`b25eedba…`, v3.0.2) es el compilado; se verificó
  extrayendo el MSI con `msiexec /a`. El ensayo de la testnet con ese binario
  (`ensayo-testnet.sh`, autoridad descartable) pasó entero: génesis embebido,
  segundo nodo con el suministro exacto (5.000.000 + 10 × 12 ZKA) y checkpoint
  híbrido aceptado.
- **Instalada en la PC** (lo pidió el usuario; `msiexec /passive`, código 0, a
  las 11:34) en `%LOCALAPPDATA%\ZSilent Core`, con `zoka.exe` `b25eedba…`
  v3.0.2. Antes se apartaron, renombrándolos con el sufijo
  `.genesis-1d204e81-20261023` y sin borrar nada, tres archivos de
  `%APPDATA%\Zoka\.zoka\quantic-testnet`:
  - `blockchain.db`, la cadena vieja;
  - `quantic-wallets\zsilent-2.scan`, el escaneo de la wallet; sin apartarlo,
    mientras la cadena nueva fuera más baja que la vieja mostraba el saldo viejo
    como «nodo atrasado»;
  - `user_mining_state.json`.

  Las wallets, las claves de autoridad, la onion y la identidad de iroh no se
  tocaron. La dirección corta hay que volver a publicarla: el directorio vivía
  en la base vieja.
- **3.0.3, instalada en la PC a las 12:40** (lo pidió el usuario; sin apartar
  nada, misma cadena). MSI `zsilent-core/build/release/3.0.3/`, SHA-256
  `61f44c568abbd2268a993e570a5b4caeb9afce76e5aa6e938f8c08d301acfe16`; `zoka.exe`
  `c5d1b4d5…`. Se verificó extrayendo el MSI: el logo nuevo, el código nuevo y el
  binario. Trae:
  - **Peerstore propio de cada red de Quantic** (`815f5e67`):
    `peers-quantic-testnet.json`. El `~/.zoka/peers.json` era uno solo, y el
    nodo de la testnet marcaba en cada arranque las 17 direcciones de Zenith
    (puerto 28080) de la 2.1.
  - **El Stratum dice si arranca o falló** (`eeb2fb2f`, ZSilent `a537054`): se
    abre al final del arranque del nodo, cerca de un minuto después. La 3.0.2
    mostraba «NOT LISTENING» en rojo durante ese minuto; ahora dice
    «STARTING».
  - **Logo nuevo de Zoka Quantic** (ZSilent `f198355`): la Z con una Q rosa
    pixelada, desde `Downloads\zokaquantic.jpg`, con
    `scripts/prepare-brand-assets.ps1`.
  - **La dirección corta en toda la app** (ZSilent `88fcc1b`,
    `ZSilentUiState.shownAddress`): Wallet, barra lateral, Mining, Ajustes,
    Dashboard y login. La completa sigue en Receive, y la tarjeta de Wallet
    avisa «not published».

  La corta de `zsilent-2` es
  `zqtest15MnwVgjVzRRABprTurtK7rB2tUw4V7FUyfi66Mt1zJPXuv76Bp`; hay que
  publicarla desde Receive.
- **XMRig en la PC del usuario** (12:54): 6.25.0 oficial, versión gcc, hash
  verificado contra el `SHA256SUMS` del release. Minó por la red local
  (`192.168.1.13:3333`) contra ZSilent 3.0.3, con la corta de `zsilent-2`: 7
  bloques en minutos y 0 aciertos rechazados. Con la dificultad en el piso, cada
  acierto es un bloque. El usuario había bajado primero la versión arm64, que en
  su i5-6400 da «No coincide el tipo de máquina».
- **ZSilent 3.0.4, instalada en la PC** (lo pidió el usuario; código 0;
  `7c51a75`; MSI `zsilent-core/build/release/3.0.4/`,
  SHA-256 `d357baae3a5e6173fdb7074e85a4dacf748021afa40da05e45303c5d78862059`;
  nodo 3.0.3 sin cambios). Trae tres decisiones del usuario, anotadas en
  `docs/QUANTIC-DIRECCIONES-CORTAS.md`:
  - solo la corta a la vista;
  - la corta se publica sola («sola y siempre»);
  - un QR estándar, porque el de puntos redondos les costaba a los teléfonos.
    `QrCodeReadBackTest` lo vuelve a leer con un lector de QR.

  78 pruebas.
- **Minero CLI y web nueva, listos sin publicar** (pedido del usuario: ZSilent
  como wallet oficial, un minero CLI con guía para GitHub y ZKAPriv «en
  desarrollo»; las tres secciones del menú al día y sin Zenith).
  - **Minero CLI 3.0.4** (`packaging/cli-miner/`, se arma con
    `scripts/build-cli-miner.ps1` y `scripts/build-cli-miner-linux.sh`):
    - un script por sistema (create-wallet, restore-wallet, address, mine,
      balance, xmrig-node, status), accesos `.cmd` y guías en inglés y
      castellano;
    - en Windows trae Tor 0.4.9.14 con su licencia; en Linux usa el de la
      distribución;
    - todo queda en `./data`, con puertos propios: API 3180, P2P 28181 solo
      local, Tor 19150 y Stratum 3333 (se cambia con -StratumPort o
      --stratum-port).

    Salida en `target/cli-miner/3.0.4/`:
    - zip de Windows `9abd1773…158f`;
    - tar.gz de Linux `edee7995…84ca`;
    - `SHA256SUMS.txt` y `RELEASE-NOTES.md`.
  - **Linux sobre Debian 11:** compilado en Debian 13 pedía glibc 2.39 (no
    arrancaba en Ubuntu 22.04). `scripts/build-linux-bullseye.sh` compila dentro
    de un Debian 11 aislado en WSL (`/srv/zoka-bullseye`, Rust 1.97.1). El
    binario pide glibc 2.30 y anda de Ubuntu 20.04 y Debian 11 en adelante.
    Probado adentro de ese Debian 11 con su Tor 0.4.5.16: bajó la cadena, se
    conectó a la PC por Tor y abrió el Stratum. El de Windows se probó igual en
    una carpeta limpia.
  - **Errores del nodo que encontró esa prueba**, arreglados en `8139bb61`:
    - sin `--network-config`, el nodo no cargaba ninguna semilla de libp2p
      (bajaba por iroh, pero quedaba con `peers=0` y no publicaba bloques). Ahora
      usa el manifiesto del binario;
    - en Quantic `auto_mine` es siempre falso (`sync_only`), así que un minero
      CLI **minaba sin pares**, como en la 1.6.2. Ahora `--quantic-mine-to`
      cuenta como minar para esa regla;
    - la consola mostraba la dirección completa y el aviso «Sync-only» de Zenith.

    ZSilent no tenía los dos primeros, porque pasa `--network-config` y
    `--min-mine-peers 1`. Además: el zip de `Compress-Archive` (PS 5.1) usaba
    barras invertidas, y ahora se arma a mano (`a7f320cf`). Nodo final
    `zoka.exe` `93d9e9f5…`.
  - **CLI con la corta** (`cb565d9f`): `new`, `restore` y `address` muestran la
    corta (`--full` da la completa), `--quantic-mine-to` acepta la corta de una
    wallet propia, y el nodo publica solo la dirección a la que mina.
  - **ZSilent 3.0.5** (con el nodo 3.0.4): MSI
    `zsilent-core/build/release/3.0.5/`, SHA-256
    `7b14f365ae079c4fbc8e816ef2e4a6504e50b35c332c2dc3620d2af1cb161bab`. Se
    verificó extrayéndolo. Tiene `RELEASE-NOTES.md`. **No instalada en la PC**,
    que sigue en la 3.0.4.
  - **Web** (`zokaweb`, rama `web-quantic-23oct`, `eab109b`):
    - Download tiene tres tarjetas: ZSilent, el minero CLI y ZKAPriv;
    - Tech y Docs al día;
    - Zenith sale de todo lo visible, incluso de `index.html` y del pie;
    - `TESTNET_LIVE = true` y descargas en los releases de GitHub
      (`zsilent-core` v3.0.5 y `zokanetwork` v3.0.4): esos tags tienen que
      existir antes de publicar la web.
- **PUBLICADO el 24/10/2026** (el usuario: «ocupate del resto solo»):
  - **ZSilent 3.0.5 instalada en la PC**, con el nodo `93d9e9f5…`.
  - **Código:** `main` de los tres repos (nodo `96a35324`, ZSilent `38baed0`, web
    `7105778`) y las etiquetas `v3.0.4` y `v3.0.5`, en GitHub y GitLab. GitLab
    necesita `GIT_SSH_COMMAND="ssh -i ~/.ssh/gitlab_zoka -o IdentitiesOnly=yes"`.
  - **Releases en GitHub** (con `gh`, cuenta zoka-network):
    - `zoka-network/zokanetwork` v3.0.4, con los dos paquetes del minero CLI;
    - `zoka-network/zsilent-core` v3.0.5, con `ZSilent-Core-3.0.5.msi` (con
      guion: GitHub cambia los espacios por puntos).

    Bajados desde GitHub, los hashes coinciden.
  - **Web publicada** en zoka.network (Cloudflare Pages, proyecto `zokaweb`). Se
    verificó en vivo: textos nuevos, enlaces y whitepaper 2.2.
- **Dificultad y hora del génesis (para la mainnet):** ASERT se ancla en la hora
  del génesis. La testnet se creó a las 01:38 UTC y se empezó a minar a las
  14:41 UTC: quedó unos 300 bloques atrás del ritmo (uno cada 120 s), con la
  dificultad en el piso hasta alcanzarlo (con XMRig, cada acierto era un bloque).
  Para la mainnet: crear el génesis justo al lanzar, o anclar ASERT en el primer
  bloque minado. Eso es consenso: hay que decidirlo antes del génesis.
- **Pendiente, con aprobación:** push, release y `TESTNET_LIVE=true` en la web.
  Antes de publicar, probar el MSI en un Windows limpio.

**22/10, lo último: la Zoka Quantic Testnet está andando en la PC.** Génesis
`1d204e8116f53d74302db20b943b288d141681a9be463f65a43861dd47c2d242` (11:16 UTC,
commit `2fe98fe3`), premine de prueba a la wallet del equipo. **ZSilent 3.0.1**
instalado en la PC (lo pidió el usuario; `msiexec /passive`, código 0; `zoka`
3.0.1 `3bb4ae04…`), minando a `zsilent-2` con Tor y anunciando su onion
(`d5pk3…:28180`): bloque 2 a las 15:32. Paso 8 hecho: un nodo de prueba que solo
podía entrar por la onion sincronizó la cadena. La wallet del equipo es
`zsilent-2` (restaurada con las 24 palabras); la `equipo` original no abría con
la contraseña que recordaba el usuario y quedó apartada como
`equipo.json.olvidada-20261022` (mismas claves). La 3.0.0 no arrancaba el nodo con
Tor: ZSilent busca `--external-addr` en `zoka --help` y la ayuda de Quantic no lo
nombraba. **3.0.1** (commits `6baf31ed` nodo y `fbdac36` ZSilent): ese flag en la
ayuda, `--address` elige la wallet que abre con la contraseña, el tablero de Zenith
no corre en Quantic (escribía un error por segundo), el diagnóstico va a la carpeta
de la red y a cada nodo solo le llegan los pares manuales de su puerto. Suite
990+92+2+3. MSI `zsilent-core/build/release/3.0.1/`, SHA-256
`8b8facc01ab0c571fd19d0e1f2ae18305c43d26ab779b545a5def82a0004bd71`. **Sin pushear
ni publicar**: falta, con aprobación del usuario, el push, el release y
`TESTNET_LIVE=true` en la web. Pool de minería: ZSilent 3.0 lo sacó (era protocolo
de Zenith); recomendación dada: minería individual por ahora y más adelante un
pool sin custodio estilo P2Pool (el consenso ya admite coinbase de hasta 64
salidas, así que no hace falta fork). Decisión del usuario pendiente.

**22/10: un nodo de Quantic nunca llega a Groth16.** Preparando el instalador
apareció que, en release y sin el setup de Groth16, verificar cualquier prueba de
Zenith es un `panic!` (`build_setup_bundle`). En un nodo de Quantic había cinco
caminos que llegaban ahí a pedido de un par: un bloque con cuerpos privados desde la
altura 25.570 (la política de Zenith corría antes que la forma de Quantic), el
encabezado de un bloque ya conocido con cuerpos inventados, el gossip de Zenith,
`/private/tx` y la cola de privadas bajada por RPC. Cerrados con pruebas en el
commit `77d5cda3` (`quantic-cadena-completa`). En ZSilent (`quantic-solo`, commit
`97fca30`) el seed sale del manifiesto: el `-Dzsilent.bootstrapMultiaddr` de
Zenith le ganaba. El setup de Groth16 (12 MB) sigue en el instalador a propósito:
si quedara algún camino sin cubrir, verifica en vez de colgarse.

La prueba de humo del release (`scratchpad/interop/humo-release.sh`: nodo de
Quantic sin setup al lado, cuerpo de Zenith a `/private/tx`) encontró lo que las
unitarias no veían: `StateManager::chain_id()` devolvía siempre Zenith, así que la
API de un nodo de Quantic no sabía su red (la guarda no se activaba, `/blocks/0`
era el padre de Zenith y `POST /sync/block`, prendido en la plantilla de los
nodos RPC públicos, aplicaba reglas de Zenith). Arreglado en `c985304b`, junto con
la consola de minería de Quantic (decía `mining=off` y `0 block(s)` mientras
minaba). El binario 3.0.0 de release está en `target/release` y los comandos de
los dos pasos del usuario están probados con él en una carpeta temporal.

La semilla de la testnet ya está en el manifiesto (`fb9b732d`): ZSilent usa una
sola onion y una sola identidad de iroh para todas las redes, así que la PC
publica la misma onion que en Zenith (`d5pk3…`) en el puerto 28180 y el mismo
EndpointId (`ca7721e2…`); el instalador que se publique ya la lleva. **Ensayo
general de los pasos 3 a 5 con wallet y autoridad descartables: pasó entero**
(génesis embebido importado, segundo nodo con suministro exacto, la wallet
"equipo" ve los 5.000.000 ZKA, checkpoint híbrido aceptado). El guion de los
pasos 3 y 4 quedó en el repo: `scripts/lanzar-testnet-quantic.py`. Los ensayos
siguen fuera del repo (matan solo procesos de su carpeta):
`C:\Users\MDERRA~1\AppData\Local\Temp\claude\c--Users-mderramus-ZOKA\44fbeb6a-61fb-49c2-ae95-13d4b284bb2d\scratchpad\interop\`
(`ensayo-testnet.sh <carpeta del binario> <hash del génesis>`, con `TEAM_HOME`
y `TEAM_PASSWORD` solo si la wallet es descartable, y `humo-release.sh`).

**21/10: dos redes, Zenith retirada.** Pedido del usuario ("no quiero que
se mezclen las redes"): solo existen **Zoka Quantic Testnet** y **Zoka Quantic
Mainnet**, las dos desde cero; nada de Zenith pasa a Quantic. La testnet arranca
**ya** con el protocolo de hoy y se reinicia desde cero con el pool postcuántico;
la mainnet sale solo con el protocolo completo. Hecho y commiteado (nada pusheado,
publicado ni instalado):

- **Nodo 3.0.0** (commit `c0b6c300`): en una build con `quantic`, la red por omisión
  es quantic-testnet, "testnet"/"mainnet" nombran a las de Quantic, `--network
  zenith` y los comandos de Zenith responden qué usar; `zoka help` solo Quantic;
  Zenith se muestra como retirada. Suites completas en verde (987+89+2+3 y 929+86+2).
- **TLS híbrido X25519MLKEM768 en iroh** (`src/network/pq_tls.rs`, commit
  `01001969`), solo en procesos de Quantic.
- **Web y papers 2.1** (`zokaweb`, rama `whitepaper-ironwood`, commit `a75ebfd`):
  dos redes, Zenith como historia, sección 4.8 del pool; el estado de la testnet
  sale de `TESTNET_LIVE` en `src/components/zoka/network.ts` (ponerlo en `true` al
  lanzar).
- **Falta para lanzar la testnet:** los dos pasos del usuario de
  [LANZAMIENTO-quantic.md](docs/LANZAMIENTO-quantic.md) (wallet `equipo` y
  `checkpoint-keygen --network quantic-testnet`; la dirección se lee con
  `quantic-wallet address`, las públicas vienen en la salida del keygen). Después:
  `scripts/lanzar-testnet-quantic.py` (génesis, economics.rs y manifiesto activo),
  compilar, ensayo local con un segundo nodo, instalador de ZSilent 3.0.

**21/10: decisiones del usuario.** Prioridad: terminar la cadena Zoka Quantic
completa (arquitectura) para lanzar testnet y mainnet, y después ZSilent; ZKAPriv
queda para más adelante.

- **ZSilent pasa a ser solo de Quantic** (reemplaza a Zenith; Zenith queda en las
  versiones anteriores).
- **Parámetros aprobados:** prefijos `zq1` / `zqtest1` / `zqregtest1`, memo de 512
  bytes, comisión de 0,0001 ZKA por acción con mínimo de 2 acciones.
- **La mainnet sale solo cuando todo sea postcuántico** (elección "Todo
  postcuántico antes"). Hoy Quantic es postcuántica en privacidad, no en el
  gasto: RedPallas, Halo 2 y los compromisos de Orchard caen con Shor. La testnet
  puede salir antes con Orchard. Inventario y atacante simulado:
  [QUANTIC-INVENTARIO-CRIPTOGRAFICO.md](docs/QUANTIC-INVENTARIO-CRIPTOGRAFICO.md);
  pool nuevo (STARK con Plonky3 + Poseidon2, a prototipar y medir, también en el
  teléfono de 32 bits): [QUANTIC-POOL-POSTCUANTICO.md](docs/QUANTIC-POOL-POSTCUANTICO.md).
- **Rama `quantic-cadena-completa`:** perfil de mainnet, autoridad híbrida
  obligatoria, nodo con pila grande, y el minero de Quantic con control en caliente
  para ZSilent (`--no-mine` + `/mining/start|stop`, `/quantic/mining`, `/stats`,
  `mine-status` con datos de Quantic). `quantic-wallet` completa para ZSilent:
  escaneo guardado y cifrado (`<nombre>.scan`), `generate-phrase`, `list` con
  desbloqueo, `--address`, `estimate-fee`, `show-phrase`, `change-password`,
  `backup`/`import-backup`, `history --limit` con hora.
- **ZSilent 3.0, rama `quantic-solo` de `zsilent-core`** (sobre `wallet-liviana`):
  solo Quantic (testnet por omisión, prefijo `zqtest1`, P2P 28180 también en el
  onion), `WalletService` sobre `quantic-wallet`, el nodo sin contraseña y minando
  a `--quantic-mine-to`, enviar con memo y MAX real, recibir con QR animado,
  historial con montos, alta de 24 palabras, sin minado en equipo. 72 pruebas y
  una de punta a punta contra un nodo regtest (`interop/zsilent-wallet.sh`).
  Para el release: el nodo tiene que salir como 3.0.x (mismo mayor.menor) y
  compilarse con `--features randomx,quantic` (el script de release lo exige).

**16 y 17/10: desarrollo post-cuántico en modo autónomo** (pedido del usuario:
"enfocate en el desarrollo post quantic"; "continúa ... cuando no sepas qué hacer
seguí el roadmap"). **Nada está pusheado ni publicado ni instalado.** Ninguna
red cambió: todo es código en ramas, probado en local.

- **Nodo, rama `etapa5-quantic-nodo`** (apilada sobre `etapa6-firmas-hibridas` y
  `etapa5-ironwood`): perfil Zoka Quantic completo detrás de la feature `quantic`
  (bloques, mempool, minería, P2P, API `/quantic/*`, `zoka quantic-wallet`),
  probado en una red local de tres nodos con reorganización
  ([perfil](docs/ETAPA5-perfil-quantic-nodo.md)); dificultad ASERT con trabajo
  real (7.2, [doc](docs/ETAPA7-dificultad-quantic.md)); protocolo P2P de wallets
  livianas `/zoka-quantic/light/1` con presupuesto por par; estado guardable de la
  wallet (biblioteca); motor del teléfono `src/mobile_quantic.rs` con red Quantic
  en `mobile_net` ([API](docs/QUANTIC-MOTOR-MOVIL.md)); especificación en inglés
  para la revisión externa ([7.7](docs/REVIEW-zoka-quantic.md)).
  Pruebas: `cargo test` (921+85+2) y `cargo test --features quantic`
  (962+86+2+3) en verde con la ASERT; después, las de red y de Quantic de cada
  cambio. La biblioteca: 34 unitarias y 7 de punta a punta con Halo 2 real.
- **ZKAPriv, rama `quantic-motor`:** puente JNI del motor (`native/src/quantic.rs`,
  feature `quantic`), compila para ARMv7. No toca Kotlin. Los `.so` de `jniLibs`
  modificados sin commitear **no son de este trabajo** (venían de antes): no
  pisarlos.
- **Web, rama `whitepaper-ironwood` de `zokaweb`:** whitepaper y litepaper 2.0 y
  `SectionOverlay.tsx` reescritos con lo implementado (Ironwood, X-Wing, sin
  ceremonia, comisiones públicas, checkpoints del equipo declarados). PDFs
  regenerados y revisados. **Sin publicar: lo revisa el usuario.**
- **libp2p 0.53 → 0.57.1 (7.1), hecho:** rama `libp2p-057` (encima de
  `etapa5-quantic-nodo`; worktree `c:\tmp\zoka-libp2p`). `cargo audit` pasó de 16
  vulnerabilidades a 0. Todas las pruebas en verde. Interoperabilidad probada en
  una red Quantic regtest local: un nodo 0.53 y uno 0.57 se sincronizan en los dos
  sentidos y un envío que entra por el 0.57 lo mina el 0.53.
- **Arreglo que afecta a Zenith** (commit `53bdcfc5`): un nodo completo cuyo único
  par es un teléfono dejaba de minar mientras le pedía bloques que el teléfono no
  tiene (más de un minuto por vez). Ahora el teléfono contesta al toque y el nodo
  no insiste. Lo encontró `examples/probar_motor_quantic`, que prueba el motor del
  teléfono de punta a punta contra un nodo real (41 s); también encontró que la
  prueba de Halo 2 desbordaba una pila de 1 MB (ahora corre en un hilo propio).

**Para contarle al usuario (decisiones suyas):**

1. Zenith tiene dos debilidades de consenso que Quantic no hereda: su prueba de
   trabajo no cubre el cuerpo del bloque, y elige la cadena por la suma de los
   bits y no por el trabajo. Los checkpoints de la PC las mitigan. ¿Se corrigen
   con un fork o queda como testnet protegida? Opciones y recomendación en
   [ZENITH-debilidades-consenso.md](docs/ZENITH-debilidades-consenso.md).
2. Revisar y aprobar la web (`whitepaper-ironwood`) antes de publicarla; declara
   los checkpoints del equipo y que las comisiones son públicas.
3. Prefijos de dirección (`zq1`, `zqtest1`, `zqregtest1`), tamaño del memo (512)
   y comisión (0,0001 ZKA por acción).
4. Interfaz de Quantic en ZKAPriv y ZSilent (el motor está; las pantallas no).
   Opciones y recomendación en
   [PROPUESTA-quantic-en-las-apps.md](docs/PROPUESTA-quantic-en-las-apps.md).
5. Pasos suyos para abrir la testnet de Quantic: crear la wallet del equipo
   (`zoka quantic-wallet new --network quantic-testnet --name equipo`) y pasar la
   dirección; crear la mitad ML-DSA de la clave de la autoridad
   (`zoka checkpoint-keygen --mldsa`).
6. Pushear las ramas a GitHub y GitLab cuando lo apruebe.
7. Una release de testnet con el arreglo del teléfono que frena al nodo y con
   libp2p 0.57: binario nuevo en la PC y `.so` nuevo en ZKAPriv.

**15/10: primera prueba real de `zkq1`** (el 25.630 se minó el 14/10 a las 23:25,
sin errores).

- **ZSilent → teléfono: OK.** Tx `81cede9a…42fc`, dos salidas v2 (`output_version`
  2, `kem_ciphertext` de 1.120 bytes), confirmada en el bloque 25.780; ZKAPriv la
  encontró en su revisión siguiente (100 ZKA, el monto que puso el usuario).
- **Teléfono → ZSilent: falló dos veces antes de armar la prueba**, con "el
  sondeo de altura … fue inconcluso … Timeout while waiting for a response". El
  teléfono llega al nodo solo por Tor (semillas `.onion` fijas en
  `Constants.ZOKA_P2P_SEEDS`), abre una conexión por consulta (72 en una hora) y
  parte de los pedidos quedan sin respuesta: a las 08:43 abandonó un lote que el
  nodo sirvió recién entre las 08:44 y las 08:45. Coincide con la revisión de
  fondo, que repite 12 cuerpos que nadie tiene. Pendiente: reintentar; si sigue,
  reproducirlo con `probar_cliente_movil` y arreglarlo en ZKAPriv (reintentar el
  sondeo, no competir con la revisión de fondo).
- **Los QR de las `zkq1` son ilegibles** en las dos apps (1.715 caracteres): hay
  que rediseñarlos.
- Sin QR, las direcciones se pasaron por adb: la del teléfono, con "Copy" y
  "Paste" en Enviar y leyendo el campo con `uiautomator dump` (ZKAPriv bloquea
  las capturas); la de ZSilent, escrita con `input text` en tramos de 40 con
  pausas (en tramos largos el teclado pierde caracteres) y verificada contra el
  original.
- No se leyeron las claves que el usuario dejó en el portapapeles: se pisaron con
  la `zkq1` del teléfono.

**14/10, 22:43: el bloque 25.570 (bundles canónicos) se minó a las 18:28 sin
errores en el log; la punta iba en 25.626, a 18 bits y ~6 min por bloque, así
que el 25.630 (`zkq1`) llegaba esa misma noche.** Checkpoint firmado en 25.608.
**Para el 15/10, a pedido del usuario:** confirmar que pasó el 25.630 y hacer los
envíos reales a `zkq1` ZSilent → teléfono y teléfono → ZSilent (las dos apps
abiertas).

**14/10, más tarde: ZSilent 2.1.3, arreglos del explorador** (rama
`explorador-interno` de zsilent-core; el nodo sigue en 2.1.2, mismo `zoka.exe`).
Publicada con aprobación del usuario (release `v2.1.3` de zsilent-core, MSI
`3ce744b0…` verificado por dentro y bajado de GitHub; `main` al día en GitHub y
GitLab) e instalada en la PC (respaldo `ZSilent Core.respaldo-20261014-v2.1.2`).
El usuario dudaba de que la altura fuera la correcta: lo era (checkpoint firmado
en 25.458 con el mismo hash que la cadena local; suministro 25.465 × 12 ZKA =
305.580, lo que mostraba la pantalla; el "1 peer" era el teléfono con ZKAPriv).
Lo que fallaba: el panel listaba alturas calculadas sin datos, y los botones
"Explorer ↗" abrían `zokascan.cc`, muerto desde julio (sus rutas `/rpc` y `/rpc2`
devuelven la página, sin datos, y dice "v1.6.1"). Ahora: últimos 8 bloques reales
que se abren con un clic, búsqueda de transacciones privadas con su bloque
(`GET /private/tx/:hash` da `status` y `block_height`) y los botones abren el
explorador interno. `prepare-release.ps1` deja que la app vaya adelante del nodo
solo en el último número. **Ojo:** la API del nodo corta a 1.200 lecturas por
minuto por cliente y bloquea hasta 64 s a quien se pasa (ZSilent comparte el
cliente 127.0.0.1): no recorrer la cadena bloque por bloque desde la PC.

**14/10: SALIÓ LA 2.1.2, alturas adelantadas a pedido del usuario: bundles
canónicos desde 25.570 y v2 / `zkq1` desde 25.630** (antes 30.284 y 31.004: once
días de minado con la PC sola a 17 bits). Cliente mínimo 2.1.2: la 2.1.0 y la 2.1.1
quedan fuera de la cadena desde 25.570 y sus releases avisan "Superseded".
Publicada en `github.com/zoka-network` (`v2.1.2` de nodo, ZSilent y ZKAPriv; `main`
de los tres en esa versión) y espejada en GitLab:

- Nodo (`9b129cd7`, `pq-experimental`): `zoka.exe` `74c95b7d…`, `zokahd.exe`
  `0914c854…`, zip `f3398397…`.
- ZSilent: MSI `7ce4451f…` (verificado por dentro: mismo `zoka.exe`, Tor 0.4.9.14
  `8c38513b…`). ZKAPriv: versionCode 15, APK `502c69bd…`, mismo certificado
  `061376f8…`. Lo publicado se bajó de GitHub y coincide byte a byte.
- **Instalado:** PC con ZSilent 2.1.2 (respaldo
  `ZSilent Core.respaldo-20261014-v2.1.1`; lo instaló Claude con el OK explícito
  del usuario) y teléfono con ZKAPriv 2.1.2 (`adb install -r`, conserva la wallet).
- Ese día, a pedido del usuario y fuera de Zoka: se desactivaron las cuatro tareas
  "BOTX …" del bot de trading, que prendían WSL al iniciar sesión (para Zoka no
  hace falta WSL), y se corrió `Desktop\optimizar-windows.ps1` como administrador:
  telemetría, mapas, fax, informe de errores, impresora, ubicación e indexado de
  búsqueda deshabilitados; el estado anterior quedó en
  `Desktop\optimizar-windows-antes.json` y se vuelve atrás con `-Revertir`.
- **Falta:** que la red llegue a 25.570 y 25.630 con la PC minando, ver que la
  autoridad siga firmando checkpoints, los envíos `zkq1` reales entre ZSilent y
  ZKAPriv y el anuncio (texto de la 2.1.2 en
  [docs/ANUNCIO-zenith-testnet-BORRADOR.md](docs/ANUNCIO-zenith-testnet-BORRADOR.md)).

**Después, el mismo 13/10: 2.1.1** (nodo y ZSilent, `main` de los tres repos ya en
la versión publicada). La autoridad de checkpoints mina aunque no tenga pares: Zenith
estaba quieta porque ninguna otra semilla respondía y ZSilent pide
`--min-mine-peers 1`. `zoka.exe` `6c6d8722…`, MSI `e86b1d03…`; instalada en la PC
(respaldo `ZSilent Core.respaldo-20261013-v2.1.0`). A las 21:58 la red volvió a
avanzar (25.245, 25.247…), dificultad 17 bits, unos 720 H/s: al 30.284 faltan entre
4 y 7 días de minado continuo.

**13/10/2026, a pedido del usuario: SALIÓ LA 2.1.0 DE TESTNET.** Publicada en
`github.com/zoka-network` (y espejada en GitLab), probada antes en una red local:

- Nodo `v2.1.0` (rama `release-2.1.0`, código de `b542a83f`, `pq-experimental`):
  `zoka.exe` `0ac64cfb…`, `zokahd.exe` `6ec21042…`. Fork en Zenith: **bundles
  canónicos desde 30.284 y salidas v2 / `zkq1` desde 31.004** (punta del día:
  25.244, la red estaba quieta). `PROTOCOL_FREEZE_ID` sin cambios.
- ZSilent `v2.1.0`: MSI `a8d3814b…` (verificado por dentro: mismo `zoka.exe`, Tor
  0.4.9.14). ZKAPriv `v2.1.0` (la `v1.1.0` ya existía de 2025): APK `e9176c93…`,
  mismo certificado `061376f8…`. Release `trusted-setup-mainnet-v1` con el `DIGEST`.
- **Instalado:** PC con ZSilent 2.1.0 (respaldo entero en
  `%LOCALAPPDATA%\ZSilent Core.respaldo-20261013-v2.0.2`) y teléfono con ZKAPriv
  2.1.0 (abre y carga el motor). **Falta que el usuario abra ZSilent y desbloquee**
  para que el nodo 2.1.0 corra y siga firmando checkpoints.
- **Falta:** el anuncio (texto listo en
  [docs/ANUNCIO-zenith-testnet-BORRADOR.md](docs/ANUNCIO-zenith-testnet-BORRADOR.md),
  lo publica el usuario), alguien que mine para llegar a 30.284, binario Linux
  (WSL no arrancó) y, después de 31.004, los envíos reales `zkq1` entre las apps.
  `main` sigue sin tocar: la versión está en las ramas `release-2.1.0`.

**La web y los papers están publicados. La base criptográfica experimental está
integrada al nodo detrás de la feature `pq-experimental`, apagada por defecto
(rama `postcuantica-integracion-nodo`), y Zenith se presenta como testnet en la
rama `zenith-testnet` de nodo, ZSilent y ZKAPriv, sin mover datos. Nada de eso
está publicado como release. La red actual no tiene activada privacidad
postcuántica.**

**12/10, a pedido del usuario:** `fullnode --sync-only` arreglado (rama
`arreglo-sync-only`); ZSilent, ZKAPriv y el nodo se presentan como **"Zoka
Quantic · Testnet"** y ZSilent tomó el estilo de zoka.network (ramas
`quantic-interfaz`); **etapa 3 implementada en el nodo y probada en una red
local, sin activar** (rama `etapa3-direcciones-hibridas`): consenso de la
salida v2 en todas las builds; envío, recepción, gasto y CLI de `zkq1` con
`pq-experimental`; la v2 exige también los bundles canónicos. Ver la etapa 3.

**13/10: la etapa 3 quedó cerrada en código, sin activar.** Motor móvil con
`pq-experimental` en los dos ABIs (+172 KB / +104 KB), ZKAPriv y ZSilent reconocen,
muestran y envían a la `zkq1` **solo cuando la red fija la altura** (hoy nula: no se
ve nada), y la comisión del minero viaja en un sobre v2 que **no cambia el
consenso**. Ramas `etapa3-apps-zkq1` (ZKAPriv `3ee8956`, `f2f8953`; ZSilent
`a55423f`) y `etapa3-direcciones-hibridas` (nodo). Nada instalado ni publicado.

**13/10, más tarde: el usuario delegó sus decisiones ("tomalas tú, menos la
VPS")** y pidió que **su PC sea el nodo autoritario** de Zenith y de Quantic.

- Decisiones tomadas y su criterio: [docs/DECISIONES-2026-10-13.md](docs/DECISIONES-2026-10-13.md).
  El ciphertext queda como arreglo JSON; los bundles canónicos y la v2 se activan
  en Zenith con la 2.1.0 (punta + una semana; la v2, un día después); las pruebas
  reales de `zkq1` entre apps van después de activar.
- **Nodo autoritario:** rama `checkpoints-autoritativos` (`9bcb6ad9`, sobre
  `etapa3-direcciones-hibridas`). Diseño en
  [docs/CHECKPOINTS-AUTORITATIVOS.md](docs/CHECKPOINTS-AUTORITATIVOS.md). La clave de
  Zenith está en la PC (`%APPDATA%\Zoka\.zoka\mainnet\checkpoint-authority.key`,
  pública `bf5c9215…7bd4`, compilada). **Es la única copia: el usuario tiene que
  respaldarla fuera de la PC.** Los demás nodos obedecen recién con un binario que
  la traiga (2.1.0). Suite: 904 + 81 + 1, sin fallos.
- VPS: paquete listo en `deploy/vps/` (`ed304d43`); no se contrata ni se arranca
  nada hasta que el usuario consiga los servidores.

**13/10, tarde: el usuario dijo "sigue tú solo".** Hecho, en ramas nuevas apiladas
sobre `checkpoints-autoritativos`:

- **Notas gastadas de ZSilent** (rama `arreglo-notas-gastadas`, `88e59f4a`): causa
  más probable hallada leyendo el código, no reproducida en vivo. Varios caminos
  guardaban una copia de wallets leída sin el candado y borraban la reserva de un
  envío hecho en el medio; el caso frecuente era el cierre de cada tanda de minado.
  Ahora se lee con el candado tomado (`with_fresh_private_wallets`). Falta verlo en
  ZSilent minando con el binario nuevo.
- **Checkpoints** (rama `checkpoints-correccion-manual`, `f94b624f`):
  `checkpoint-sign` para corregir una rama a mano, y el firmador no firma mientras
  su cadena no contenga el último checkpoint (si no, devolvía la red a la rama
  corregida).
- **Versiones:** `NETWORK_MIN_CLIENT_VERSION` = manifiesto (2.0.1); la prueba
  heredada de la 1.9.0 ahora comprueba coherencia y la suite pasa sin saltear nada.
- **Tor 0.4.9.14** (bundle 15.0.25 del 12/10): firma y SHA-256 verificados, fijado
  en zsilent-core rama `tor-0.4.9.14` (`9096d40`) y en
  `docs/zsilent-managed-tor-onion-seed.md`.
- **2.1.0:** procedimiento en [docs/RELEASE-2.1.0-testnet.md](docs/RELEASE-2.1.0-testnet.md)
  (el `PROTOCOL_FREEZE_ID` no cambia; las alturas se fijan el día de publicar) y
  sección 2.1.0 en el borrador de anuncio.
- **Etapa de gasto sin key images** (rama `etapa5-investigacion`): construcción
  elegida, **confirmada por el usuario**, **Ironwood** (el circuito corregido de Orchard sobre Halo 2,
  sin trusted setup), con el cifrado de notas X-Wing. Medido en el teléfono: 20 s
  por envío, 40 s para armar la clave al abrir, 111 MB. Ver
  [docs/ETAPA5-gasto-sin-key-images.md](docs/ETAPA5-gasto-sin-key-images.md).
- **Instalar en la PC:** el clasificador de permisos frenó dos veces reemplazar el
  `zoka.exe` instalado. Queda `scripts/instalar-en-pc.ps1`, que corre el usuario
  con ZSilent cerrado: instala el nodo de `target/release` y Tor 0.4.9.14, con
  respaldos y verificación de hashes y firma.

**Siguiente paso natural:** que el usuario corra `scripts/instalar-en-pc.ps1` y
respalde la clave de autoridad; después la 2.1.0 según su procedimiento (publicar
y anunciar, solo con su aprobación). Luego la etapa 4 (gastar sin key images), que
es diseño.

Las ramas del nodo están apiladas, cada una sobre la anterior:
`postcuantica-integracion-nodo` → `zenith-testnet` → `arreglo-sync-only` →
`quantic-interfaz` → `etapa3-direcciones-hibridas`. La última contiene todo.
En ZSilent y ZKAPriv: `zenith-testnet` → `quantic-interfaz`.

### Instalado en los equipos del usuario (12/10, con su autorización)

- **PC:** ZSilent desde `quantic-interfaz` (`21dac3e`) con `zoka.exe`/`zokahd.exe`
  release de `etapa3-direcciones-hibridas` (`38534d0c`, features por defecto, sin
  `pq-experimental`; `zoka.exe` SHA-256 `109B94FB…`), en
  `%LOCALAPPDATA%\ZSilent Core`. Copia completa de lo anterior en
  `%LOCALAPPDATA%\ZSilent Core.respaldo-20261012`; el `zoka.exe` previo también en
  `app\resources\bin\zoka.exe.bak-20261012`. Nota con los detalles:
  `INSTALADO-20261012.txt` en la carpeta de instalación. No se tocó `%APPDATA%\Zoka`;
  el binario nuevo abre la base real (altura 25.204 al instalar).
- **Teléfono (SM-A135M):** ZKAPriv 1.0.1 release (`com.zoka.zkapriv`, certificado
  `061376f8…`) desde `quantic-interfaz` (`8b60551`), con los motores recompilados
  contra `38534d0c`. Era la primera instalación (no había ninguna). Abre y muestra
  "Zoka Quantic · Testnet". La app usa `FLAG_SECURE`: no admite capturas de
  pantalla, a propósito.
- La versión declarada sigue siendo 2.0.2 (ZSilent y nodo) y 1.0.1 (ZKAPriv): no se
  cambió el número; alinear versiones es la etapa 7.4.
- **13/10, 14:08, el usuario corrió `scripts/instalar-en-pc.ps1`:** `zoka.exe`
  `1797905D…` y `zokahd.exe` `F50C4E41…` (código de `e142e349`, features por defecto;
  respaldos `*.bak-20261013`) y Tor 0.4.9.14 (respaldo
  `ZSilent Core\backup-tor-0.4.9.13-20261013`). Con ZSilent abierto el nodo firmó su
  primer checkpoint real: altura 25.238 (`a60526f5…`), la punta menos 6. **La red está
  quieta en 25.244 desde el 12/10: nadie mina.**
- **Respaldo completo de `%APPDATA%\Zoka` en `D:\Respaldo-Zoka-20261013`** (pedido
  del usuario; 2.505 archivos, verificado por tamaño y por hash de la clave de
  autoridad y de `iroh_key`). Incluye la clave de autoridad, las claves de la onion de
  la seed (`tor\zoka-p2p`), `iroh_key` y las wallets cifradas. Es un disco interno:
  sigue faltando una copia de la clave en un pendrive desconectado.

Segunda sesión, 11 y 12/10 (Claude): se cerró la integración de la etapa 1 y se
hizo la etapa 2 (Zenith como testnet) en ramas de los tres repos. El detalle de
lo hecho, lo probado y lo que falta está en cada etapa de abajo.

Este archivo es el punto de entrada del traspaso. El plan detallado y su historial
están en [docs/HOJA-DE-RUTA-mainnet-postcuantica.md](docs/HOJA-DE-RUTA-mainnet-postcuantica.md).
Ese documento conserva propuestas antiguas: no interpretar afirmaciones o diseños
propuestos como propiedades ya demostradas. Contrastar con el código, los resultados
de pruebas y las decisiones más recientes del usuario.

## Decisiones del usuario que hay que conservar

- La red actual, **Zenith**, pasa a ser la testnet. **Zoka Quantic** será una red nueva.
- Quantic arranca sin reclamo ni migración de saldos de Zenith.
- Premine del equipo: **5.000.000 ZKA**, gastable desde el génesis, dentro del máximo
  de **23.000.000 ZKA**. La minería emite los **18.000.000** restantes.
- Emisión acordada: 12 ZKA durante 1.051.200 bloques y 6 ZKA durante otros 897.600.
  Después termina el subsidio; la minería puede continuar con comisiones.
- El P2P Exchange se retira del nodo, ZSilent y la documentación. Preservar ese trabajo.
- Los cambios postcuánticos se implementan y prueban primero en la testnet.
- **VPS en espera:** el usuario avisará cuándo continuar con cuentas e infraestructura.
- Instalar en sus equipos solo con su indicación. **12/10: lo autorizó e instalé**
  (ver "Instalado en los equipos del usuario").
- Home: **ZOKA QUANTIC en una sola línea**, **Private by design. más destacado debajo**,
  esfera pequeña, fondo negro y acentos rosas. Esta fue la corrección final del usuario.
- **12/10:** ZSilent "ya no es más Zenith, pasa a ser Quantic": las dos apps muestran
  **"Zoka Quantic · Testnet"** (la mainnet Quantic todavía no existe) y ZSilent copia
  el estilo de la web (acento rosa `#F2ABD0`, ZOKA **QUANTIC**, "Private by design.").
- **12/10, etapa 3:** **D1 = ML-KEM-768 vía X-Wing** y **D2 = direcciones largas**
  (unos 1.715 caracteres, con QR y copiar/pegar).

## Lo terminado y dónde está

| Entregable | Estado y ubicación |
|---|---|
| Home y navegación | Publicados en [zoka.network](https://zoka.network); comprobados en escritorio y móvil |
| Whitepaper y litepaper v2 | Revisados y publicados como diseño de Quantic en desarrollo |
| Papers históricos v1 | Conservados con aviso inicial de correcciones; cuatro PDF publicados en total |
| Primitivas experimentales | [crates/zoka-pq](crates/zoka-pq/README.md): ML-KEM-768, X-Wing y ML-DSA-65 |
| Integración al nodo | Feature `pq-experimental` (apagada por defecto), `zoka::pq`; iroh-base 1.0.3 y curve25519-dalek 5.0.0 |
| Zenith como testnet | Rama `zenith-testnet` en los tres repos; ver [etapa 2](docs/ETAPA2-zenith-testnet.md) |
| Interfaz Zoka Quantic | Ramas `quantic-interfaz`; capturas en `zsilent-core/docs/capturas-quantic` |
| Arreglo de `--sync-only` | Rama `arreglo-sync-only` (nodo) |
| Etapa 3 (sin activar) | Consenso v2, envío/recepción/CLI y prueba en red local: [especificación](docs/ETAPA3-direcciones-hibridas.md) |
| Pruebas criptográficas | 13/13 en Windows, Linux x64, Linux ARM64 (qemu) y el teléfono ARMv7, con el lock alineado al nodo |
| Pruebas del nodo | Ver [resultados](docs/RESULTADOS-postcuantica-etapa1.md#integración-al-nodo) por plataforma |
| Ejecución pendiente | Android ARM64 (no hay dispositivo de 64 bits) y hardware ARM64 real |
| Benchmarks | Windows, Linux x64 y teléfono ARMv7 (repetido el 12/10 con el lock alineado); no equivalen a un escaneo completo de wallet |

Referencias Git verificadas al preparar este archivo:

- **Nodo:** rama `postcuantica-base-criptografica`, commit `1be41b57`.
  Publicada en GitHub y GitLab como rama de desarrollo; no se integró a `main`.
- **Nodo, etapa 1:** rama `postcuantica-integracion-nodo` (sobre `1be41b57`),
  commits `9c8fca0f` (integración) y `df74753c` (documentación).
- **Nodo, etapa 2:** rama `zenith-testnet` (sobre la anterior); ver etapa 2 abajo.
- La rama `postcuantica-integracion-nodo` (nodo) y las `zenith-testnet` de los
  tres repos se publicaron en GitHub (`zoka-network`) y GitLab. Ninguna se integró
  a `main`.
- **Web:** repositorio independiente `zokaweb`, rama `cerrar-web-quantic`.
  Commits `d4e0af6` (papers), `3c03184` (home y navegación) y `46e9afa`
  (título en una línea, privacidad más visible y esfera reducida).
  Publicados en `main` y en la rama en GitHub y GitLab.
- **Despliegue web:** Cloudflare Pages, proyecto `zokaweb`, carga directa.
  El push de Git por sí solo no despliega. Última URL inmutable:
  <https://b9b102ec.zokaweb.pages.dev>.

## Orden de trabajo pendiente

### 1. Integrar la base criptográfica al nodo

- [x] Revisar la rama y el estado Git de cada repositorio antes de modificarlo.
- [x] Resolver el conflicto: bastó subir `iroh-base` de 1.0.2 a 1.0.3, que solo
  acepta las versiones estables; `curve25519-dalek` pasa a 5.0.0. Diff de fuentes
  revisado: nada de lo que cambió está activo en el nodo.
- [x] `zeroize`: el crate acepta `^1.8.2` y el nodo resuelve 1.9.0. Los dos
  lockfiles quedaron alineados paquete por paquete.
- [x] Pruebas de compatibilidad: vector RFC 8032 para la identidad iroh y prueba
  cruzada con el binario 2.0.2 instalado: snapshot por iroh en las dos
  direcciones (349.675.478 bytes, 25.204 bloques) y wallets de un binario
  visibles en el otro. Contra la red real por Tor (12/10): el nodo nuevo y el
  2.0.2, en paralelo, se conectaron a seed3 y sincronizaron los mismos bloques
  (25.204 → 25.244). seed1 (PC del usuario) y seed2 no respondían.
- [x] **Falla previa encontrada y corregida (12/10, rama `arreglo-sync-only`,
  `1acd4c68`):** `fullnode --sync-only` no arrancaba ni en el 2.0.2 instalado
  (`invalid team member address`: sin wallet la dirección quedaba vacía y
  `TeamMiningRuntime::new` la rechazaba). Ahora ese modo usa un runtime de
  team-mining observador (`without_local_member`) con almacén en memoria: no
  actúa como miembro ni toca el `team.json`. ZSilent usa ese modo para bajar
  bloques antes del login. Suite: 884 + 79 + 3; probado con el binario en una
  carpeta temporal (llega a ready y no crea wallets). Falta un release.
- [x] Integrar `crates/zoka-pq` detrás de `pq-experimental`, apagada por defecto.
- [x] Pruebas del nodo en Windows y Linux x64 (880 + 76 + 3 en ambos); `check` para
  Android ARM64/ARMv7; build release cruzada a Linux ARM64 y subconjunto de
  pruebas bajo qemu (PQ, iroh, snapshot, economics, PoW RandomX, transacciones).
- [ ] Android ARM64: no hay dispositivo de 64 bits; el teléfono solo corre ARMv7.
  Linux ARM64 se ejecutó emulado con qemu-user, no en hardware real.

**Cierre:** ver [resultados](docs/RESULTADOS-postcuantica-etapa1.md#integración-al-nodo).
La única prueba del nodo que fallaba (`recovery_manifest_matches_the_compiled_1_9_consensus_policy`)
exigía la versión 1.9.0 con el crate en 2.0.2 desde agosto. Desde el 13/10 se llama
`network_manifest_matches_the_compiled_policy` y comprueba coherencia en vez de
fijar la versión: ya no hace falta `--skip`.

### 2. Preparar Zenith como testnet y definir activación

Hecha el 12/10 en la rama `zenith-testnet` de los tres repos. Detalle, decisión y
procedimiento: [docs/ETAPA2-zenith-testnet.md](docs/ETAPA2-zenith-testnet.md).

- [x] Presentar Zenith como testnet (el JSON suma `network_role`/`network_name`).
  Desde el 12/10 (ramas `quantic-interfaz`) el nombre visible es la marca nueva:
  "Zoka Quantic · Testnet" en ZSilent y ZKAPriv, "Zoka Quantic testnet" en CLI y
  API; ZSilent tomó además el estilo de zoka.network. ZKAPriv avisa que sus ZKA
  son de prueba.
- [x] Chain ID histórico conservado; `zoka-testnet-v1` retirada (nunca operó).
- [x] Directorios: **decisión de no mover nada.** Zenith sigue en `.zoka/mainnet`
  con la etiqueta interna `mainnet`, que las bases guardan en
  `chain:genesis:network`. `--network testnet|zenith|mainnet` abren Zenith. Mover
  los datos haría que un cliente viejo y uno nuevo vieran wallets distintas, y en
  la PC del usuario ya hay un `.zoka/testnet` de la red retirada con wallets
  propias, que no se toca.
- [x] Marco de activación: `economics::PqFeature` con alturas `None` en todas las
  builds. **Todavía no hay una altura acordada.**
- [ ] Anuncio: borrador en [docs/ANUNCIO-zenith-testnet-BORRADOR.md](docs/ANUNCIO-zenith-testnet-BORRADOR.md);
  publicarlo solo con aprobación del usuario y clientes nuevos publicados.
- [ ] Releases de nodo, ZSilent y ZKAPriv con estos cambios (no se generaron).

**Cierre:** compatibilidad probada con el binario 2.0.2 instalado (wallets visibles
en los dos sentidos, snapshot por iroh en los dos sentidos). Procedimiento de
cambio de cliente: reemplazar el binario, sin mover datos; volver atrás es
reinstalar el anterior. Suite del nodo en Windows: 883 + 79 + 3 OK (falla solo la
prueba heredada de la versión 1.9.0). ZSilent 62/62; ZKAPriv compila.

Ramas y commits de la etapa 2:

- **Nodo:** `zenith-testnet` sobre `postcuantica-integracion-nodo`: `357d2089`
  (presentación y retiro de la testnet vieja), `dab312af` (marco de activación),
  `b5414f5f` (ayuda de zokahd) y el de documentación.
- **ZSilent (`zsilent-core`):** `zenith-testnet` sobre `sacar-exchange` (`8c09b71`),
  commit `b86b1c5`. En la copia de trabajo de `zsilent-core` (rama `sacar-exchange`)
  había un cambio **sin commitear** en `ZSilentAppState.kt` de las 14:20 del 11/10 que
  borraba 1.195 líneas. **Resuelto el 12/10:** era la versión del primer release
  (0.1.0, `06c895d`, 19/05) regrabada con BOM, sin nada nuevo. Quedó guardada en
  `stash@{0}` de `zsilent-core` y el archivo volvió al último commit.
- **ZKAPriv (`zkapriv`):** `zenith-testnet` sobre `seguridad-octubre-2026` (`5e5752f`),
  commit `dc7f448`.

### 3. Direcciones, cifrado y escaneo de wallets

Rama `etapa3-direcciones-hibridas`: `986e98b5` (módulo híbrido), `38534d0c`
(costo en el teléfono), `4c5064f3` (consenso de la salida v2), `b814468b`
(armar, reconocer y gastar con `pq-experimental`), `4ea05c73` (CLI) y `6f3a1b57`
(prueba en red local: v2 atada a los bundles canónicos y mensaje P2P).
Especificación de trabajo: [docs/ETAPA3-direcciones-hibridas.md](docs/ETAPA3-direcciones-hibridas.md).
Decisiones del usuario del 12/10: probar en una red **local** de la PC (no en
la testnet pública), el coinbase sigue público (v1) y las apps muestran la
`zkq1` solo cuando la red la active. Nada de esto se instaló: la PC sigue con
el `zoka.exe` de `38534d0c`.

- [x] Especificar versiones, prefijos, checksum, tamaños máximos y recuperación de
  claves: dirección `zkq1`, versión 2, ~1.715 caracteres; misma clave de gasto que
  `zka1` y clave X-Wing derivada de la misma raíz (la frase recupera las dos).
- [x] Perfil híbrido y derivación contextual para salidas e importes
  (`src/private_wallet/hybrid.rs`, 8 pruebas). **Faltan** comisión del minero y
  coinbase (abiertos en la especificación: hoy el coinbase publica la dirección).
- [x] Conectar las salidas v2 al armado y la validación de transacciones, con el
  rechazo antes de la altura y el hash extendido solo para v2. El hash de las
  v1 quedó fijado con el código anterior. **La v2 exige además los bundles
  canónicos:** sin ellos los cuerpos no son de consenso y un nodo sin la
  activación aceptaba el bloque (medido en la red local).
- [x] Wallets guardadas y CLI (`wallet-receive --hybrid`, `tx-send --to zkq1…`).
  El vuelto sigue al pago (pago v2 ⇒ vuelto v2).
- [x] ZSilent y motor móvil (13/10): `.so` de los dos ABIs con `pq-experimental`,
  JNI `hybridWalletInfo`/`scanHybridOutputs`, escaneo v2 en lote en ZKAPriv, zkq1 y
  QR en las dos apps solo con altura, envío a zkq1 con la misma condición. Detalle
  en la sección "Clientes" de la especificación.
- [x] Comisión del minero (13/10): sobre v2 con ciphertext X-Wing en el campo
  efímero, etiqueta `zoka-hybrid-fee-claim-v2`; sin cambio de consenso (la regla de
  bloques solo exige campos no vacíos, y hay prueba). Sale v2 solo con la feature,
  la altura fijada y la wallet del minero local.
- [x] Prueba en red local de tres nodos (dos corridas): la v2 espera en el
  mempool hasta la altura, entra con sus cuerpos, el destinatario la ve y la
  gasta, los saldos cierran exactos y un nodo sin la activación rechaza el
  bloque. Hallazgos ajenos a la v2 (notas ya gastadas en la wallet del CLI,
  "Archived: 0" desde el nodo minero, chisme P2P muerto): en la especificación.
- [x] Medir escaneo en el teléfono de 32 bits: v2 3,8 ms por salida contra 1,5 ms de v1
  (`examples/hybrid_scan_bench.rs`, tres pasadas; ver la especificación). Faltan
  memoria, batería y tamaño real de los datos de la cadena.
- [x] D1/D2 decididos por el usuario: ML-KEM-768 vía X-Wing y direcciones largas.
- [ ] Diseñar y revisar cualquier etiqueta de vista antes de incorporarla: una etiqueta
  basada solamente en ECDH clásico puede filtrar pertenencia frente a un adversario cuántico.
- [ ] Probar envíos ZSilent → ZKAPriv y ZKAPriv → ZSilent, recuperación y resincronización.

**Referencia medida:** ML-KEM desencapsula en unos 1,11 ms y X-Wing en 3,41–3,52 ms
en el teléfono ARMv7. X-Wing incluye expansión interna de claves. No son tiempos
de escaneo completo ni una comparación aislada del coste del combinador.

### 4. Circuito de privacidad y eliminación de key images

**13/10: no se diseña un circuito propio.** Construcción elegida y confirmada por
el usuario: Ironwood, mediante el crate `orchard`, con el cifrado de notas X-Wing; quita además
el trusted setup de los gastos. Criterio, medición y próximos pasos en
[docs/ETAPA5-gasto-sin-key-images.md](docs/ETAPA5-gasto-sin-key-images.md). Las tareas
de abajo se cumplen adoptando ese circuito, salvo la revisión externa.

- [ ] Diseñar un circuito que demuestre propiedad de cada nota, pertenencia al árbol,
  derivación correcta de nullifiers y conservación de valor.
- [ ] Vincular la prueba al contenido de la transacción para impedir reutilización
  con otras salidas. Probar todas las entradas activas.
- [ ] Mantener compromisos consumidos, claves consumidas y caminos de inclusión
  como testigos privados en el diseño nuevo.
- [ ] Añadir pruebas adversariales de doble gasto, propiedad, pertenencia y emisión.
- [ ] Retirar CLSAG/key images únicamente cuando la construcción sustituta esté completa
  y revisada. Borrar campos del JSON no soluciona el problema criptográfico.
- [ ] Conseguir revisión externa y ejecutar una ceremonia multiparte con los circuitos finales.

**Hallazgos que no hay que perder:**

- El formato actual publica `input_commitments`, `input_public_keys` y caminos de
  inclusión; permite identificar entradas consumidas sin computación cuántica.
- El nullifier del wallet usa SHA-256 con dominio y la clave de gasto original.
  No confundirlo con el helper BLAKE3 de `src/transaction/nullifier.rs`.
- El `proof_nullifier` de Groth16 usa una relación sintética de 64 bits
  `secret * index + blind`; no demuestra la derivación SHA-256 del wallet.
- Resolver el logaritmo discreto de la clave pública de gasto no da directamente
  la preimagen de la clave de gasto. No repetir la afirmación contraria.

### 5. Snapshots, clientes y pendientes entre repositorios

- [ ] Implementar snapshots con Ed25519 + ML-DSA-65 y verificación obligatoria de ambos.
- [ ] Completar los motores móviles `.so` para los dos ABIs y generar APK revisables.
- [x] Corregir el problema de ZSilent que ofrece notas ya gastadas (13/10, rama
  `arreglo-notas-gastadas`). **Falta probarlo** en ZSilent minando con el binario nuevo.
- [ ] Revisar y sincronizar las ramas de seguridad/Tor/retiro del exchange de los demás
  repositorios, conservando los cambios previos y resolviendo divergencias sin force push.
- [ ] Rehospedar los archivos de trusted setup todavía referenciados en la cuenta antigua,
  verificar hashes y actualizar manifiestos y clientes de forma coherente.
- [ ] Publicar releases verificables en la nueva cuenta `zoka-network` cuando estén listos.

Los pendientes heredados y sus ramas están enumerados en las etapas 0, 0b y 0d
del plan detallado. Su estado debe comprobarse por repositorio; publicar la web
no significa haber publicado clientes nuevos.

### 6. Validación integral y preparación del génesis

- [ ] Resolver las actualizaciones mayores pendientes de red y sus avisos de seguridad.
- [ ] Ajustar dificultad inicial y verificar el tiempo objetivo de bloque con pruebas.
- [ ] Implementar y probar premine, suministro máximo, fronteras de emisión y comisiones.
- [ ] Fijar la dirección pública del equipo para el génesis; todavía no está definida aquí.
- [ ] Alinear versiones de crate, cliente mínimo, manifiestos y formatos de protocolo.
  13/10: cliente mínimo del código = manifiesto (2.0.1) y la prueba heredada de la
  1.9.0 comprueba coherencia; el salto a 2.1.0 va con la versión (ver
  [docs/RELEASE-2.1.0-testnet.md](docs/RELEASE-2.1.0-testnet.md)).
- [ ] Ejecutar pruebas integrales en la testnet y revisión externa de cifrado/circuitos.
- [ ] Revisar los papers contra lo efectivamente implementado, después de cerrar el diseño.

### 7. Lanzamiento de Quantic

- [ ] Fijar chain ID nuevo, génesis reproducible y parámetros finales.
- [ ] Activar desde el bloque 0 de Quantic el diseño que haya superado las validaciones.
- [ ] Preparar clientes, seeds, recuperación, explorador y documentación de operación.
- [ ] Explicar públicamente el premine y que los saldos de Zenith no se trasladan.
- [ ] Verificar los criterios anteriores antes de anunciar una mainnet operativa.

**Infraestructura VPS:** continúa en espera de la indicación del usuario. Tor se
conserva; un VPS no lo sustituye. No iniciar cuentas, servidores ni minería en la nube
como parte de este traspaso.

## Archivos para retomar

1. [Plan detallado e historial](docs/HOJA-DE-RUTA-mainnet-postcuantica.md).
2. [Diagnóstico de código y circuito](docs/REVISION-postcuantica-etapa1.md).
3. [Resultados y límites de validación](docs/RESULTADOS-postcuantica-etapa1.md).
4. [Benchmarks completos](docs/benchmarks/2026-10-11-pq.json).
5. [API experimental y límites](crates/zoka-pq/README.md).
6. [Script de validación de plataformas](scripts/check-pq-platforms.ps1).
7. [Procedimiento de papers](zokaweb/docs/papers.md).

Comandos existentes para verificar el crate independiente:

```powershell
cargo test --manifest-path crates/zoka-pq/Cargo.toml --locked
cargo clippy --manifest-path crates/zoka-pq/Cargo.toml --locked --all-targets -- -D warnings
cargo fmt --manifest-path crates/zoka-pq/Cargo.toml --check
./scripts/check-pq-platforms.ps1 -RunAndroidBench -Iterations 100 -AndroidRuns 3
```

El último comando requiere un dispositivo Android ARMv7 autorizado y las herramientas
del SDK/NDK. Usa archivos temporales de benchmark; no instala una aplicación.

## Cómo dejar el siguiente traspaso

- Actualizar este archivo al cerrar cada etapa: commit, pruebas, resultado y pendientes.
- Conservar los documentos/archivos ajenos al trabajo. Al preparar esta hoja había
  archivos sin seguimiento: `Zoka Network Web Applications.docx`, `apelacion-github.md`,
  `discord-bot/` y `zoka-guard-vault/`.
- No guardar tokens, semillas, claves privadas ni credenciales en la documentación.
- Separar siempre: diseño propuesto, código integrado, pruebas ejecutadas y activación real.
- No presentar el módulo experimental como privacidad postcuántica ya activa en la red.
