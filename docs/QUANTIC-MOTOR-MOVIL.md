# Motor del teléfono para Zoka Quantic

17/10/2026. Para quien conecte la interfaz de ZKAPriv. Rama del nodo
`etapa5-quantic-nodo` (`src/mobile_quantic.rs`, `src/mobile_net.rs`); puente JNI
en ZKAPriv, rama `quantic-motor` (`native/src/quantic.rs`, feature `quantic`).
**La interfaz no está hecha**: qué pantallas cambian es decisión del usuario.

## Qué hace

- Deriva la wallet de Quantic de la **misma semilla** que la app ya tiene
  (`seed_hex`, la raíz de la frase): otra wallet, sin relación con la de Zenith.
  La misma frase en `zoka quantic-wallet restore` da la misma dirección.
- Habla con los nodos por P2P (`/zoka-quantic/light/1`), por Tor, sin servidores.
- Escanea bloques compactos verificando cada uno contra su `state_root`: un nodo
  no puede esconder ni cambiar una salida (A5). Lo que no verifica es la prueba de
  trabajo de los encabezados.
- Arma y prueba los envíos en el teléfono: las claves no salen de él.

## Compilar

```powershell
native\build-android.ps1 -Features "pq-experimental,quantic"
```

Sin `-Features`, el script compila lo de siempre (los releases actuales). El
motor con Quantic agrega `orchard` y Halo 2 al `.so`.

## Flujo

1. **Al abrir la app**, en un hilo de fondo: `quanticPreloadProvingKey()`. Arma la
   clave de prueba (unos 40 s en el teléfono de pruebas la primera vez; después
   no cuesta nada). Hace falta para enviar, no para ver el saldo.
2. **Conectar** con `connect0` como siempre, agregando la red y semillas de esa red:

   ```json
   {"seeds": ["<multiaddr de un nodo de Quantic>"], "tor_proxy": "<lo que devuelve startTor0>",
    "data_dir": "<directorio privado de la app>", "network": "quantic-testnet"}
   ```

   La red se fija **una vez por proceso**: para volver a Zenith hay que reiniciar
   la app. Si ya estaba conectada a otra red, contesta error: primero
   `disconnect0`.
3. **Altura**: `quanticLight0({"kind":"info"})` →
   `{"kind":"info","chain_id":"zoka-quantic-testnet-v1","height":N,"tip":"...","tree_root":"..."}`.
4. **Sincronizar** de a lotes de hasta 500 bloques, desde `current.next_height`
   (la primera vez, desde 1):

   ```text
   quanticLight0({"kind":"compact","from":F,"to":T})   → {"kind":"compact","blocks":[...]}
   quanticSync({"seed_hex":S,"network":"quantic-testnet","state_hex":C,
                "last_hash":H,"tip_height":N,"blocks":[...]})
   ```

   Respuesta:

   ```json
   {"current": {"state_hex": "...", "next_height": 1234, "last_hash": "..."},
    "safe": {"state_hex": "...", "next_height": 1114, "last_hash": "..."},
    "balance_atoms": 25000000, "immature_atoms": 0,
    "history": [{"txid": "...", "height": 1200, "coinbase": false,
                 "received_atoms": 0, "spent_atoms": 25000200, "net_atoms": -25000200}],
    "blocks_scanned": 500, "scan_ms": 1800}
   ```

   - Guardar `current` siempre y `safe` cuando viene (es el estado 120 bloques
     detrás de la punta).
   - Si la llamada falla con un error que empieza con `reorg:`, la cadena se
     reorganizó: volver a sincronizar desde `safe`.
   - La primera vez: sin `state_hex` ni `last_hash`.
5. **Enviar**:

   ```text
   quanticCheckAddress({"address":"zqtest1...","network":"quantic-testnet"})  → {"valid":true}
   quanticBuildTransfer({"seed_hex":S,"network":"quantic-testnet","state_hex":C,
                         "to":"zqtest1...","amount_atoms":25000000,"memo_text":"gracias"})
     → {"tx_hex":"...","txid":"...","fee_atoms":200,"actions":2,"build_ms":20000}
   quanticLight0({"kind":"submit_tx","tx_hex":"..."})
     → {"kind":"submitted","accepted":true,"txid":"...","reason":null}
   ```

   `quanticBuildTransfer` tarda unos 20 s: siempre en un hilo de fondo. El estado
   tiene que estar al día con la punta (sincronizar justo antes).

Los errores vuelven como `{"error": "..."}`.

## Guardar los estados

`state_hex` contiene la **clave de vista**: quien lo tiene ve toda la historia
de la wallet. Se guarda cifrado, igual que la semilla. Con el estado guardado la
app no vuelve a escanear desde el génesis; sin él, empieza de cero (y llega igual,
más despacio).

## Límites conocidos

- **Dos envíos seguidos:** las notas que usa un envío siguen como no gastadas en
  el estado hasta que un bloque lo incluye. Si la app arma otro antes, puede
  elegir las mismas y el nodo rechaza el segundo. Esperar la confirmación (que
  `quanticSync` lo muestre en `history`) antes del siguiente.
- **Memos:** el escaneo compacto no los trae. Mostrar el memo de un pago exige
  bajar la transacción entera (pendiente).
- **Encabezados:** el motor no verifica la prueba de trabajo (RandomX no entra en
  un teléfono): confía en los nodos para la cadena, y verifica todo lo demás. En la
  testnet el génesis va fijo en el binario y se comprueba.
- **Medir en el teléfono:** el escaneo con X-Wing (unos 4 ms por salida medidos en
  la etapa 3) y el envío dentro de la app.

## Pruebas

- `quantic::tests::the_phone_engine_follows_a_real_chain_spends_and_resumes`:
  el motor sobre una cadena regtest real (sincroniza desde el génesis, envía, el
  mempool lo admite, sigue desde el estado guardado, detecta la reorganización y
  rechaza estados ajenos o de otra red).
- `network::p2p::tests::un_telefono_pide_los_datos_de_quantic_a_un_nodo_por_p2p`:
  el protocolo entre nodos reales por TCP.
- `a_saved_wallet_resumes_where_it_stopped` (biblioteca): el estado guardado con
  pruebas Halo 2 reales.
