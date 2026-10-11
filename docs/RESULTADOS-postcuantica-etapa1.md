# Base postcuántica: resultados de plataforma

Validación del 11 de octubre de 2026, 18:01–18:04 UTC. Se implementó y probó el
crate independiente `crates/zoka-pq`. No está conectado al nodo: estas pruebas
no activan privacidad postcuántica, direcciones nuevas ni cambios de consenso.

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

## Trabajo pendiente

La integración exige resolver la incompatibilidad entre curve25519-dalek
5.0.0-rc.0 fijada por iroh-base 1.0.0 y la versión estable requerida por X-Wing.
El grafo de dependencias del nodo se conservó. Faltan ejecución en Linux y
Android ARM64, evaluación de memoria/escaneo, formato de direcciones y salidas,
derivación contextual, activación y pruebas integrales.

Retirar CLSAG requiere además un circuito nuevo que pruebe propiedad,
pertenencia, nullifiers y conservación sin revelar las entradas consumidas,
seguido de revisión externa y ceremonia. Ver la
[revisión técnica](REVISION-postcuantica-etapa1.md) y la
[hoja de ruta](HOJA-DE-RUTA-mainnet-postcuantica.md).
