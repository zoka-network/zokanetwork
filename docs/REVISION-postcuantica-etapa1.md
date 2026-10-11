# Revisión técnica: base postcuántica de Zoka Quantic

Esta revisión registra el diagnóstico inicial. La implementación independiente
y las mediciones realizadas después se describen en
[Resultados de plataforma](RESULTADOS-postcuantica-etapa1.md).

Fecha: 2026-10-11. Alcance: inspección de código, herramientas locales y fuentes
primarias; propuesta de implementación. No se alteró consenso, ni se abrieron
wallets o bases de datos reales, ni se instaló software en el teléfono.

## Resultado

Las etapas 1 y 3 todavía no están implementadas en el nodo: `Cargo.toml` no incluye
ML-KEM, ML-DSA ni X-Wing y las salidas siguen usando el intercambio clásico de
Ristretto. El primer entregable viable es un crate independiente con las primitivas,
pruebas de interoperabilidad y un benchmark ejecutable en los cinco destinos.

La etapa 5 no consiste en borrar key images o campos del JSON. El circuito actual
no prueba por sí solo la autorización y derivación de nullifiers requerida para
eliminar CLSAG. Necesita un diseño nuevo, pruebas adversariales, revisión externa y
un trusted setup posterior al cierre del circuito.

## Evidencia en el código

Las líneas son las observadas al preparar esta revisión; las funciones y campos
identifican el punto relevante aunque cambie su posición.

| Componente | Evidencia | Consecuencia |
|---|---|---|
| Dirección clásica | `src/private_wallet/mod.rs:235`, `payment_address_from_keys`; `:263`, `canonical_payment_address` | Claves públicas Ristretto, sin clave ML-KEM |
| Salida clásica | `src/private_wallet/mod.rs:762`, `derive_stealth_payload` | Secreto ECDH derivado de `ephemeral_scalar * scan_public` |
| Entradas reales públicas | `src/private_wallet/mod.rs:1328`, `input_commitments`; `:1332`, `input_public_keys`; `:3006`, selección de notas | Permite unir una entrada con su salida original sin computación cuántica |
| Caminos de inclusión públicos | `src/private_wallet/mod.rs:1355`, `input_inclusion_proofs` | También deben pasar a ser testigos privados en el formato nuevo |
| Nullifier efectivo del wallet | `src/private_wallet/mod.rs:910`, `derive_note_nullifier` | SHA-256 con dominio, clave de gasto original, identificador y compromiso |
| Nullifier usado por Groth16 | `src/private_wallet/mod.rs:2001`, construcción del testigo; `src/zk/groth16_backend.rs:1697` | Escalar artificial de 64 bits; relación `secret * index + blind` |
| Compromisos públicos en Groth16 | `src/zk/groth16_backend.rs:1656`, `proven_input_commitment_public_vars` | Ocultarlos en la serialización externa no basta: forman parte del statement público |
| Pertenencia parcial | `src/zk/groth16_backend.rs:57` y `:66` | Perfil v1 admite 96 entradas y prueba caminos para 8; el formato nuevo debe probar todas las entradas activas |
| Hash del árbol | `src/crypto/private_tree.rs:104`, `hash_pair` | Construcción propia de 16 rondas; requiere análisis antes de reutilizarse |

### Tres valores distintos que no deben confundirse

1. `Nullifier::from_commitment` (`src/transaction/nullifier.rs`) ofrece
   `BLAKE3(commitment || spending_key)`. No es la derivación usada al construir los
   envíos del wallet privado.
2. `derive_note_nullifier` calcula
   `SHA256("zoka-note-nullifier" || spending_key || 0x1f || note_id || 0x1f || commitment)`.
   Los identificadores y compromisos entran como bytes de sus cadenas.
3. `proof_nullifier` se construye para el circuito mediante un escalar reducido a
   64 bits y un blind. La prueba comprueba una relación algebraica; no comprueba
   la función SHA-256 anterior ni la vincula a la propiedad de cada nota.

La clave pública de gasto usa `scalar_from_secret_bytes`, que reduce
`SHA256(spending_key)` al campo escalar de Ristretto (`src/private_wallet/mod.rs:168`).
Resolver su logaritmo discreto no proporciona directamente la preimagen
`spending_key`. No es correcto afirmar que eso, por sí solo, revela los nullifiers
del wallet. La exposición de entradas, key images y ECDH sigue siendo un problema
independiente y suficiente para impedir una promesa de privacidad postcuántica.

## Bibliotecas verificadas

Se consultaron documentación de RustCrypto y metadatos de crates.io el 2026-10-11.
Son versiones candidatas para una base experimental, no una aprobación para mainnet.

| Crate | Versión | Rust mínimo declarado | Observación |
|---|---|---|---|
| `ml-kem` | `0.3.2` | `1.85` | FIPS 203 final; `rand_core 0.10`, `kem 0.3` |
| `ml-dsa` | `0.1.1` | `1.85` | FIPS 204 final; `signature 3` |
| `x-wing` | `0.1.1` | `1.85` | X25519 + ML-KEM-768; usa `x25519-dalek 3` |

Manifest propuesto, sujeto a compilación y pruebas:

```toml
ml-kem = { version = "=0.3.2", default-features = false, features = ["alloc", "getrandom", "zeroize"] }
ml-dsa = { version = "=0.1.1", default-features = false, features = ["alloc", "getrandom", "zeroize"] }
x-wing = { version = "=0.1.1", default-features = false, features = ["getrandom", "zeroize"] }
```

El `rand 0.8` del nodo no implementa los traits de `rand_core 0.10`. Usar los métodos
de generación con RNG del sistema de los crates, o un adaptador explícito. No
mezclar tipos de las dos generaciones de `x25519-dalek`; Cargo puede resolver ambas.
Fijar el lockfile del crate nuevo para reproducir la evaluación.

Las tres bibliotecas declaran que no han recibido una auditoría independiente.
ML-DSA 0.1.1 está después de la corrección de GHSA-hcp2-x6j4-29j7, publicada para
0.1.0-rc.3 y posteriores; eso no equivale a una auditoría completa.

Fuentes: [ML-KEM](https://docs.rs/ml-kem/latest/ml_kem/),
[ML-DSA](https://docs.rs/ml-dsa/latest/ml_dsa/),
[X-Wing](https://docs.rs/x-wing/latest/x_wing/),
[advisory de RustCrypto](https://github.com/RustCrypto/signatures/security/advisories/GHSA-hcp2-x6j4-29j7).
Metadatos reproducibles:
[ml-kem 0.3.2](https://crates.io/api/v1/crates/ml-kem/0.3.2),
[ml-dsa 0.1.1](https://crates.io/api/v1/crates/ml-dsa/0.1.1),
[x-wing 0.1.1](https://crates.io/api/v1/crates/x-wing/0.1.1).

### Compatibilidad X-Wing

El crate declara draft 06; la especificación actual es draft 11, del 2026-09-23.
El historial publicado desde 06 describe cambios editoriales, ASN.1 y unidades de
longitud SHAKE, sin enumerar un cambio de algoritmo. Esto sugiere compatibilidad,
pero debe verificarse contra los vectores actuales antes de fijar el perfil.
No justifica implementar un combinador propio de entrada.

Tamaños X-Wing: clave pública 1216 bytes, ciphertext 1120, semilla secreta 32 y
secreto compartido 32. La construcción incluye 1088 bytes de ciphertext ML-KEM y
32 de X25519. El combinador especificado no incluye el ciphertext ML-KEM; no
modificarlo para satisfacer una descripción genérica del plan. El contexto propio
de Zoka debe incorporarse en una derivación posterior claramente especificada.

El crate ofrece aceptación o rechazo del comportamiento X25519 no contributivo
(`DecapsulationKeyRejectNonContrib`). El protocolo deberá fijar uno y probarlo.
Además, su desencapsulado reconstruye la clave expandida desde la semilla: el
benchmark debe medir este coste y distinguirlo del ML-KEM puro.

Fuentes: [especificación X-Wing](https://datatracker.ietf.org/doc/draft-connolly-cfrg-xwing-kem/),
[implementación RustCrypto](https://github.com/RustCrypto/KEMs/blob/master/x-wing/src/lib.rs),
[vectores de RustCrypto](https://github.com/RustCrypto/KEMs/blob/master/x-wing/tests/test-vectors.json).

## Primer módulo propuesto

`crates/zoka-pq` será un crate standalone, sin importación desde el nodo durante
esta fase. Debe incluir:

- Wrappers ML-KEM-768, X-Wing y ML-DSA-65 con tamaños y formatos explícitos.
- Validación estricta de longitudes y claves públicas ML-KEM canónicas.
- Secretos con borrado al liberar memoria; sin representación de secretos en logs.
- Tests con vectores independientes, errores de formato y manipulación.
- Un ejecutable `pq-bench` que solo use datos sintéticos y reporte JSON.

El benchmark medirá keygen, encapsulado, desencapsulado, firma y verificación;
tiempo por operación, número de iteraciones y tamaño de objetos/formatos. Separará
inicialización y expansión de claves del bucle de escaneo y consumirá resultados
con `black_box` para evitar optimizaciones que eliminen trabajo. Un roundtrip por
algoritmo debe comprobarse antes de medir; no basta con obtener tiempos.

## Entorno disponible

Inspeccionado mediante `rustup show`, `rustup target list --installed`, `rustc
--version`, `adb devices -l` y `adb shell getprop ro.product.cpu.abilist`.

| Herramienta | Ubicación o estado |
|---|---|
| Cargo | `C:\Users\mderramus\.cargo\bin\cargo.exe` |
| Rustup | `C:\Users\mderramus\.cargo\bin\rustup.exe` |
| Rust estable | `1.95.0 (59807616e 2026-04-14)`, host Windows MSVC x64 |
| cargo-ndk | `C:\Users\mderramus\.cargo\bin\cargo-ndk.exe` |
| ADB | `C:\Users\mderramus\AppData\Local\Android\Sdk\platform-tools\adb.exe` |
| Android SDK | `C:\Users\mderramus\AppData\Local\Android\Sdk` |
| Android NDK | `C:\Users\mderramus\AppData\Local\Android\Sdk\ndk\27.3.13750724` |
| LLVM del NDK | `C:\Users\mderramus\AppData\Local\Android\Sdk\ndk\27.3.13750724\toolchains\llvm\prebuilt\windows-x86_64` |
| Teléfono conectado | Samsung `SM_A135M`, autorizado por ADB |
| ABI del teléfono | `armeabi-v7a,armeabi`; permite medir ARM de 32 bits |

Targets Rust instalados: `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`,
`aarch64-linux-android`, `armv7-linux-androideabi` y `wasm32-unknown-unknown`.
Falta `aarch64-unknown-linux-gnu`. La instalación de un target permite comprobar
tipos y compilar bibliotecas; enlazar y ejecutar en Linux requiere además un
toolchain adecuado y un entorno de ejecución de esa arquitectura.

### Comandos propuestos de validación

Estos comandos son el plan para el crate futuro; no se ejecutaron como parte de
esta revisión. Ajustar nombres si cambian en la implementación.

```powershell
cargo test --manifest-path crates/zoka-pq/Cargo.toml --locked
cargo clippy --manifest-path crates/zoka-pq/Cargo.toml --all-targets --locked -- -D warnings
cargo run --manifest-path crates/zoka-pq/Cargo.toml --release --locked --bin pq-bench
rustup target add aarch64-unknown-linux-gnu

$pqTargets = @(
    'x86_64-pc-windows-msvc',
    'x86_64-unknown-linux-gnu',
    'aarch64-unknown-linux-gnu',
    'aarch64-linux-android',
    'armv7-linux-androideabi'
)
foreach ($pqTarget in $pqTargets) {
    cargo check --manifest-path crates/zoka-pq/Cargo.toml --locked --all-targets --target $pqTarget
    if ($LASTEXITCODE -ne 0) { throw "Fallo de compilación: $pqTarget" }
}

Push-Location crates/zoka-pq
try {
    $env:ANDROID_NDK_HOME = 'C:\Users\mderramus\AppData\Local\Android\Sdk\ndk\27.3.13750724'
    cargo ndk -t arm64-v8a -P 26 build --release --locked --bin pq-bench
    if ($LASTEXITCODE -ne 0) { throw 'Fallo Android arm64' }
    cargo ndk -t armeabi-v7a -P 26 build --release --locked --bin pq-bench
    if ($LASTEXITCODE -ne 0) { throw 'Fallo Android armv7' }
} finally {
    Pop-Location
}
```

La medición posterior en el teléfono puede copiar solamente ese ejecutable
sintético a `/data/local/tmp/zoka-pq-bench`, darle permiso de ejecución y recoger
su salida mediante ADB. No requiere reemplazar APK, abrir el wallet ni acceder a
los datos de ZKAPriv. Linux x64 y aarch64 requieren también ejecutar los tests y
benchmark en esas arquitecturas para afirmar validación de ejecución, además de
`cargo check`.

## Criterios de prueba y cierre

1. KATs de FIPS 203/204 y X-Wing con procedencia y versión registradas; comparar
   clave pública, ciphertext, secreto compartido y firmas según corresponda.
2. Roundtrip, clave incorrecta, longitudes truncadas o sobrantes, clave ML-KEM no
   canónica y ciphertext manipulado. ML-KEM usa rechazo implícito: normalmente
   produce otro secreto, no un error explícito para ciphertext alterado.
3. X-Wing: alterar cada componente por separado; fijar y probar el tratamiento
   de resultados X25519 no contributivos.
4. ML-DSA: rechazar mensaje, firma, contexto o clave incorrectos.
5. Compilación en cinco targets; ejecución y medición reportadas con arquitectura,
   versión de Rust, perfil y número de iteraciones. No presentar `cargo check`
   como benchmark o como prueba de ejecución.
6. Documentar resultados ARMv7 antes de cerrar D1/D2 o diseñar etiquetas de vista.
   Una etiqueta verificable únicamente con ECDH clásico filtraría pertenencia a
   un adversario cuántico; requiere análisis de privacidad, no solo velocidad.

## Dependencias para las etapas posteriores

La integración necesita formato versionado, prefijo de dirección, recuperación y
derivación separada de secretos, contexto de cifrado, límites de tamaño y altura
explícita de activación en Zenith. Monedas, comisiones y coinbase deben usar el
mismo perfil criptográfico documentado. Nada de esto queda activado por añadir
el crate independiente.

Para retirar CLSAG, el circuito nuevo debe probar la propiedad de cada nota, su
compromiso, su pertenencia al árbol, el cálculo del nullifier y la conservación
de valor. Commitments consumidos, claves consumidas y caminos serán testigos
privados; los públicos incluirán las raíces, nullifiers y los elementos de salida
que el protocolo defina. Debe ligarse también el contenido de la transacción para
impedir reutilizar una prueba con salidas diferentes. Esta revisión no prescribe
una construcción criptográfica casera ni acredita la seguridad del circuito actual.

No declarar privacidad postcuántica de la red hasta terminar las etapas 3 y 5,
medir el escaneo móvil, hacer pruebas integrales y completar la revisión externa.
