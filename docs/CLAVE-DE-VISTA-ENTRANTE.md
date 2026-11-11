# Clave de vista de entrantes

10/11/2026, rama `endurecimiento-sin-consenso`. No toca el consenso.

## Qué es

Hasta ahora una billetera tenía una sola clave de vista, y era todo o nada:
quien la tiene ve lo recibido, lo enviado, el saldo y toda la historia.

La clave de entrantes es la mitad de esa clave. Ve **lo que llega** a la
billetera, con monto y memo. No ve lo que la billetera gasta ni su saldo, y
no puede gastar. Sirve para darle a quien tiene que saber qué se cobró y
nada más: un contador, o la página de cobro de un comercio que solo necesita
detectar pagos.

| | Dirección | Clave de entrantes | Clave de vista | 24 palabras |
| --- | --- | --- | --- | --- |
| Recibir pagos | sí | | | |
| Ver lo recibido | | sí | sí | sí |
| Ver lo enviado y el saldo | | | sí | sí |
| Gastar | | | | sí |

## Cómo es

- Lleva el dueño de las notas y la semilla X-Wing de la billetera. Le falta
  la clave de salida, que es la que abre lo enviado
  ([wallet.rs](../crates/zoka-pq-pool/src/wallet.rs), `IncomingViewKey`).
- Empieza con `zqviewin2` (mainnet), `zqtestviewin2` (testnet) o
  `zqregtestviewin2`. Ninguna dirección ni clave de vista completa empieza
  así, y cada texto se rechaza donde va otro.
- De la clave completa sale la de entrantes (`ViewKey::incoming`). Al revés
  no se puede.

## El vuelto

El vuelto de un pago vuelve a la dirección de la billetera como cualquier
pago. Sin más, la clave de entrantes lo contaría como plata cobrada: pagar 10
con una nota de 100 mostraría un «ingreso» de 90.

Por eso la billetera marca su vuelto. El memo de una salida que vuelve al
emisor sin memo lleva una etiqueta de 32 bytes: el hash del `rho` de esa
salida bajo la semilla de vista (`change_memo`).

- La calculan la billetera y sus claves de vista, y nadie más. Quien paga no
  puede disfrazar un pago de vuelto.
- La de una salida no sirve en otra.
- Su primer byte es cero: como texto es un memo vacío, así que las billeteras
  que ya existen no muestran nada raro.
- Va dentro del sobre cifrado: en la cadena no se ve.

La marca la pone `transaction::transfer`, por donde pasan todas las
billeteras, así que ninguna necesita código propio. **El vuelto de una
billetera anterior a este cambio no está marcado** y la clave de entrantes lo
lee como un pago más. Se corrige solo a medida que se actualizan.

## Cómo leer con ella

`IncomingViewer` ([scan.rs](../crates/zoka-pq-pool/src/scan.rs)) lee las
mismas páginas de `/scan` que la clave completa y devuelve:

- `received_atoms`: todo lo que llegó menos el vuelto propio;
- `change_atoms`: el vuelto que volvió;
- `immature_atoms`: recompensas de bloque que todavía no maduraron;
- la lista de notas, cada una con altura, txid, monto, memo y si es vuelto.

No hay saldo: la clave no ve los gastos.

## Cómo sacarla

```powershell
zoka quantic-pq-wallet show-view-key --network testnet --dir <carpeta> --name <billetera> --password-env VAR --incoming
```

Sin `--incoming` devuelve la clave completa, como antes. La respuesta en JSON
suma el campo `kind` (`incoming` o `full`).

## Qué falta

- **Las aplicaciones.** ZSilent, la billetera web y ZKAPriv muestran solo la
  clave completa, y ZokaScan solo lee esa. Sumar la de entrantes es interfaz
  en cada repo y pide publicar versiones nuevas.
- **El lector en el navegador** (`crates/zoka-pq-scan-wasm`) todavía no
  acepta esta clave.
- **Direcciones distintas por contraparte.** La clave de entrantes separa
  quién ve qué; no evita que dos personas con tu dirección sepan que sos la
  misma. Eso es otro trabajo.
