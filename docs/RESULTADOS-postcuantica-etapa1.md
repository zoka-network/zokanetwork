# Base postcuántica: resultados de plataforma

Validación del 11 de octubre de 2026, 18:01–18:04 UTC. Se implementó y probó el
crate independiente `crates/zoka-pq`. En esa validación todavía no estaba
conectado al nodo; la integración posterior está en
[Integración al nodo](#integración-al-nodo). Ninguna de estas pruebas activa
privacidad postcuántica, direcciones nuevas ni cambios de consenso.

## Compilación y ejecución

| Destino | Check y biblioteca release | Ejecutable enlazado | Pruebas ejecutadas | Benchmark ejecutado |
|---|---|---|---|---|
| Windows x86_64 MSVC | Sí | Sí | 13/13 | 1 pasada |
| Linux x86_64 GNU | Sí | No | No | No |
| Linux aarch64 GNU | Sí | No | No | No |
| Android arm64-v8a | Sí | Sí | No | No |
| Android armeabi-v7a | Sí | Sí | 13/13 | 3 pasadas |

También pasaron `cargo fmt --check` y Clippy con `-D warnings`. Los tests
incluyen respuestas conocidas de NIST ACVP y X-Wing, corrupción, claves
incorrectas, límites de formato/contexto y redacción de secretos en Debug.
El resultado no equivale a una auditoría criptográfica ni a ejecutar el nodo.

Entorno: Rust 1.95.0, cargo-ndk 4.1.2, Android NDK 27.3.13750724. Windows:
Intel Core i5-6400 a 2,70 GHz, cuatro núcleos. Teléfono: Samsung SM-A135M,
API Android 34, ABI `armeabi-v7a,armeabi`. Se ejecutaron archivos temporales
con datos sintéticos; se retiraron después. No se instalaron APK ni se
abrieron wallets o bases de datos.

## Tiempos observados

Microsegundos por operación. Windows muestra la mediana de una pasada;
Android, el mínimo y máximo de las medianas de tres pasadas independientes.
Cada pasada usa 100 iteraciones por operación después de cinco calentamientos.

| Operación | Windows, mediana µs | Android ARMv7, rango de medianas µs |
|---|---:|---:|
| ML-KEM-768, generar clave | 58,60 | 966,58–970,08 |
| ML-KEM-768, expandir semilla | 54,50 | 938,67–939,98 |
| ML-KEM-768, encapsular | 52,30 | 898,15–900,81 |
| ML-KEM-768, desencapsular con clave expandida | 65,00 | 1107,48–1117,44 |
| X-Wing, importar semilla y expandir clave pública | 95,30 | 1640,10–1690,71 |
| X-Wing, copiar clave pública ya almacenada | 0,30 | 2,50–2,54 |
| X-Wing, encapsular | 162,70 | 2223,94–2275,19 |
| X-Wing, desencapsular incluida expansión interna | 237,65 | 3413,83–3517,89 |
| ML-DSA-65, generar clave | 441,25 | 4358,98–4379,10 |
| ML-DSA-65, expandir semilla | 436,40 | 4336,19–4356,90 |
| ML-DSA-65, firmar 64 bytes | 1009,75 | 5370,96–15332,50 |
| ML-DSA-65, verificar 64 bytes | 73,45 | 614,58–621,39 |

La firma determinista reutiliza una clave y un mensaje durante cada pasada;
la clave cambia entre pasadas. El amplio rango de firma no debe ocultarse:
estas muestras no caracterizan una distribución general de mensajes/claves.
Los tiempos incluyen liberación y borrado de los valores devueltos. No se
fijaron frecuencia de CPU, afinidad ni condiciones térmicas. No se midieron
memoria, batería, latencia de red, lecturas de disco ni escaneo completo.

ML-KEM reutiliza una clave expandida; X-Wing 0.1.1 reconstruye internamente
la clave secreta al desencapsular. Por eso los dos tiempos no son una
comparación aislada del coste del combinador híbrido. El perfil medido es
`x-wing-draft06-rustcrypto-0.1.1-reject-noncontributory`.

Como extrapolación aritmética, 100.000 desencapsulados secuenciales al coste
mediano observado tomarían unos 111 s con ML-KEM o 341–352 s con X-Wing en
este teléfono. No es una medición del wallet. Hace falta medir un escáner
representativo antes de fijar las decisiones D1/D2 o un esquema de view tags.

## Evidencia y reproducción

Las cuatro salidas JSON completas, hashes de sus archivos de origen y datos
del entorno se conservan en [benchmarks/2026-10-11-pq.json](benchmarks/2026-10-11-pq.json).
Los logs locales están en `docs/artifacts/pq-20261011-180158` (ignorados por Git).
SHA-256 del lockfile independiente:
`CFA9E775F403084E3FCB11C547619EDC41D570A5C32C496A72FF12BB37191258`.

```powershell
cargo test --manifest-path crates/zoka-pq/Cargo.toml --locked
cargo clippy --manifest-path crates/zoka-pq/Cargo.toml --locked --all-targets -- -D warnings
./scripts/check-pq-platforms.ps1 -RunAndroidBench -Iterations 100 -AndroidRuns 3
```

La primera ejecución del script terminó antes de copiar archivos al teléfono:
cargo-ndk consumía el JSON de Cargo y no se encontraba el test compilado. La
ejecución documentada arriba usa la detección corregida de un único harness
ELF en el directorio de compilación aislado y terminó con todos los pasos en cero.

## Integración al nodo

Rama `postcuantica-integracion-nodo`, 11 y 12 de octubre de 2026 (traspaso a Claude).

### Dependencias

- `iroh-base` 1.0.2 → 1.0.3. No cambia código: solo acepta las versiones
  estables de `curve25519-dalek` y `ed25519-dalek`. Con eso `curve25519-dalek`
  pasa de 5.0.0-rc.0 a 5.0.0, la que X-Wing exige a través de `x25519-dalek 3`.
  No cambió ningún otro paquete que el nodo ya usaba.
- Entre 5.0.0-rc.0 y 5.0.0 se revisó el código fuente: solo hay traits nuevos
  detrás de las features `group` y `lizard`, un método renombrado en Ristretto
  y el flag de AVX-512. En el nodo están activas `alloc`, `digest`,
  `precomputed-tables`, `rand_core`, `serde` y `zeroize`; ninguna de las que
  cambiaron.
- `zoka-pq` pasa de `zeroize = "=1.8.2"` a `^1.8.2`: el nodo resuelve 1.9.0
  porque `iroh-base` la exige. `zeroize` 1.9.0 pide Rust 1.85, igual que el crate.
- El `Cargo.lock` propio del crate quedó alineado con el del nodo: cada paquete
  de su árbol resuelve a la misma versión en los dos. SHA-256 nuevo:
  `1D7F3E8882943ED4107096F2CA4CCC056790D5DF622040824A4E494FCBE1985B`.
- El nodo depende del crate con la feature `pq-experimental`, apagada por
  defecto, que solo expone `zoka::pq`. Ningún camino de consenso, wallet o
  dirección la usa, y el motor móvil (`default-features = false`) no la compila.

### Pruebas agregadas

- Identidad iroh leída de disco contra el vector 1 de RFC 8032 (clave pública
  y firma): los EndpointId de los nodos existentes no cambian con la versión
  estable de la curva.
- `tests/pq_experimental.rs`: ML-KEM-768, X-Wing y ML-DSA-65 dentro de la build
  del nodo.

### Resultados

| Plataforma | Qué se ejecutó | Resultado |
|---|---|---|
| Windows x86_64 (Rust 1.95.0) | Suite completa del nodo con `pq-experimental` | 880 lib (6 ignoradas), 76 bin, 3 PQ: todas OK. Falla 1 prueba heredada (ver abajo) |
| Windows x86_64 | 13 pruebas del crate con el lock alineado; Clippy `-D warnings`; fmt | OK |
| Linux x86_64, WSL2 Debian 13 (Rust 1.97.1) | 13 pruebas del crate | OK |
| Linux x86_64, WSL2 | Suite completa del nodo con `pq-experimental` | Igual que Windows: 880 lib (6 ignoradas), 76 bin, 3 PQ OK; la misma prueba heredada falla (65 min) |
| Linux aarch64, qemu-user 10.0.13 (emulado) | 13 pruebas del crate | OK (9,6 s) |
| Linux aarch64, qemu-user | Build release cruzada del nodo (GCC 14.2, con RandomX y RocksDB) y subconjunto de pruebas, en la rama `zenith-testnet` | Build OK (24 min 39 s, binario de 51 MB). Bajo qemu: PQ 3/3, iroh 5/5, snapshot 15/15, economics 15/15, `mining::pow` 13/13 (RandomX, 406 s), transaction 15/15 |
| Android arm64-v8a y armeabi-v7a (NDK 27.3) | `check` de la biblioteca del nodo con `mobile` y con `mobile,pq-experimental` | Los cuatro OK |
| Android armeabi-v7a, Samsung SM-A135M (API 34) | Script de plataformas completo con el lock alineado: check y biblioteca en los cinco destinos, 13 pruebas en el teléfono y tres pasadas de benchmark | 13/13. Medianas: ML-KEM desencapsula en 1104–1156 µs y X-Wing en 3433–3527 µs, como el 11/10; firma ML-DSA 1648–4154 µs (sigue muy dispersa). Ver [benchmarks/2026-10-12-pq.json](benchmarks/2026-10-12-pq.json) |
| Android arm64-v8a | Ejecución | No hay dispositivo de 64 bits; el teléfono del usuario solo ejecuta ARMv7 |

Benchmarks de Linux x86_64 dentro de WSL2 (tres pasadas de 100 iteraciones,
rango de medianas en µs, sin detener otras tareas del sistema): ML-KEM-768
desencapsula en 61,0–63,1 y encapsula en 49,3–50,7; X-Wing desencapsula,
incluida la expansión, en 208,5–220,0 y encapsula en 144,4–150,2; ML-DSA-65
firma en 554–1001 y verifica en 71,4–74,0. Son del mismo orden que los de
Windows en esta máquina. La pasada bajo qemu solo comprueba que el binario
ARM64 ejecuta: sus tiempos son de emulación. Datos completos en
[benchmarks/2026-10-12-pq-linux.json](benchmarks/2026-10-12-pq-linux.json).

La prueba que falla en Windows y en Linux,
`recovery_manifest_matches_the_compiled_1_9_consensus_policy`
(`tests/mainnet_recovery_policy.rs`), exige que el crate sea la versión 1.9.0;
el `Cargo.toml` dice 2.0.2 desde el 9 de agosto de 2026 (commit `e73e14ff`).
No la causa esta integración: es el desalineo de versiones de la etapa 7.4.

### Compatibilidad con la red

Binarios: el 2.0.2 instalado en ZSilent (SHA-256 `7DBCF418…`, iroh-base 1.0.2 y
curva 5.0.0-rc.0) contra uno nuevo de la rama `zenith-testnet`, que contiene
esta integración sin otros cambios de dependencias (SHA-256 `CED0D8C6…`,
features por defecto). Cada prueba usó un `ZOKA_HOME` temporal y una copia de
la base real de Zenith (altura 25.204); no se tocó `%APPDATA%\Zoka`.

| Prueba | Resultado |
|---|---|
| Servidor nuevo (`--serve-iroh-snapshot`) → cliente viejo (`snapshot-fetch-iroh`) | OK: 349.675.478 bytes por iroh, 25.204 bloques aplicados, misma altura (696 s en total) |
| Servidor viejo → cliente nuevo | OK: 349.675.478 bytes, 25.204 bloques, misma altura (715 s) |
| Nodo nuevo contra la red real por Tor, 12/10 13:00 (argumentos como los pasa ZSilent: `--allow-isolated-start`, `--sync-warmup-secs 30`, `--tor-proxy`) | OK: se conectó a seed3 por su onion, entró al gossip de todos los tópicos y sincronizó 25.204 → 25.244, la punta de la red durante la prueba |
| Binario 2.0.2 instalado, misma prueba y en paralelo | Igual: seed3 por Tor, 25.204 → 25.244. Los dos aceptaron los mismos bloques (25.206 `f42925f1…`, 25.244 `8f276db6…`) |

Un intento anterior (11/10, de noche) no pudo conectarse: ninguna semilla
respondía por Tor (`curl` SOCKS, código 97) mientras una onion de control
respondía. El 12/10 seed3 volvió (seed1, la PC del usuario, y seed2 siguieron
sin responder). Con un Tor recién iniciado, el plazo inicial de 20 s por
semilla puede no alcanzar; ZSilent usa `--allow-isolated-start` y el nodo
reintenta en segundo plano, que es como se corrió la prueba buena.

Hallazgo aparte: `fullnode --sync-only` no arranca ni en el binario instalado
ni en el nuevo (`failed to initialize team-mining runtime: invalid team member
address`). Sin wallet, la dirección local queda vacía y `TeamMiningRuntime::new`
la rechaza. ZSilent usa ese modo para bajar bloques antes del login, así que esa
descarga previa falla desde que existe (agosto); los bloques bajan cuando el
nodo arranca con la wallet. No se corrigió en esta etapa; las pruebas de arriba
usaron una wallet de prueba nueva con `--no-mine`.

## Trabajo pendiente

Faltan ejecución en Android ARM64 (hace falta un dispositivo de 64 bits) y en
hardware ARM64 real, evaluación de memoria/escaneo, formato de direcciones y
salidas, derivación contextual y pruebas integrales. El marco de activación por
altura está en la rama `zenith-testnet` (etapa 2), sin alturas fijadas.

Retirar CLSAG requiere además un circuito nuevo que pruebe propiedad,
pertenencia, nullifiers y conservación sin revelar las entradas consumidas,
seguido de revisión externa y ceremonia. Ver la
[revisión técnica](REVISION-postcuantica-etapa1.md) y la
[hoja de ruta](HOJA-DE-RUTA-mainnet-postcuantica.md).
