# Zoka Quantic: hoja de ruta para continuar con Claude

Actualizada: **12 de octubre de 2026**, Argentina (segunda sesión, Claude).
Proyecto: `C:\Users\mderramus\ZOKA`.

## Punto de partida

**La web y los papers están publicados. La base criptográfica experimental está
integrada al nodo detrás de la feature `pq-experimental`, apagada por defecto
(rama `postcuantica-integracion-nodo`). La red actual no tiene activada
privacidad postcuántica.**

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
- Instalar un APK en el teléfono sigue pendiente de indicación del usuario.
- Home: **ZOKA QUANTIC en una sola línea**, **Private by design. más destacado debajo**,
  esfera pequeña, fondo negro y acentos rosas. Esta fue la corrección final del usuario.

## Lo terminado y dónde está

| Entregable | Estado y ubicación |
|---|---|
| Home y navegación | Publicados en [zoka.network](https://zoka.network); comprobados en escritorio y móvil |
| Whitepaper y litepaper v2 | Revisados y publicados como diseño de Quantic en desarrollo |
| Papers históricos v1 | Conservados con aviso inicial de correcciones; cuatro PDF publicados en total |
| Primitivas experimentales | [crates/zoka-pq](crates/zoka-pq/README.md): ML-KEM-768, X-Wing y ML-DSA-65 |
| Integración al nodo | Feature `pq-experimental` (apagada por defecto), `zoka::pq`; iroh-base 1.0.3 y curve25519-dalek 5.0.0 |
| Pruebas criptográficas | 13/13 en Windows, Linux x64, Linux ARM64 (qemu) y el teléfono ARMv7, con el lock alineado al nodo |
| Pruebas del nodo | Ver [resultados](docs/RESULTADOS-postcuantica-etapa1.md#integración-al-nodo) por plataforma |
| Ejecución pendiente | Android ARM64 (no hay dispositivo de 64 bits) y hardware ARM64 real |
| Benchmarks | Windows, Linux x64 y teléfono ARMv7 (repetido el 12/10 con el lock alineado); no equivalen a un escaneo completo de wallet |

Referencias Git verificadas al preparar este archivo:

- **Nodo:** rama `postcuantica-base-criptografica`, commit `1be41b57`.
  Publicada en GitHub y GitLab como rama de desarrollo; no se integró a `main`.
- **Nodo, etapa 1:** rama `postcuantica-integracion-nodo` (sobre `1be41b57`),
  commit `9c8fca0f` (integración) más el de documentación.
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
  visibles en el otro. Contra la red real por Tor no se pudo: ninguna de las tres
  semillas respondía (seed1 es la PC del usuario, apagada). Repetir con seed1 en línea.
- [ ] **Falla previa encontrada:** `fullnode --sync-only` no arranca ni en el 2.0.2
  instalado (`invalid team member address`: sin wallet, la dirección queda
  vacía y `TeamMiningRuntime::new` la rechaza, `src/main.rs`, armado del
  runtime de team-mining). ZSilent usa ese modo para bajar bloques antes del
  login. No se corrigió: queda para una rama aparte.
- [x] Integrar `crates/zoka-pq` detrás de `pq-experimental`, apagada por defecto.
- [x] Pruebas del nodo en Windows y Linux x64 (880 + 76 + 3 en ambos); `check` para
  Android ARM64/ARMv7; build release cruzada a Linux ARM64 y subconjunto de
  pruebas bajo qemu (PQ, iroh, snapshot, economics, PoW RandomX, transacciones).
- [ ] Android ARM64: no hay dispositivo de 64 bits; el teléfono solo corre ARMv7.
  Linux ARM64 se ejecutó emulado con qemu-user, no en hardware real.

**Cierre:** ver [resultados](docs/RESULTADOS-postcuantica-etapa1.md#integración-al-nodo).
La única prueba del nodo que falla (`recovery_manifest_matches_the_compiled_1_9_consensus_policy`)
ya fallaba antes: exige la versión 1.9.0 y el crate es 2.0.2 desde agosto (etapa 7.4).

### 2. Preparar Zenith como testnet y definir activación

- [ ] Presentar Zenith como testnet en CLI, ZSilent y ZKAPriv de forma consistente.
- [ ] Conservar el chain ID histórico: renombrarlo alteraría la identidad de la cadena.
- [ ] Diseñar cualquier cambio de directorios con copia y verificación de wallets.
- [ ] Implementar el marco de activación y definir una altura explícita antes de
  cambiar el consenso de la red existente.

**Cierre:** compatibilidad histórica probada y procedimiento reproducible de cambio
de cliente/datos. Todavía no hay una altura de activación acordada.

### 3. Direcciones, cifrado y escaneo de wallets

- [ ] Especificar versiones, prefijos, checksum, tamaños máximos y recuperación de claves.
- [ ] Fijar el perfil híbrido y la derivación contextual para salidas, importes,
  comisiones del minero y coinbase. Respetar la construcción X-Wing especificada.
- [ ] Adaptar nodo, ZSilent y motor móvil al formato nuevo.
- [ ] Medir escaneo representativo, memoria y tamaño de datos en el teléfono de 32 bits.
- [ ] Cerrar D1/D2 (parámetros y longitud de direcciones) con evidencia.
- [ ] Diseñar y revisar cualquier etiqueta de vista antes de incorporarla: una etiqueta
  basada solamente en ECDH clásico puede filtrar pertenencia frente a un adversario cuántico.
- [ ] Probar envíos ZSilent → ZKAPriv y ZKAPriv → ZSilent, recuperación y resincronización.

**Referencia medida:** ML-KEM desencapsula en unos 1,11 ms y X-Wing en 3,41–3,52 ms
en el teléfono ARMv7. X-Wing incluye expansión interna de claves. No son tiempos
de escaneo completo ni una comparación aislada del coste del combinador.

### 4. Circuito de privacidad y eliminación de key images

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
- [ ] Corregir y probar el problema pendiente de ZSilent que ofrece notas ya gastadas.
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
