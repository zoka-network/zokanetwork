# Servidores VPS de Zoka

Preparado el 13/10/2026 para cuando se consigan los servidores. **Nada de esto está
desplegado todavía.** Los tres VPS son ayudas, no piezas de las que dependa la red:
si los tres se apagan, la red sigue andando por P2P y Tor como hoy.

| Rol | Qué corre | Para qué |
|---|---|---|
| `wallet` | Nodo sin wallet (`--sync-only`), onion + TCP público, sirve snapshots por iroh | Que ZKAPriv y ZSilent encuentren siempre un par encendido y sincronicen rápido |
| `explorador` | Nodo sin wallet + `publish.py` cada 5 minutos + Caddy sirviendo JSON estático | Datos del explorador (zokascan.cc) sin exponer la API del nodo |
| `liviano` | Nodo sin wallet, onion + TCP público, sirve snapshots | Semilla redundante y barata |

Los tres siguen la **testnet Zenith** (`--network testnet`). Para Zoka Quantic se repite
el mismo procedimiento con el manifiesto de Quantic cuando exista su génesis.

## Reglas que no se negocian

- **Ningún VPS mina.** Es lo que más prohíben los planes gratis y lo que terminó con
  la organización de GitHub en agosto. Todos corren con `--sync-only`.
- **Ningún VPS guarda una wallet con fondos.** `--sync-only` no abre ni crea wallets.
- **La API del nodo solo escucha en `127.0.0.1`.** Hacia afuera salen el P2P (puerto
  28080) y, en el explorador, Caddy con archivos estáticos.
- Dos proveedores distintos como mínimo, para que una suspensión no los apague a todos.

## Requisitos

- Ubuntu 24.04 LTS, x86_64 o ARM64 (Oracle Always Free es ARM64).
- `wallet` y `liviano`: 1–2 vCPU, 1–2 GB de RAM, 20 GB de disco. RandomX en modo
  liviano usa unos 256 MB para verificar bloques.
- `explorador`: 2 vCPU, 2–4 GB de RAM, 20 GB de disco.
- Puertos de entrada: 22 (SSH), 28080/tcp (P2P) y, en el explorador, 80 y 443.

## 1. Compilar el binario

Desde la PC, en WSL o en un Linux con Rust, en la rama que se vaya a desplegar:

```sh
cargo build --release --bin zoka            # x86_64
# ARM64 (Oracle): compilar en una máquina ARM64 o con el destino aarch64-unknown-linux-gnu
sha256sum target/release/zoka
```

Copiar `target/release/zoka` al VPS junto con esta carpeta (`deploy/vps/`) y
`tools/explorer-publisher/publish.py` si es el explorador.

## 2. Instalar

```sh
sudo ./instalar.sh --rol wallet     --binario ./zoka --ip <IP-publica>
sudo ./instalar.sh --rol liviano    --binario ./zoka --ip <IP-publica>
sudo ./instalar.sh --rol explorador --binario ./zoka --ip <IP-publica> \
     --dominio datos.zokascan.cc --publicador ./publish.py
```

El script instala Tor y crea la onion del nodo, el usuario `zoka` sin shell, el
servicio `zoka-nodo`, el firewall y, en el explorador, Caddy y el timer del
publicador. Es idempotente: correrlo de nuevo actualiza el binario y la configuración.

Al terminar imprime la dirección onion y las dos direcciones para registrar.

## 3. Registrar el servidor en la red

Con las direcciones que imprime el script:

1. **Manifiesto de red** (`docs/mainnet-network.v1.json`, lista de semillas): agregar la
   onion `/onion3/<onion>:28080` y, si se quiere conexión directa, `/dns4/<dominio>/tcp/28080`.
2. **ZKAPriv** (`core/.../Constants.kt`, `ZOKA_P2P_SEEDS`): agregar la onion. La dirección
   TCP directa (`/dns4/…`) sincroniza mucho más rápido pero le muestra al servidor la IP del
   teléfono; dejarla como opción, no por defecto. El motor móvil ya sabe marcar a las dos.
3. **ZSilent**: toma las semillas del manifiesto que viene con el nodo.
4. **Explorador** (`zokaexplorer/public/zoka-network.json`): `"rpc_url": "https://<dominio>"`.
   No hace falta recompilar el sitio, solo volver a publicarlo.
5. **DNS** (Cloudflare, zona `zoka.network` o `zokascan.cc`): un registro A por servidor.
   En el explorador el registro tiene que existir antes de instalar, para que Caddy saque
   el certificado.

Después de cambiar el manifiesto o las apps hay que sacar binarios y APK nuevos.

## 4. Operar

```sh
systemctl status zoka-nodo
journalctl -u zoka-nodo -f
curl -s http://127.0.0.1:3000/sync/status       # desde el propio servidor
systemctl list-timers zoka-explorador-publicar   # solo explorador
```

Actualizar el binario: copiar el nuevo y correr `instalar.sh` con los mismos argumentos.

## Checkpoints autoritativos

Cuando estén implementados (ver la hoja de ruta), estos nodos obedecen los checkpoints
que firma el nodo del equipo, igual que cualquier otro cliente. No guardan la clave de
firma: esa queda solo en la PC del equipo.
