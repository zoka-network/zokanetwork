# Decisiones del 13/10/2026

El usuario delegó estas decisiones en Claude ("tomalas tú"), salvo los VPS, que
quedan preparados para cuando se consigan. Rumbo confirmado por el usuario el
mismo día:

- **Zenith es la testnet.** Sus ZKA no tienen valor comercial.
- **Zoka Quantic es la mainnet nueva**, lanzada de cero. Sus ZKA son los que
  tendrán valor, a futuro.
- **La PC del usuario es el nodo autoritario** de las dos redes: los demás nodos
  obedecen los checkpoints que firma (`docs/CHECKPOINTS-AUTORITATIVOS.md`).

## 1. Formato del ciphertext en el JSON → queda como arreglo de bytes

`kem_ciphertext` (1.120 bytes por salida v2) se serializa igual que los otros
campos binarios de la salida (`ephemeral_pubkey`, `range_proof`,
`encrypted_amount`): un arreglo de números. Se evaluó pasarlo a hex (unos 2,2 KB
por salida en lugar de ~4 KB) y se descartó:

- El hash de la transacción se calcula sobre los bytes, no sobre el JSON: el
  formato no cambia la identidad de la transacción.
- El cuerpo canónico (`serde_json::to_vec`) tiene un tope de 1 MiB por
  transacción y 8 MiB por bloque; ~1,8 KB más por salida no se acerca a ninguno.
- Cambiar un solo campo lo deja distinto de sus vecinos y obliga a tocar cuatro
  estructuras del nodo y el modelo de ZKAPriv justo antes de activar.

Lo que importa es que el formato quede fijo antes de la activación: cambiarlo
después rompería a los nodos que no actualicen. Queda fijo así. Quantic, que
arranca de cero, puede definir desde el génesis un cuerpo binario compacto para
todos los campos (pendiente previo al génesis).

## 2. Alturas de activación en Zenith → con la próxima versión de testnet

Los bundles canónicos y las salidas v2 se activan en Zenith con la **próxima
versión de testnet** (2.1.0), no antes: activar exige que los demás nodos
actualicen, y para eso tiene que haber binarios publicados. Al compilar esa
versión se fijan, con la punta y el ritmo de bloques medidos ese día:

- bundles canónicos: la punta + una semana de bloques (5.040 al objetivo de
  120 s por bloque);
- salidas v2 (zkq1): un día después (720 bloques más al mismo ritmo).

La semana deja tiempo a que los nodos actualicen; el día entre una y otra
permite ver los bundles andando antes de sumar la v2. Un nodo que no actualice
queda en su rama sin arrastrar a nadie: los que sí actualizaron siguen los
checkpoints firmados.

## 3. Pruebas reales entre ZSilent y ZKAPriv → en la testnet, después de activar

Probar envíos a una zkq1 entre las dos apps exige la activación. Una red local con
el teléfono exigiría un motor móvil compilado solo para eso. Se prueban en Zenith
apenas se active la v2, con la versión 2.1.0 instalada en la PC y el teléfono.

## 4. Nodo autoritario → instalado en la PC del usuario

La clave de Zenith se creó en la PC del usuario
(`%APPDATA%\Zoka\.zoka\mainnet\checkpoint-authority.key`, pública
`bf5c92154ee0e250f4419d47cf0a16c263c586244274ccad196234d339c47bd4`) y está
compilada en el nodo. El `zoka.exe` nuevo va en ZSilent: con la clave presente
firma un checkpoint cada 30 bloques, y solo con pares conectados que confirman su
punta. El usuario dijo "sigue tú solo", pero el clasificador de permisos frenó
dos veces el reemplazo del ejecutable instalado; queda
`scripts/instalar-en-pc.ps1` para que lo corra él con ZSilent cerrado (instala
también Tor 0.4.9.14). Los demás nodos lo obedecen desde que corran un binario con
la clave compilada (2.1.0). **El usuario tiene que respaldar ese archivo fuera de
la PC:** es la única copia.

Quantic tendrá su propia clave, creada al fijar su génesis.

## 5. Anuncio y releases → con la 2.1.0

El anuncio de Zenith como testnet y los releases en `github.com/zoka-network` salen
juntos con la 2.1.0, cuando haya binarios que ofrecer. La web ya presenta a Zenith
como testnet y a Quantic como la mainnet en desarrollo.
