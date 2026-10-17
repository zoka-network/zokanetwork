# Etapa 7.2: tiempo de bloque y dificultad de Zoka Quantic

16/10/2026. Implementado en `src/quantic/difficulty.rs` (rama `etapa5-quantic-nodo`),
activo solo en las cadenas de Quantic. Zenith no cambia.

## Lo que se encontró en Zenith

Revisando el ajuste de dificultad para fijar el de Quantic aparecieron tres
problemas. En Zenith los mitiga la autoridad de checkpoints (la PC firma cada 30
bloques y ningún nodo se reorganiza por debajo), así que no son urgentes allí,
pero Quantic no los puede heredar:

1. **La dificultad va en potencias de 2.** Se cuenta en bits de ceros del hash y
   se ajusta ±1 bit por bloque. Entre 16 y 17 bits la dificultad se duplica: si la
   ideal cae en el medio, oscila sin asentarse.
2. **El ajuste mira un solo bloque.** Sube si el último salió en menos de 60 s y
   baja si tardó más de 240 s. El tiempo de un bloque es muy variable
   (exponencial), así que salta, y depende de la hora que declara el minero.
3. **La cadena se elige por la suma de los bits, no por el trabajo.** Un bloque
   de 12 bits suma 12 y uno de 17 suma 17, aunque el segundo cueste 32 veces más.
   Con horas a más de 240 s entre bloques, una rama privada baja en pocos
   bloques al piso de 12 bits; con 61 s entre bloques se queda ahí, y suma
   12 / 61 ≈ 0,20 por segundo contra 17 / 120 ≈ 0,14 de la cadena honesta, con
   4.096 hashes por bloque. Con muy poco cómputo se puede armar una rama "más
   pesada" que la honesta.

A esto se suma que la prueba de trabajo de Zenith no cubre el cuerpo del bloque
(ver [ETAPA5-perfil-quantic-nodo.md](ETAPA5-perfil-quantic-nodo.md)).

## El diseño de Quantic

- **Objetivo de 256 bits.** Un bloque vale si su hash, leído como número big
  endian, no supera el objetivo. Se guarda en el formato compacto de 32 bits de
  Bitcoin (`nBits`) en el lugar donde el nodo guardaba los bits; el formato está
  probado contra el `0x1d00ffff` de Bitcoin.
- **ASERT** (`aserti3-2d`, el de Bitcoin Cash desde 2020), anclado en el génesis:

  ```text
  objetivo = objetivo_génesis · 2^((t_padre − t_génesis − 120 · (h_padre − h_génesis)) / τ)
  ```

  con la aritmética entera exacta de la especificación (punto fijo 16.16 y la
  aproximación cúbica de 2^x). Cada bloque depende solo del génesis y de su padre:
  no hay ventana que manipular. Si la red mina el doble de rápido, la dificultad
  se duplica en una vida media; si nadie mina, baja sola con el tiempo.
- **Vida media τ = 1 hora** (30 bloques). Bitcoin Cash usa 2 días para una red
  grande; una red chica, donde una PC prende y apaga la minería, necesita
  reaccionar en horas. Revisable antes de mainnet.
- **Tiempo de bloque: 120 s**, el de Zenith y el de la emisión ya decidida
  (12 ZKA durante 1.051.200 bloques ≈ 4 años).
- **Dificultad inicial de la testnet: 14 bits.** En Zenith la PC sola mina a
  16–18 bits con 3 a 6 minutos por bloque (unos 500 hashes RandomX por segundo):
  para 120 s harían falta unos 15,8 bits. Arrancar un poco por debajo da bloques
  rápidos al principio, que ASERT corrige en pocas horas.
- **Piso: 12 bits** (el de Zenith): ASERT no baja de ahí aunque nadie mine.
- **Trabajo de un bloque = 2^256 / (objetivo + 1)** del objetivo que tenía que
  cumplir, no de los ceros que tuvo de suerte. La cadena que se elige es la de
  más cómputo esperado, como en Bitcoin.
- **Hora:** después de la mediana de los últimos 11 bloques de su rama, y no más
  de 2 horas adelante del reloj del nodo que lo recibe.
- **Regtest:** objetivo fijo de 8 bits, como el regtest de Bitcoin.

## Pruebas

`cargo test --features quantic --lib quantic::` (y la red local de tres nodos):

- formato compacto ida y vuelta, igual al de Bitcoin, redondeo hacia abajo;
- el hash cumple el objetivo como número big endian;
- el trabajo se duplica con cada bit y diez bloques de 12 bits pesan menos que
  seis de 17 (lo que en Zenith da al revés);
- ASERT: en horario no cambia; una vida media de adelanto lo hace el doble de
  difícil y una de atraso la mitad; muy atrasado se queda en el piso; monótono;
- la cadena de regtest en el nodo, el mempool, la wallet liviana y una
  reorganización, con el objetivo y el trabajo nuevos;
- un bloque con la hora de la mediana o tres horas en el futuro se rechaza.

## Pendiente

- Revisar τ y la dificultad inicial con la testnet andando, antes de mainnet.
- Zenith: decidir con el usuario si se corrige (fork con altura de activación)
  o se deja como testnet protegida por los checkpoints.
