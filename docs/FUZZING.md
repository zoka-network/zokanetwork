# Fuzzing de Zoka Quantic

10/11/2026, rama `endurecimiento-sin-consenso`. Qué se deforma, qué se le exige
al código, cómo correrlo y qué encontró la primera vez.

No es una auditoría ni prueba que no haya fallas: es una forma barata de
encontrar las que se dejan encontrar. Lo que sigue describe lo que se corrió,
no una garantía.

## Qué se deforma

Todo lo que el nodo y las billeteras leen de alguien más:

| Objetivo | Quién manda esos bytes | Qué se le exige |
| --- | --- | --- |
| `transaction` | un par, o quien use la API | no cae; lo que acepta tiene una sola codificación |
| `proof` | quien arma un pago | no cae; solo se acepta la prueba que hizo el probador |
| `scan_page` | el nodo que le sirve páginas a una billetera | no cae; las dos claves de vista abren lo mismo |
| `envelope` | quien paga | no cae; cada bit del sobre está autenticado |
| `text` | quien pega una dirección o una clave de vista | no cae; cada texto aceptado es el único de su valor |
| `wallet_state` | el disco (un archivo cortado o dañado) | no cae: se usa o se rechaza |

Las seis funciones viven en
[crates/zoka-pq-pool/src/fuzzing](../crates/zoka-pq-pool/src/fuzzing/mod.rs).
Se compilan solo en las pruebas y con la feature `fuzzing`: no entran en un
nodo ni en una billetera.

Alrededor de ellas, [tests/quantic_node_fuzz.rs](../tests/quantic_node_fuzz.rs)
prueba el nodo entero:

- **Bloques de un par.** Se parte de un bloque honesto con un pago probado, se
  le cambia la cabecera, la lista de notas gastadas o lo que lleva, y se
  rehace lo que cualquier minero puede rehacer (la raíz de Merkle y el
  trabajo). Lo que el nodo acepte tiene que ser el tercer bloque, emitir lo
  que marca el calendario y llevar el pago que se probó.
- **Pedidos a la API.** Rutas, números, cuerpos y tipos de contenido
  deformados contra la API local y la pública. Después de cada pedido el nodo
  tiene que seguir respondiendo `/status`, y al final tiene que aceptar y
  minar el pago real.

## Dos maneras de deformar

Postcard no se describe a sí mismo: los bytes no dicen dónde termina un
vector. Cambiar bytes de una prueba casi siempre da algo que el decodificador
rechaza antes de que corra código de verdad. Por eso hay dos mutadores:

- **De bytes** ([mutate.rs](../crates/zoka-pq-pool/src/fuzzing/mutate.rs)):
  invertir un bit, cortar, repetir un tramo, injertar un tramo de otra
  entrada válida.
- **De forma** ([tree.rs](../crates/zoka-pq-pool/src/fuzzing/tree.rs)):
  captura el valor tal como postcard lo escribiría, con sus secuencias y
  opciones, lo edita (saca un elemento, repite otro, vacía una opción, pone
  un elemento de campo igual al módulo) y lo vuelve a escribir. El resultado
  sigue decodificando, con vectores de largos que nadie previó: son las
  entradas que encuentran las comprobaciones de largo que faltan.

A las transacciones editadas se les recalcula el txid, y a las páginas las
cabeceras, como haría un atacante: un hash lo recalcula cualquiera.

## Cómo correrlo

Con cada `cargo test`, sin herramientas especiales:

```powershell
cargo test --release --manifest-path crates/zoka-pq-pool/Cargo.toml --lib fuzzing::
cargo test --release --features quantic-pq --test quantic_node_fuzz
```

Las rondas dependen solo de una semilla: una falla se repite corriendo lo
mismo. La entrada que rompió un objetivo queda en
`crates/zoka-pq-pool/target/fuzz-failures/`.

- `ZOKA_FUZZ_SCALE=20` corre veinte veces más rondas (unos 20 minutos en la
  PC para el pool).
- `ZOKA_FUZZ_SEED=<número>` arranca en otro lado. Conviene cambiarla en cada
  corrida larga.

### Guiado por cobertura (libFuzzer)

[fuzz/](../fuzz/Cargo.toml) tiene un objetivo de `cargo fuzz` por cada
función. Necesita Rust nightly y Linux: se corre en un VPS, no en la PC.

```sh
rustup toolchain install nightly && cargo install cargo-fuzz
# Una entrada válida por objetivo (prueba dos transferencias: unos segundos).
(cd crates/zoka-pq-pool && cargo run --release --features fuzzing --example fuzz_seeds -- ../../fuzz/corpus)
cd fuzz
cargo +nightly fuzz run scan_page -- -max_total_time=3600
# Una prueba pesa 1,9 MB: hay que subir el largo máximo.
cargo +nightly fuzz run proof -- -max_len=4000000 -max_total_time=3600
cargo +nightly fuzz run transaction -- -max_len=4000000 -max_total_time=3600
```

**Estado:** los seis objetivos compilan (`cargo check` en Windows) y las
semillas se generan. **No se corrieron** con libFuzzer: falta hacerlo en un
VPS. Es el paso que sigue, y el que más puede encontrar.

## Lo que encontró

Corrida del 10/11/2026: unas 25.000 rondas por pasada del pool, una pasada
veinte veces más larga con otra semilla (unas 500.000 entradas, 20 minutos) y
2.800 rondas contra el nodo.

### Cierre verificado por Codex

La batería que dejó Claude terminó el **10/11 a las 21:54:30 (Argentina)**.
Se revisaron los resultados finales y ya no quedan procesos de esas pruebas.
Los registros se copiaron y se compararon por SHA-256 en
`target/endurecimiento-2026-11-10/` (evidencia local, fuera de git):

| Registro | Resultado | Duración de las pruebas |
| --- | --- | --- |
| `pool-all-2.log` | 36 aprobadas, 0 fallas | 119,36 s |
| `nodo-lib-tests.log` | 201 aprobadas, 0 fallas, 3 ignoradas, 883 filtradas | 228,17 s |
| `node-fuzz-3.log` | 4 aprobadas, 0 fallas | 61,31 s |
| `fuzz-pool-x20.log` | 9 aprobadas, 0 fallas; escala 20 | 1.178,10 s |

Las tres ignoradas del nodo requieren los archivos de WebAssembly o Tor
real. Las pruebas finales del pool y del nodo terminaron después de los
últimos cambios del código; la pasada extendida es anterior al guardado de
pánicos del verificador. No se repitió la batería al retomar ni se ejecutó
libFuzzer en Linux. También se conservó `fuzz-crate-check.log`, del chequeo
de compilación de sus seis objetivos en Windows.

### Hallazgos

1. **Una billetera liviana se caía al abrir un estado dañado.** Un árbol de
   notas guardado con un nivel de menos se aceptaba y `NoteTree::root`
   indexaba fuera del vector. El estado es un archivo local, así que no es un
   ataque remoto: es una billetera que no abre después de un corte de luz.
   Ahora un árbol guardado se revisa antes de usarlo (niveles, cantidad de
   nodos por nivel, elementos de campo, subárboles vacíos de la red) y uno
   que no cierra se rechaza
   ([ledger.rs](../crates/zoka-pq-pool/src/ledger.rs), `StoredTree`).

2. **Dos cuerpos de bloque distintos pueden tener el mismo hash.** El árbol
   de Merkle de un bloque empareja sus hojas y, si un nivel tiene una cantidad
   impar, empareja la última consigo misma. Un bloque con hojas impares y el
   mismo bloque con su última transacción repetida tienen la misma raíz, la
   misma cabecera, el mismo trabajo y el mismo hash. Es el defecto que Bitcoin
   corrigió como CVE-2012-2459.
   - **Hoy no le sirve a un atacante.** El cuerpo con el pago repetido se
     rechaza solo (su lista de notas gastadas no coincide con la que la
     cabecera compromete, y además repite un nullifier). Mandado de nuevo con
     el hash de un bloque que el nodo ya tiene, el nodo contesta «ya lo
     tengo» y no cambia nada de lo que guarda: al promover una rama conocida
     solo escribe índices, nunca cuerpos.
   - **Queda fijado con una prueba**
     (`a_body_that_shares_a_valid_block_s_hash_is_never_stored`), y es una
     razón para no recordar nunca un bloque como inválido solo por su hash.
   - **El arreglo es de consenso:** un árbol que comprometa su cantidad de
     hojas cambia el hash de todos los bloques. Va con el próximo reinicio de
     la red.

3. **El verificador de pruebas aguantó.** En unas 11.000 pruebas deformadas
   (de forma y de bytes) y 2.500 pagos con la prueba tocada no cayó ni aceptó
   una prueba distinta de la del probador. Tampoco para otro enunciado. Los
   testigos de trabajo, que con la configuración actual piden cero bits,
   entran igual al transcript: cambiarlos invalida la prueba, así que una
   prueba no se puede reescribir sin romperla.
   - Igual se agregó una red: si alguna prueba hace caer al verificador, esa
     prueba es inválida (`guarded` en
     [transfer.rs](../crates/zoka-pq-pool/src/transfer.rs)). El nodo verifica
     los pagos con sus candados de estado y de mempool tomados; una caída ahí
     los dejaba envenenados y el nodo rechazaba todo pedido hasta reiniciarlo.

4. **Sin novedad en lo demás.** Los 4.000 cambios de un bit en un sobre se
   rechazaron del lado que correspondía (el destinatario, el emisor o los
   dos). Ningún texto deformado se aceptó como dirección o clave, y lo que se
   acepta tiene una sola forma de escribirse.

## Qué falta

- Correr libFuzzer en un VPS durante horas, con las semillas de arriba.
- Fuzzing de lo que este trabajo no tocó: el protocolo entre nodos con
  conexiones reales (acá se prueba lo que hay detrás de cada pedido, no el
  transporte), el Stratum y el motor del teléfono por JNI.
- Un arnés que mute cada registro del circuito de forma sistemática: es otra
  técnica y otro trabajo.
