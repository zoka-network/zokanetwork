# Cómo minar ZKA en la mainnet Zenith

Guía rápida para cualquiera que quiera empezar a minar ZKA en la red Zoka — mainnet **Zenith**.

## Lo más fácil: ZSilent (Windows)

ZSilent es la wallet de escritorio oficial. Incluye el nodo completo y el minero integrado — no necesitas instalar nada más.

### 1. Descargar e instalar

- Descarga el instalador desde: **https://github.com/zoka-network/zsilent-core/releases/latest**
- Elegí el `.msi` (recomendado) o el `.exe`.
- Ejecutá el instalador y abrí ZSilent.

### 2. Crear o restaurar tu wallet

- Si es tu primera vez: elegí **Create new** y guardá tu frase de recuperación (12 o 24 palabras) en un lugar seguro y offline. Nadie de Zoka Network te la va a pedir nunca.
- Si ya tenías una wallet: elegí **Import seed** e ingresá tu frase de recuperación exacta.

⚠️ **La frase de recuperación es la única forma de recuperar tus fondos.** Si la perdés, perdés el acceso a tu wallet para siempre. No la compartas con nadie, ni la guardes en la nube sin cifrar.

### 3. Esperar la sincronización

- ZSilent conecta automáticamente con los nodos de la red (BOOT y VPS2 en Digital Ocean).
- En el Dashboard vas a ver "Sync" pasar de "importando bloques" a "OK" cuando tu nodo local alcance la altura de la red.
- La primera sincronización puede tardar — depende de cuántos bloques tenga la cadena en ese momento.

### 4. Activar minería

- En la pantalla de **Mining**, activá el switch.
- El hashrate va a aparecer una vez que el nodo esté sincronizado (no se puede minar antes de estar al día con la red).

### 5. Ver tus recompensas

- Cada bloque que minás te da 12 ZKA, pero quedan **inmaduros** (bloqueados) por 60 bloques (~2 horas a 120s por bloque).
- Pasado ese tiempo, el monto pasa a tu balance disponible automáticamente.
- Podés verificar tu progreso en el explorador público: **https://www.zokascan.cc**

---

## Verificación de seguridad (recomendado)

Antes de minar en serio, confirmá que tenés el software correcto:

- Los parámetros de la ceremonia (trusted-setup) usados por la red están publicados con su hash SHA-256 en:
  **https://github.com/zoka-network/zokanetwork/releases/tag/trusted-setup-mainnet-v1**
- ZSilent ya incluye estos archivos empaquetados — no necesitás descargarlos por separado.

---

## Avanzado: correr un nodo Linux propio (sin ZSilent)

Para minería headless en un servidor/VPS Linux, en vez de la wallet de escritorio:

1. Compilá el binario desde el código fuente (no hay un binario Linux público pre-compilado todavía):
   ```sh
   git clone https://github.com/zoka-network/zokanetwork.git
   cd ZokaNetwork
   cargo build --release --features randomx --bin zoka
   ```
2. Descargá los archivos de la ceremonia (trusted-setup) desde el release linkeado arriba y colocalos en `/opt/zoka/trusted-setup/`.
3. Arrancá el nodo apuntando a la mainnet, con tu propia dirección de wallet para recibir las recompensas:
   ```sh
   ./target/release/zoka fullnode \
     --network mainnet \
     --mine-interval-secs 30 --mine-blocks 1 --min-mine-peers 1 \
     --address <tu-direccion-zka1> \
     --password-env ZOKA_NODE_PASSWORD
   ```
   El nodo descubre automáticamente los peers de la red a partir del manifiesto de seeds (`docs/mainnet-network.v1.json`).
4. Las recompensas van directo a tu wallet — no hay pool ni intermediario.

> Esta sección es para usuarios técnicos. La mayoría de los mineros deberían usar ZSilent.

---

## Preguntas frecuentes

**¿Necesito una GPU?** No. ZOKA usa RandomX, diseñado para CPU. Las GPUs no tienen ventaja.

**¿Cuánto tarda en madurar una recompensa?** 60 bloques, aproximadamente 2 horas.

**¿Dónde reporto un problema?** Abrí un issue en `https://github.com/zoka-network/zokanetwork/issues` o `https://github.com/zoka-network/zsilent-core/issues` según corresponda.
