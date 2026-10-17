# Propuesta: Zoka Quantic en ZSilent y ZKAPriv

17/10/2026. Para que el usuario elija antes de tocar las apps. **No hay nada
construido en las pantallas.** Lo que ya existe debajo:

- **ZSilent** puede usar la wallet de línea de comandos: `zoka quantic-wallet
  new | restore | address | balance | history | send`, todas con `--json`, la
  contraseña por `--password-env` o stdin y la frase de `restore` por
  `--phrase-env` (nunca como argumento), y `send --memo`. El nodo:
  `zoka fullnode --network quantic-testnet` (carpeta y puertos propios).
- **ZKAPriv** tiene el motor y su puente JNI (rama `quantic-motor`):
  conectar a la red Quantic, sincronizar por bloques compactos, armar y publicar
  envíos ([QUANTIC-MOTOR-MOVIL.md](QUANTIC-MOTOR-MOVIL.md)), probado de punta a
  punta contra un nodo real.

Antes que cualquier pantalla hace falta la testnet de Quantic abierta (génesis con
la wallet del equipo y la PC como semilla): sin eso no hay red a la que conectar.

## Opciones

| | Qué ve el usuario | Trabajo | Riesgo |
|---|---|---|---|
| **A. Quantic reemplaza a Zenith** | Las apps pasan a ser de Quantic testnet; Zenith queda en las versiones anteriores y en la línea de comandos | El menor: se cambian las pantallas de wallet y nodo, no se agrega un modo | Quien quiera seguir en Zenith usa la versión anterior |
| **B. Selector de red** | Un ajuste para elegir Zenith o Quantic | Más: dos caminos en cada pantalla y en el nodo | Confusión de red; ya pasó que un selector inventado rompió un flujo que andaba |
| **C. Las dos a la vez** | Dos wallets y dos nodos en la misma app | El mayor | Doble consumo de batería y datos en el teléfono |

## Recomendación

**A**, al abrir la testnet de Quantic. Sigue el rumbo del 12/10 ("ZSilent ya no
es más Zenith, pasa a ser Quantic") y Zenith es la testnet vieja.

## Qué cambiaría con A

**ZSilent**

- Alta y restauración con `quantic-wallet new/restore` (la misma frase de 24
  palabras de siempre da otra wallet, de Quantic).
- Saldo, saldo por madurar e historia con `balance --json` y `history --json`.
- Recibir: la dirección `zqtest1…` (unos 1.730 caracteres) con el QR animado en
  partes que ya existe y copiar.
- Enviar: dirección, monto y memo opcional; la comisión es fija (0,0001 ZKA por
  acción), así que no hay selector de comisión. Tarda unos segundos en la PC
  (la prueba Halo 2).
- Nodo y minería: `fullnode --network quantic-testnet --quantic-mine-to <dirección>`.
- El explorador interno, con los datos de `/quantic/blocks` (alturas, horas,
  cantidad de transacciones; los montos son privados).

**ZKAPriv**

- Conectar con `"network": "quantic-testnet"` y las semillas de Quantic.
- Al abrir, armar la clave de prueba en segundo plano (40 s la primera vez en el
  teléfono de pruebas).
- Sincronizar con `quanticLight0` + `quanticSync`, guardando cifrados el estado
  actual y el seguro.
- Enviar: `quanticCheckAddress`, `quanticBuildTransfer` (unos 20 s, con una
  barra de progreso) y `quanticLight0` con `submit_tx`.
- Recibir: el QR animado de la dirección.

## Lo que hace falta que decida el usuario

1. A, B o C.
2. Si se muestra el memo de los pagos recibidos: hoy el escaneo liviano no lo
   trae y bajar la transacción entera le dice al nodo cuál interesa.
