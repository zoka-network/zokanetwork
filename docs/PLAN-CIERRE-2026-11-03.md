# Cierre acordado el 3/11/2026

Recuperado del plan de Claude `sprightly-swimming-papert.md` y de las
respuestas del usuario. El avance efectivo está en [TRASPASO.md](TRASPASO.md).

## Decisiones vigentes

- Primero nodo/minero 4.0.4 y ZSilent 4.0.6; después ZKAPriv 3.0, vigilante
  y trabajo técnico.
- Windows y el APK se arman en la PC; Linux y el motor Android en el VPS 2.
  No usar WSL ni dejar servicios o minería iniciados en la PC.
- Publicar estas versiones cuando pasen las pruebas y coincidan los hashes:
  el usuario eligió «Publicá directo si pasa». Mostrar los anuncios de Discord
  antes. No instalar ZSilent sin un pedido del usuario.
- Los archivos sin seguimiento señalados en AGENTS.md y
  `scripts/iniciar-zokascan.ps1` quedan afuera.

## 1. Versiones de escritorio y web

Ver [RELEASE-4.0.4.md](RELEASE-4.0.4.md) para pruebas y hashes.
Completar paquetes Windows/Linux y MSI, probar sin tocar billeteras del
usuario, integrar las ramas sin reescribir historial, etiquetar, subir a
GitHub y GitLab, verificar los archivos descargados y actualizar zoka.network.
Conservar la modificación local de `zsilent-core/.../ZSilentAppState.kt`:
el paquete se construye desde un worktree limpio.

## 2. ZKAPriv 3.0

El informe del agente que terminó al cerrarse Claude se recuperó en
`target/zkapriv-informe-claude-3nov.json`. Tres hallazgos prioritarios:

1. `WalletViewModel.publishUntilKnown` usa `isActive` en una función suspendida
   sin un receptor de coroutine; hay que usar el contexto actual. Los cambios
   locales del 31/10 no llegaron a producir un APK nuevo.
2. Los `.so` de ambos ABI son anteriores a la confirmación de checkpoints y
   al formato actual de `/scan2`. El de arm64 tampoco contiene `JNI_OnLoad`
   para inicializar el contexto Android del DNS. Hay que recompilar ambos.
3. El motor usa el nodo por `path = "../.."`: necesita los cambios de
   `red-sin-dependencias`, incluidos `PhonePeers` y el manifiesto con seed3.

Revisar los cambios locales; conectar `/checkpoint` con `confirmCheckpoint`
después de leer por HTTPS; pasar `stateDir` al publicar/resolver; mostrar la
altura verificada y los pares; evitar arranques concurrentes de Tor y
centralizar el estado de publicación de la dirección. No registrar direcciones
ni secretos. Revisar el código heredado v1, la política de privacidad, los
deep links y los ABI empaquetados.

Compilar los motores en Linux y firmar el APK con el keystore existente en la
PC, sin exponerlo. Probar en el teléfono real: crear, recibir desde ZSilent,
devolver y repetir con Tor. **El usuario hace los envíos.** Medir tiempo y
memoria de la prueba STARK, comprobar ambas arquitecturas y la firma del APK.
Publicar 3.0.0 y actualizar la sección Android solo después de esa validación.

## 3. Vigilante y Discord

Preparar `scripts/vps/vigilante.sh` y un timer de systemd: punta sin avanzar,
cero pares, API caída o checkpoint detenido; evitar avisos repetidos.
El usuario crea el webhook y lo guarda en un archivo que se copia sin mostrar
su contenido. Probar el aviso cuando exista ese dato. Preparar el anuncio de
semillas/versiones/minería para que el usuario lo revise y publique.

## 4. Red

- Compartir direcciones `.onion` validadas y filtrar transportes Custom si
  no se usa `SocksDialer`.
- Admitir `--bind` IPv4 e IPv6 con el mismo puerto.
- Evaluar `iroh-mainline-address-lookup` frente a iroh 1.3 y decidir la
  incorporación de la DHT con el usuario.

## 5. Privacidad y poscuántica

- Firma de releases Ed25519 + ML-DSA-65, con dominio propio y una clave de
  releases separada que queda en la PC; comandos de creación, firma y
  verificación. No reutilizar las claves de checkpoints ni leerlas.
- Dandelion++: fase de tallo, capacidad anunciada, sucesor por época y retardos
  aleatorios; pruebas en regtest antes de activar.
- Documento de solidez cuántica del STARK y propuesta de parámetros para
  mainnet. No cambiar el consenso de la testnet. Mantener explícita la revisión
  externa pendiente.
