# Zenith: dos debilidades de consenso y qué hacer con ellas

17/10/2026. Para que el usuario decida. Las encontró la revisión de la dificultad
de Quantic (etapa 7.2); Quantic nace con las dos resueltas. **Zenith no cambió.**

## Qué pasa

1. **La prueba de trabajo no cubre el cuerpo del bloque.** `build_pow_input`
   (`src/mining/pow.rs`) mete cadena, padre, dirección del minero, altura, hora y
   nonce, pero no la raíz de las transacciones. Alguien puede tomar un bloque
   recién minado, cambiarle el cuerpo (otras transacciones privadas) y
   reenviarlo con el mismo sello. El cuerpo nuevo igual se valida entero: no se
   pueden crear monedas ni gastar lo ajeno. Lo que sí se puede es que un nodo que
   reenvía saque o cambie transacciones de un bloque ajeno, y que dos versiones
   del mismo bloque compitan con el mismo trabajo.
2. **La cadena se elige por la suma de los bits de suerte.** Cada bloque suma los
   bits de ceros que *salieron* en su hash (`leading_zero_bits` del sello, en
   `src/state/chain.rs`), no el trabajo que costó: un bloque de 12 bits suma 12 y
   uno de 17 suma 17, aunque el segundo cueste 32 veces más. Con horas
   manipuladas el ajuste de ±1 bit baja la dificultad en pocos bloques, así que
   con poco cómputo se puede armar una rama "más pesada" que la honesta.

## Qué lo mitiga hoy

La PC firma un checkpoint cada 30 bloques y los nodos 2.1.x no se reorganizan
por debajo del último ([CHECKPOINTS-AUTORITATIVOS.md](CHECKPOINTS-AUTORITATIVOS.md)).
Lo expuesto son los bloques posteriores al último checkpoint: a 3–6 minutos por
bloque, unas pocas horas como mucho. Zenith es la testnet y sus ZKA no tienen
valor.

## Opciones

| | Qué cambia | Esfuerzo | Riesgo |
|---|---|---|---|
| **A. Dejarla así** | Nada; se documenta (ya está en el whitepaper revisado y en la etapa 7.2) | Ninguno | Reorganizaciones de los últimos bloques entre checkpoints; aceptable en una testnet |
| **B. Solo la elección de cadena** | El trabajo de cada bloque pasa a 2^bits del objetivo que tenía que cumplir, desde una altura | Chico: la función del trabajo y una altura de activación (el trabajo guardado de los bloques viejos no se toca) | Nodos viejos y nuevos podrían preferir ramas distintas en una pelea; los checkpoints lo resuelven |
| **C. Las dos, con un fork** | Además, la prueba de trabajo cubre la raíz de las transacciones desde una altura | Medio: consenso, pruebas, release coordinada de la PC y el teléfono | El de todo fork: quien no actualice queda afuera |

## Recomendación

**A.** Zenith es la testnet y la que va a tener valor, Quantic, nace con las dos
cosas bien. Un fork en Zenith cuesta una release coordinada y no protege nada que
vaya a tener valor; los checkpoints ya cubren lo que importa en una testnet. Si
se quiere cerrar igual la elección de cadena, B es barato y no invalida bloques.
