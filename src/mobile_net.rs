//! Cliente de red para la wallet movil: hablar con la red sin ser un nodo.
//!
//! ZKAPriv usaba una API REST contra nodos centrales. Esos nodos se dieron de
//! baja y la wallet quedo muerta: apuntaba a dos IPs que ya no existen. Este
//! modulo la conecta a la red igual que cualquier otro participante, sin
//! depender de que nadie mantenga un servidor.
//!
//! **Lo que hace y lo que no.** Publica transacciones por gossip y pide
//! bloques por el protocolo punto a punto para que la wallet escanee sus
//! notas. No mina, no valida la cadena entera y no guarda bloques: los pide,
//! el escaner los mira y se descartan. El telefono nunca almacena la cadena.
//!
//! **Por que no hay codigo de red nuevo.** El crate del nodo ya se compila
//! dentro del `.so` de Android -- solo con RandomX apagado --, asi que libp2p,
//! gossipsub y el protocolo de bloques ya viajaban en el APK sin estar
//! expuestos. Esto es una fachada sobre lo que ya estaba.
//!
//! **Tor.** El nodo alcanza onions por un proxy SOCKS5 configurable. En el
//! telefono ese proxy lo levanta Arti (Tor en Rust) dentro del mismo proceso,
//! asi que no hace falta instalar ninguna otra aplicacion.

use crate::network::message::ZokaMessage;
use crate::network::p2p::Node;
use crate::private_wallet::PrivateTransaction;
use serde::{Deserialize, Serialize};
use std::sync::mpsc as std_mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;

/// Cuanto se deja respirar al bucle de eventos entre comandos.
///
/// libp2p solo avanza cuando alguien lo bombea: los apretones de manos, los
/// anuncios de gossip y las respuestas entrantes ocurren dentro de `pump_for`.
/// Un valor corto mantiene la red viva sin quemar bateria, que en un telefono
/// importa mas que en un servidor.
const PUMP_SLICE: Duration = Duration::from_millis(250);

/// Cuanto esperar a que aparezca el primer par antes de dar por fallida una
/// conexion.
///
/// Generoso a proposito: un circuito Tor recien armado tarda decenas de
/// segundos, y en una red movil mas todavia. Fallar antes de tiempo obligaria
/// al usuario a reintentar cuando solo hacia falta esperar.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(90);

/// Cuanto esperar un anuncio de bloque antes de admitir que no sabemos la
/// altura de la red.
///
/// La altura se aprende escuchando: los pares anuncian cada bloque que aceptan.
/// No hay forma de preguntarla -- el protocolo de cabeceras responde desde el
/// genesis, no desde la punta --, asi que hay que esperar a que caiga uno. A
/// dos bloques por minuto, dos minutos cubren varios intentos.
const HEIGHT_WAIT: Duration = Duration::from_secs(120);

/// Cuanto esperar las respuestas de un lote de bloques.
const BLOCKS_WAIT: Duration = Duration::from_secs(60);

/// Cuantos bloques pedir de una. El escaner pide rangos de 250, pero cada
/// bloque es un pedido punto a punto: mandarlos todos juntos abre 250 flujos
/// simultaneos sobre un circuito Tor, que es justo como se pierde una tanda
/// entera. De a 32 el rango tarda un poco mas y llega.
const BLOCKS_BATCH: usize = 32;

/// Techo del cache de cuerpos de transacciones privadas.
///
/// Los cuerpos llegan de arriba con los bloques y se sirven al escaner sin otra
/// vuelta a la red. Sin techo, un escaneo largo los acumularia todos en la
/// memoria del telefono.
const MAX_CACHED_BODIES: usize = 4096;

#[derive(Debug, Clone, Deserialize)]
pub struct MobileNetConfig {
    /// Direcciones a las que marcar al arrancar. Onions del manifiesto o las
    /// que el usuario haya agregado a mano.
    pub seeds: Vec<String>,
    /// Proxy SOCKS5 para alcanzar onions, normalmente el de Arti en este mismo
    /// proceso.
    #[serde(default)]
    pub tor_proxy: Option<String>,
    /// Directorio propio de la aplicacion donde el nodo puede escribir.
    ///
    /// Hace falta decirselo. El nodo guarda su identidad de red en
    /// `.zoka/node_key` **relativa al directorio de trabajo**, que en un
    /// servidor es su propio directorio pero en Android es `/`, de solo
    /// lectura: sin esto el nodo ni arranca.
    #[serde(default)]
    pub data_dir: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MobileNetStatus {
    pub connected: bool,
    pub peers: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Cuantos pares tiene el nodo, publicado fuera del canal de comandos.
///
/// El estado no viaja por el canal a proposito. Pedir bloques o esperar la
/// altura ocupa el hilo del nodo durante minutos, y si preguntar "hay red?"
/// tuviera que hacer cola detras de eso, dos pantallas refrescando a la vez se
/// romperian entre si: la segunda veria "no responde" y concluiria que se
/// perdio la conexion justo cuando la primera la esta usando.
static PARES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn anotar_pares(n: usize) {
    PARES.store(n, std::sync::atomic::Ordering::Relaxed);
}

enum Cmd {
    PublishPrivateTx(Box<PrivateTransaction>, std_mpsc::Sender<Result<(), String>>),
    Height(std_mpsc::Sender<Result<(u64, u64), String>>),
    Blocks(u64, u64, std_mpsc::Sender<Result<String, String>>),
    PrivateTxBody(String, std_mpsc::Sender<Result<String, String>>),
    Shutdown,
}

struct Handle {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    /// El hilo dedicado donde vive el nodo. Se guarda para que el runtime no se
    /// caiga mientras haya comandos en vuelo.
    _hilo: std::thread::JoinHandle<()>,
}

static HANDLE: OnceLock<Mutex<Option<Handle>>> = OnceLock::new();

fn handle_slot() -> &'static Mutex<Option<Handle>> {
    HANDLE.get_or_init(|| Mutex::new(None))
}

/// Levanta el cliente y marca a las semillas. Idempotente: si ya estaba
/// arriba, devuelve el estado actual en vez de arrancar un segundo nodo -- dos
/// nodos en el mismo proceso pelearian por la misma identidad de red.
pub fn connect_json(config_json: &str) -> Result<String, String> {
    let config: MobileNetConfig =
        serde_json::from_str(config_json).map_err(|e| format!("config invalida: {e}"))?;
    if config.seeds.is_empty() {
        return Err("no hay semillas a las que conectarse".to_string());
    }

    let mut slot = handle_slot().lock().map_err(|_| "estado envenenado".to_string())?;
    if slot.is_some() {
        drop(slot);
        return status_json();
    }

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (listo_tx, listo_rx) = std_mpsc::channel();
    let seeds = config.seeds.clone();
    let tor_proxy = config.tor_proxy.clone();
    let identity_path = config
        .data_dir
        .as_ref()
        .map(|dir| std::path::PathBuf::from(dir).join("zoka-red").join("node_key"));

    // El nodo vive en un hilo propio con su runtime de un solo hilo, no en un
    // pool compartido.
    //
    // No es una preferencia: el nodo guarda devoluciones de llamada que son
    // `Send` pero no `Sync`, asi que su futuro no puede moverse entre hilos y
    // `spawn` sobre un runtime multi-hilo ni siquiera compila. Darle un hilo
    // fijo tambien le sienta mejor a un telefono -- una sola cosa haciendo red,
    // sin robarle nucleos al prover ni a la interfaz.
    let hilo = std::thread::Builder::new()
        .name("zoka-mobile-net".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = listo_tx.send(Err(format!("no pude crear el runtime: {e}")));
                    return;
                }
            };
            rt.block_on(run_client(seeds, tor_proxy, identity_path, cmd_rx, listo_tx));
        })
        .map_err(|e| format!("no pude crear el hilo de red: {e}"))?;

    match listo_rx.recv_timeout(CONNECT_TIMEOUT) {
        Ok(Ok(())) => {
            *slot = Some(Handle { cmd_tx, _hilo: hilo });
            drop(slot);
            status_json()
        }
        Ok(Err(err)) => Err(err),
        Err(_) => Err(format!(
            "ningun par respondio en {} segundos",
            CONNECT_TIMEOUT.as_secs()
        )),
    }
}

/// Estado actual: si hay red y con cuantos pares.
///
/// Se responde al instante leyendo lo que el hilo de red dejo anotado, sin
/// mandarle nada. Preguntar por el estado nunca puede quedarse esperando a que
/// termine una descarga de bloques.
pub fn status_json() -> Result<String, String> {
    let arriba = handle_slot()
        .lock()
        .map_err(|_| "estado envenenado".to_string())?
        .is_some();
    let pares = if arriba {
        PARES.load(std::sync::atomic::Ordering::Relaxed)
    } else {
        0
    };
    serde_json::to_string(&MobileNetStatus {
        connected: arriba && pares > 0,
        peers: pares,
        error: None,
    })
    .map_err(|e| e.to_string())
}

/// Publica una transaccion privada ya construida y probada localmente.
///
/// La wallet arma y prueba la transaccion en el telefono; esto solo la pone en
/// la red. Los mineros la validan como a cualquier otra: este cliente no pide
/// ni recibe ningun trato especial.
pub fn publish_private_tx_json(tx_json: &str) -> Result<String, String> {
    let tx: PrivateTransaction =
        serde_json::from_str(tx_json).map_err(|e| format!("transaccion invalida: {e}"))?;

    let slot = handle_slot().lock().map_err(|_| "estado envenenado".to_string())?;
    let handle = slot
        .as_ref()
        .ok_or_else(|| "la red no esta conectada".to_string())?;

    let (tx_resp, rx_resp) = std_mpsc::channel();
    handle
        .cmd_tx
        .send(Cmd::PublishPrivateTx(Box::new(tx), tx_resp))
        .map_err(|_| "el cliente de red se detuvo".to_string())?;
    drop(slot);

    match rx_resp.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(())) => Ok(r#"{"published":true}"#.to_string()),
        Ok(Err(err)) => Err(err),
        Err(_) => Err("la publicacion no confirmo a tiempo".to_string()),
    }
}

/// Corta la red y libera el nodo. La wallet la llama al pasar a segundo plano:
/// mantener circuitos Tor abiertos con la pantalla apagada gasta bateria sin
/// que nadie mire el resultado.
/// Manda un comando al hilo de red y espera su respuesta.
fn pedir<T>(
    armar: impl FnOnce(std_mpsc::Sender<T>) -> Cmd,
    espera: Duration,
) -> Result<T, String> {
    let slot = handle_slot().lock().map_err(|_| "estado envenenado".to_string())?;
    let handle = slot.as_ref().ok_or("la red no esta conectada")?;
    let (tx, rx) = std_mpsc::channel();
    handle
        .cmd_tx
        .send(armar(tx))
        .map_err(|_| "el hilo de red murio".to_string())?;
    drop(slot);
    rx.recv_timeout(espera)
        .map_err(|_| "el hilo de red no respondio a tiempo".to_string())
}

/// Hasta donde puede leer la cadena este cliente.
///
/// Devuelve `{"height":N,"announced_height":M}`, donde `N` es lo que los pares
/// conectados pueden entregar de verdad y `M` lo que la red anuncia. Suelen
/// coincidir; cuando no, la wallet tiene que escanear hasta `N` -- pedir `M` le
/// falla entero por unos bloques que su par no tiene.
///
/// Puede tardar recien conectado: hasta que no pase un bloque nuevo nadie
/// anuncia nada, y despues hay que tantear hasta donde llega el par.
pub fn chain_height_json() -> Result<String, String> {
    let (servible, anunciada) = pedir(Cmd::Height, HEIGHT_WAIT + Duration::from_secs(90))??;
    Ok(format!(
        r#"{{"height":{servible},"announced_height":{anunciada}}}"#
    ))
}

/// Bloques crudos de `from` a `to`, en el mismo formato que devolvia la API
/// REST para que el escaner no note la diferencia.
///
/// **Confianza.** Este es un cliente liviano: no valida la prueba de trabajo ni
/// reconstruye la cadena, asi que se cree lo que le mandan. No es peor que
/// antes -- la version REST le creia a un unico servidor central --, pero
/// conviene decirlo: aca los bloques se piden repartidos entre los pares
/// conectados, no a uno solo.
pub fn blocks_range_json(from: u64, to: u64) -> Result<String, String> {
    if from > to {
        return Err(format!("rango invalido: {from} > {to}"));
    }
    let espera = BLOCKS_WAIT * (((to - from) as u32 / BLOCKS_BATCH as u32) + 2);
    pedir(
        |tx| Cmd::Blocks(from, to, tx),
        espera + Duration::from_secs(15),
    )?
}

/// Cuerpo de una transaccion privada por su hash, en JSON.
///
/// Primero mira el cache que dejaron los bloques ya bajados; solo sale a la red
/// si el cuerpo no vino con su bloque.
pub fn private_tx_body_json(tx_hash: &str) -> Result<String, String> {
    pedir(
        |tx| Cmd::PrivateTxBody(tx_hash.to_string(), tx),
        BLOCKS_WAIT + Duration::from_secs(15),
    )?
}

pub fn disconnect() -> Result<String, String> {
    let mut slot = handle_slot().lock().map_err(|_| "estado envenenado".to_string())?;
    if let Some(handle) = slot.take() {
        let _ = handle.cmd_tx.send(Cmd::Shutdown);
    }
    anotar_pares(0);
    Ok(r#"{"connected":false}"#.to_string())
}


/// Publica una transaccion privada por el mismo camino que usa el nodo
/// completo, que son tres pasos y no uno.
///
/// La red no difunde la transaccion entera y ya: publica un ANUNCIO con el
/// hash, y los pares que lo ven piden el cuerpo punto a punto. Por eso hay que
/// dejar el cuerpo en cache ANTES de anunciar -- si no, quien pida ese hash se
/// encuentra con que el unico que lo tiene no se lo puede dar, y la
/// transaccion muere sin llegar a ningun minero.
///
/// Despues se publica tambien la transaccion completa, que es lo que hace el
/// nodo: cubre a los pares que la reciben directo sin pasar por el anuncio.
async fn publicar_tx_privada(node: &mut Node, tx: &PrivateTransaction) -> Result<(), String> {
    let cuerpo = serde_json::to_vec(tx).map_err(|e| format!("no pude serializar la tx: {e}"))?;
    let msg: crate::network::message::PrivateTransactionMessage = serde_json::to_value(tx)
        .and_then(serde_json::from_value)
        .map_err(|e| format!("no pude convertir la tx al mensaje de red: {e}"))?;

    node.cache_private_tx_body(&msg.tx_hash, cuerpo.clone());

    let anuncio = crate::network::message::PrivateTxAnnounceMessage::from_private_tx(
        &msg,
        Some(cuerpo.len() as u64),
    );

    // Reintentar bombeando entre intentos: gossipsub rechaza publicar mientras
    // el tema todavia no tiene pares en la malla, y sobre Tor eso tarda unos
    // segundos en armarse despues de conectar.
    node.publish_message_with_retry(&ZokaMessage::PrivateTxAnnounce(anuncio), 10, PUMP_SLICE)
        .await
        .map_err(|e| format!("no pude anunciar la tx: {e}"))?;

    // La transaccion completa es el segundo camino, no el principal: si el
    // anuncio llego, la red ya puede pedir el cuerpo. Un fallo aca no invalida
    // la publicacion.
    let _ = node
        .publish_message_with_retry(&ZokaMessage::PrivateTransaction(msg), 5, PUMP_SLICE)
        .await;
    Ok(())
}

/// Cuerpos de transacciones privadas que llegaron pegados a sus bloques.
///
/// El escaner necesita el cuerpo de cada transaccion confirmada. Como el
/// bloque ya los trae, guardarlos evita una vuelta a la red por cada uno --
/// sobre Tor, la diferencia entre un escaneo de minutos y uno de horas.
#[derive(Default)]
struct CacheCuerpos {
    por_hash: std::collections::HashMap<String, Vec<u8>>,
    orden: std::collections::VecDeque<String>,
}

impl CacheCuerpos {
    fn guardar(&mut self, hash: String, cuerpo: Vec<u8>) {
        if self.por_hash.contains_key(&hash) {
            return;
        }
        while self.orden.len() >= MAX_CACHED_BODIES {
            if let Some(viejo) = self.orden.pop_front() {
                self.por_hash.remove(&viejo);
            }
        }
        self.orden.push_back(hash.clone());
        self.por_hash.insert(hash, cuerpo);
    }

    fn buscar(&self, hash: &str) -> Option<&Vec<u8>> {
        self.por_hash.get(hash)
    }
}

/// Cuanto esperar un bloque suelto mientras se tantea hasta donde llega un par.
///
/// Corto a proposito: se hacen varios sondeos seguidos y lo que interesa es si
/// el par tiene el bloque, no exprimirle una respuesta lenta.
const PROBE_WAIT: Duration = Duration::from_secs(20);

/// Si un par tiene realmente el bloque de esa altura.
async fn tiene_bloque(node: &mut Node, altura: u64) -> bool {
    let respuestas = node.request_blocks_batch_direct(&[altura], PROBE_WAIT).await;
    matches!(respuestas.get(&altura), Some(Ok(_)))
}

/// Hasta donde pueden servir de verdad los pares conectados.
///
/// No es lo mismo que la altura anunciada, y la diferencia importa. Gossip
/// propaga los anuncios por toda la malla: un par atrasado reenvia la punta que
/// mino otro sin tener el bloque. Si la wallet escaneara hasta lo anunciado,
/// fallaria entera por unos bloques que nadie a su alcance puede darle --
/// medido contra la red real, la semilla anunciaba 17325 y solo servia hasta
/// 17061.
///
/// Un saldo calculado hasta unos bloques atras es lo normal en cualquier
/// wallet; no poder mostrar saldo no lo es.
///
/// El caso sano cuesta un solo pedido. Cuando el par esta atrasado, se retrocede
/// a saltos crecientes hasta encontrar terreno firme y despues se afina por
/// mitades, que son unos pocos pedidos mas y quedan cacheados.
async fn altura_servible(node: &mut Node, anunciada: u64, cache: &mut u64) -> u64 {
    if anunciada == 0 {
        return 0;
    }
    if tiene_bloque(node, anunciada).await {
        *cache = anunciada;
        return anunciada;
    }

    let mut ultimo_fallo = anunciada;
    let mut piso = 0u64;
    let mut salto = 16u64;
    while salto < anunciada {
        let candidato = anunciada - salto;
        if candidato <= *cache {
            break;
        }
        if tiene_bloque(node, candidato).await {
            piso = candidato;
            break;
        }
        ultimo_fallo = candidato;
        salto *= 4;
    }
    if piso == 0 && *cache > 0 && tiene_bloque(node, *cache).await {
        piso = *cache;
    }

    // Afinar entre el ultimo que si estaba y el primero que no.
    let (mut bajo, mut alto) = (piso, ultimo_fallo.saturating_sub(1));
    while bajo < alto {
        let medio = bajo + (alto - bajo + 1) / 2;
        if tiene_bloque(node, medio).await {
            bajo = medio;
        } else {
            alto = medio - 1;
        }
    }
    *cache = bajo;
    bajo
}

/// Espera a que algun par anuncie un bloque, bombeando mientras tanto.
async fn esperar_altura(node: &mut Node) -> Result<u64, String> {
    if node.highest_announced_height() > 0 {
        return Ok(node.highest_announced_height());
    }
    let limite = tokio::time::Instant::now() + HEIGHT_WAIT;
    while tokio::time::Instant::now() < limite {
        let _ = node.pump_for(PUMP_SLICE).await;
        if node.highest_announced_height() > 0 {
            return Ok(node.highest_announced_height());
        }
    }
    Err(format!(
        "ningun par anuncio un bloque en {} segundos; puede que la conexion se haya caido",
        HEIGHT_WAIT.as_secs()
    ))
}

/// Baja el rango pedido en tandas y lo devuelve con la forma que el escaner
/// espera.
async fn traer_bloques(
    node: &mut Node,
    cache: &mut CacheCuerpos,
    from: u64,
    to: u64,
) -> Result<String, String> {
    let mut bloques: Vec<serde_json::Value> = Vec::new();

    // El genesis no lo tiene nadie que pedir: es el mismo para toda la red y se
    // deriva localmente, igual que hacia el servidor.
    let mut altura = from;
    if altura == 0 {
        let genesis = crate::state::chain::genesis_parent_for(crate::economics::MAINNET_CHAIN_ID);
        bloques.push(
            serde_json::to_value(&genesis)
                .map_err(|e| format!("no pude serializar el genesis: {e}"))?,
        );
        altura = 1;
    }

    while altura <= to {
        let hasta = (altura + BLOCKS_BATCH as u64 - 1).min(to);
        let alturas: Vec<u64> = (altura..=hasta).collect();
        let respuestas = node.request_blocks_batch_direct(&alturas, BLOCKS_WAIT).await;

        for h in &alturas {
            let respuesta = match respuestas.get(h) {
                Some(Ok(r)) => r,
                Some(Err(e)) => return Err(format!("el bloque {h} no llego: {e}")),
                None => {
                    return Err(format!(
                        "ningun par respondio por el bloque {h}; probablemente se corto la red"
                    ))
                }
            };

            // Pasar por `Block` no es ceremonia: valida que lo que mando el par
            // tenga la forma de un bloque antes de que el escaner lo mire.
            let bloque: crate::types::Block =
                serde_json::from_slice(&respuesta.response.encrypted_block)
                    .map_err(|e| format!("el par mando un bloque {h} ilegible: {e}"))?;

            for cuerpo in &respuesta.response.private_tx_bodies {
                if let Ok(tx) = serde_json::from_slice::<serde_json::Value>(cuerpo) {
                    if let Some(hash) = tx.get("tx_hash").and_then(|v| v.as_str()) {
                        cache.guardar(hash.to_string(), cuerpo.clone());
                    }
                }
            }

            bloques.push(
                serde_json::to_value(&bloque)
                    .map_err(|e| format!("no pude reserializar el bloque {h}: {e}"))?,
            );
        }
        altura = hasta + 1;
    }

    serde_json::to_string(&serde_json::json!({
        "from_height": from,
        "to_height": to,
        "blocks_count": bloques.len(),
        "blocks": bloques,
    }))
    .map_err(|e| format!("no pude armar la respuesta: {e}"))
}

/// Devuelve el cuerpo de una transaccion privada, del cache o de la red.
async fn traer_cuerpo_privado(
    node: &mut Node,
    cache: &CacheCuerpos,
    hash: &str,
) -> Result<String, String> {
    let crudo = match cache.buscar(hash) {
        Some(cuerpo) => cuerpo.clone(),
        None => node
            .request_private_tx_body_with_retry(hash, 3, BLOCKS_WAIT)
            .await
            .map_err(|e| format!("no pude conseguir el cuerpo de {hash}: {e}"))?,
    };
    String::from_utf8(crudo).map_err(|e| format!("el cuerpo de {hash} no es texto valido: {e}"))
}

async fn run_client(
    seeds: Vec<String>,
    tor_proxy: Option<String>,
    identity_path: Option<std::path::PathBuf>,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    listo_tx: std_mpsc::Sender<Result<(), String>>,
) {
    // Puerto 0: este cliente no escucha. Un telefono no es alcanzable desde
    // afuera y no tiene por que serlo -- solo necesita salir.
    let mut node = match Node::new_with_tor_proxy_at(0, tor_proxy, identity_path).await {
        Ok(node) => node,
        Err(err) => {
            let _ = listo_tx.send(Err(format!("no pude crear el nodo: {err}")));
            return;
        }
    };

    let mut alguno = false;
    for seed in &seeds {
        if node.connect_to_peer(seed).await.is_ok() {
            alguno = true;
        }
        // Bombear entre marcados: el apreton de manos ocurre dentro del bucle
        // de eventos, no dentro de connect_to_peer.
        let _ = node.pump_for(PUMP_SLICE).await;
    }

    if !alguno {
        let _ = listo_tx.send(Err(
            "ninguna semilla acepto la conexion; revisa Tor y la lista de pares".to_string(),
        ));
        return;
    }
    anotar_pares(node.connected_peer_count());
    let _ = listo_tx.send(Ok(()));
    let mut cache = CacheCuerpos::default();
    // Hasta donde se sabe que los pares pueden servir. Se recuerda entre
    // llamadas para no re-tantear la cadena entera cada vez.
    let mut tope_servible = 0u64;

    loop {
        // Se anota aca y no dentro de una rama del select: cuando llega un
        // comando, `select!` descarta el futuro de la otra rama a mitad de
        // camino, asi que cualquier cosa escrita despues del bombeo casi nunca
        // corre. Eso hacia que el estado dijera "cero pares" mientras el
        // cliente estaba bajando bloques de un par.
        anotar_pares(node.connected_peer_count());

        tokio::select! {
            biased;
            cmd = cmd_rx.recv() => match cmd {
                Some(Cmd::PublishPrivateTx(tx, resp)) => {
                    let r = publicar_tx_privada(&mut node, &tx).await;
                    let _ = resp.send(r);
                }
                Some(Cmd::Height(resp)) => {
                    let r = match esperar_altura(&mut node).await {
                        Ok(anunciada) => {
                            let servible =
                                altura_servible(&mut node, anunciada, &mut tope_servible).await;
                            Ok((servible, anunciada))
                        }
                        Err(e) => Err(e),
                    };
                    let _ = resp.send(r);
                }
                Some(Cmd::Blocks(from, to, resp)) => {
                    let r = traer_bloques(&mut node, &mut cache, from, to).await;
                    let _ = resp.send(r);
                }
                Some(Cmd::PrivateTxBody(hash, resp)) => {
                    let r = traer_cuerpo_privado(&mut node, &cache, &hash).await;
                    let _ = resp.send(r);
                }
                Some(Cmd::Shutdown) | None => return,
            },
            _ = async {
                let _ = node.pump_for(PUMP_SLICE).await;
            } => {}
        }
    }
}
