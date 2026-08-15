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
use crate::network::p2p::{
    MobileBlockProbe, Node, NodeProfile, MOBILE_P2P_OPERATION_WAIT, PUBLISH_RETRY_PAUSE,
};
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

/// Cuanto esperar las respuestas de un lote de bloques.
const BLOCKS_WAIT: Duration = MOBILE_P2P_OPERATION_WAIT;

/// Cuantos bloques pedir de una. El escaner pide rangos de 250, pero cada
/// bloque es un pedido punto a punto: mandarlos todos juntos abre 250 flujos
/// simultaneos sobre un circuito Tor, que es justo como se pierde una tanda
/// entera. De a 32 el rango tarda un poco mas y llega.
const BLOCKS_BATCH: usize = 32;

/// Cuantos resumenes pedir de una. Son un pedido solo -- no uno por bloque --
/// asi que el limite lo pone el tamaño de la respuesta, no la cantidad de
/// flujos. Mil resumenes son medio mega.
const DIGESTS_BATCH: usize = 1000;

/// A coherent mobile chunk is tried against at most two peers. The shared
/// deadline is divided among the candidates that remain, so an unresponsive
/// first peer cannot consume the second peer's opportunity.
const COHERENT_BLOCK_PEER_ATTEMPTS: usize = 2;
const HEIGHT_PROBE_PEER_ATTEMPTS: usize = 3;
const MOBILE_CALL_SLACK: Duration = Duration::from_secs(15);
const PRIVATE_TX_ANNOUNCE_PUBLISH_ATTEMPTS: u32 = 10;
const PRIVATE_TX_FULL_PUBLISH_ATTEMPTS: u32 = 5;
/// Round the deterministic 4.5s retry ceiling up to five seconds.
const PUBLISH_RETRY_BUDGET_SLACK: Duration = Duration::from_millis(500);
/// Height probing is bounded separately from reconnect so its synchronous JNI
/// caller has a finite, derivable outer timeout.
const HEIGHT_DISCOVERY_WAIT: Duration = Duration::from_secs(120);

/// Techo del cache de cuerpos de transacciones privadas.
///
/// Los cuerpos llegan de arriba con los bloques y se sirven al escaner sin otra
/// vuelta a la red. Sin techo, un escaneo largo los acumularia todos en la
/// memoria del telefono.
const MAX_CACHED_BODIES: usize = 4096;

fn range_batch_count(from: u64, to: u64, batch_size: usize) -> u32 {
    if from > to || batch_size == 0 {
        return 0;
    }
    let count = to.saturating_sub(from).saturating_add(1);
    let batches = count.saturating_add(batch_size as u64 - 1) / batch_size as u64;
    u32::try_from(batches).unwrap_or(u32::MAX)
}

fn network_work_budget(batch_count: u32) -> Duration {
    BLOCKS_WAIT
        .checked_mul(batch_count)
        .unwrap_or(Duration::MAX)
}

fn coherent_block_attempt_budget(remaining: Duration, candidates_left: usize) -> Duration {
    remaining
        .checked_div(candidates_left.max(1) as u32)
        .unwrap_or(remaining)
}

fn blocks_call_timeout(from: u64, to: u64) -> Duration {
    PEER_WAIT
        .saturating_add(network_work_budget(range_batch_count(
            from,
            to,
            BLOCKS_BATCH,
        )))
        .saturating_add(MOBILE_CALL_SLACK)
}

fn digests_call_timeout(from: u64, to: u64) -> Duration {
    // Worst case: several digest chunks succeeded, the last provider is
    // legacy/invalid, and the call restarts the complete range with blocks.
    PEER_WAIT
        .saturating_add(network_work_budget(range_batch_count(
            from,
            to,
            DIGESTS_BATCH,
        )))
        .saturating_add(network_work_budget(range_batch_count(
            from,
            to,
            BLOCKS_BATCH,
        )))
        .saturating_add(MOBILE_CALL_SLACK)
}

fn private_body_call_timeout() -> Duration {
    PEER_WAIT
        .saturating_add(BLOCKS_WAIT)
        .saturating_add(MOBILE_CALL_SLACK)
}

fn maximum_publish_retry_work() -> Duration {
    PUMP_SLICE
        .saturating_add(PUBLISH_RETRY_PAUSE)
        .checked_mul(
            PRIVATE_TX_ANNOUNCE_PUBLISH_ATTEMPTS
                .saturating_add(PRIVATE_TX_FULL_PUBLISH_ATTEMPTS),
        )
        .unwrap_or(Duration::MAX)
}

fn publish_call_timeout() -> Duration {
    PEER_WAIT
        .saturating_add(maximum_publish_retry_work())
        .saturating_add(PUBLISH_RETRY_BUDGET_SLACK)
        .saturating_add(MOBILE_CALL_SLACK)
}

fn chain_height_call_timeout() -> Duration {
    PEER_WAIT
        .saturating_add(HEIGHT_DISCOVERY_WAIT)
        .saturating_add(MOBILE_CALL_SLACK)
}

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
    PublishPrivateTx(
        Box<PrivateTransaction>,
        std_mpsc::Sender<Result<(), String>>,
    ),
    Height(std_mpsc::Sender<Result<(u64, u64), String>>),
    Blocks(u64, u64, std_mpsc::Sender<Result<String, String>>),
    Digests(u64, u64, std_mpsc::Sender<Result<String, String>>),
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

    let mut slot = handle_slot()
        .lock()
        .map_err(|_| "estado envenenado".to_string())?;
    if slot.is_some() {
        drop(slot);
        return status_json();
    }

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (listo_tx, listo_rx) = std_mpsc::channel();
    let seeds = config.seeds.clone();
    let tor_proxy = config.tor_proxy.clone();
    let identity_path = config.data_dir.as_ref().map(|dir| {
        std::path::PathBuf::from(dir)
            .join("zoka-red")
            .join("node_key")
    });
    // Los pares aprendidos viven al lado de la identidad, en el directorio
    // privado de la aplicacion. Sin `data_dir` cae a una ruta relativa, que en
    // Android no se puede escribir: ahi el almacen no persiste y la wallet
    // vuelve a depender de las semillas, como antes. Degrada, no rompe.
    let peer_store_path = config
        .data_dir
        .as_ref()
        .map(|dir| {
            std::path::PathBuf::from(dir)
                .join("zoka-red")
                .join("peer_store.json")
        })
        .unwrap_or_else(|| std::path::PathBuf::from("peer_store.json"));

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
            rt.block_on(run_client(
                seeds,
                tor_proxy,
                identity_path,
                peer_store_path,
                cmd_rx,
                listo_tx,
            ));
        })
        .map_err(|e| format!("no pude crear el hilo de red: {e}"))?;

    match listo_rx.recv_timeout(CONNECT_TIMEOUT) {
        Ok(Ok(())) => {
            *slot = Some(Handle {
                cmd_tx,
                _hilo: hilo,
            });
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
    let tx_hash = tx.tx_hash.clone();

    let slot = handle_slot()
        .lock()
        .map_err(|_| "estado envenenado".to_string())?;
    let handle = slot
        .as_ref()
        .ok_or_else(|| "la red no esta conectada".to_string())?;

    let (tx_resp, rx_resp) = std_mpsc::channel();
    handle
        .cmd_tx
        .send(Cmd::PublishPrivateTx(Box::new(tx), tx_resp))
        .map_err(|_| "el cliente de red se detuvo".to_string())?;
    drop(slot);

    match rx_resp.recv_timeout(publish_call_timeout()) {
        Ok(Ok(())) => serde_json::to_string(&serde_json::json!({
            "published": true,
            "hash": tx_hash,
        }))
        .map_err(|e| format!("no pude serializar la confirmacion: {e}")),
        Ok(Err(err)) => Err(err),
        Err(_) => Err("la publicacion no confirmo a tiempo".to_string()),
    }
}

/// Corta la red y libera el nodo. La wallet la llama al pasar a segundo plano:
/// mantener circuitos Tor abiertos con la pantalla apagada gasta bateria sin
/// que nadie mire el resultado.
/// Manda un comando al hilo de red y espera su respuesta.
fn pedir<T>(armar: impl FnOnce(std_mpsc::Sender<T>) -> Cmd, espera: Duration) -> Result<T, String> {
    let slot = handle_slot()
        .lock()
        .map_err(|_| "estado envenenado".to_string())?;
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
    let (servible, anunciada) = pedir(Cmd::Height, chain_height_call_timeout())??;
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
    pedir(
        |tx| Cmd::Blocks(from, to, tx),
        blocks_call_timeout(from, to),
    )?
}

/// Resumenes de bloque de `from` a `to`: lo justo para escanear un saldo.
///
/// Un bloque entero pesa unos 4.500 bytes y el 56% es la prueba de rango del
/// coinbase, que solo hace falta para gastar la nota -- o sea, en los poquisimos
/// bloques donde el coinbase es propio. Bajar la cadena entera asi son 77 MB y
/// cincuenta minutos sobre Tor. Un resumen son unos 500 bytes.
///
/// Si el par no conoce el protocolo se cae a bloques enteros, que es lento pero
/// funciona con cualquier nodo. La wallet no puede quedarse sin saldo solo
/// porque le toco un par viejo.
pub fn blocks_digest_range_json(from: u64, to: u64) -> Result<String, String> {
    if from > to {
        return Err(format!("rango invalido: {from} > {to}"));
    }
    pedir(
        |tx| Cmd::Digests(from, to, tx),
        digests_call_timeout(from, to),
    )?
}

/// Cuerpo de una transaccion privada por su hash, en JSON.
///
/// Primero mira el cache que dejaron los bloques ya bajados; solo sale a la red
/// si el cuerpo no vino con su bloque.
pub fn private_tx_body_json(tx_hash: &str) -> Result<String, String> {
    pedir(
        |tx| Cmd::PrivateTxBody(tx_hash.to_string(), tx),
        private_body_call_timeout(),
    )?
}

pub fn disconnect() -> Result<String, String> {
    let mut slot = handle_slot()
        .lock()
        .map_err(|_| "estado envenenado".to_string())?;
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
    node.publish_message_with_retry(
        &ZokaMessage::PrivateTxAnnounce(anuncio),
        PRIVATE_TX_ANNOUNCE_PUBLISH_ATTEMPTS,
        PUMP_SLICE,
    )
    .await
    .map_err(|e| format!("no pude anunciar la tx: {e}"))?;

    // La transaccion completa es el segundo camino, no el principal: si el
    // anuncio llego, la red ya puede pedir el cuerpo. Un fallo aca no invalida
    // la publicacion.
    let _ = node
        .publish_message_with_retry(
            &ZokaMessage::PrivateTransaction(msg),
            PRIVATE_TX_FULL_PUBLISH_ATTEMPTS,
            PUMP_SLICE,
        )
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

fn validar_bloque_sondeado(
    altura: u64,
    respuesta: &crate::network::p2p::SourcedBlockResponse,
) -> Result<(), String> {
    let bloque: crate::types::Block = serde_json::from_slice(&respuesta.response.encrypted_block)
        .map_err(|error| {
            format!(
                "el par {} mando un bloque {altura} ilegible: {error}",
                respuesta.source
            )
        })?;
    if respuesta.response.block_height != altura
        || bloque.header.block_height != altura
        || bloque.header.hash != respuesta.response.block_hash
        || bloque.compute_hash() != bloque.header.hash
        || bloque.compute_merkle_root() != bloque.header.merkle_root
    {
        return Err(format!(
            "el bloque {altura} del par {} no coincide con sus compromisos",
            respuesta.source
        ));
    }
    Ok(())
}

/// Si los pares tienen realmente el bloque de esa altura.
///
/// Los candidatos se prueban de a uno: un nodo atrasado puede contestar
/// `NotFound` sin tapar a otro que si tiene el bloque. Solo se devuelve `false`
/// si todos los candidatos seleccionados respondieron `NotFound`; fallos,
/// rechazos, cuerpos invalidos y deadlines son inconclusos y fallan cerrados.
async fn tiene_bloque_antes(
    node: &mut Node,
    altura: u64,
    deadline: tokio::time::Instant,
) -> Result<bool, String> {
    let now = tokio::time::Instant::now();
    let probe_deadline = deadline.min(now + PROBE_WAIT);
    if probe_deadline <= now {
        return Err(height_discovery_timeout_error());
    }

    let mut peers = node.mobile_block_request_peers();
    peers.truncate(HEIGHT_PROBE_PEER_ATTEMPTS);
    if peers.is_empty() {
        return Err(format!(
            "el sondeo de altura {altura} no tiene pares elegibles"
        ));
    }

    let peer_count = peers.len();
    let mut not_found = 0usize;
    let mut errores = Vec::new();
    for (peer_index, peer) in peers.into_iter().enumerate() {
        let remaining = probe_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            errores.push(height_discovery_timeout_error());
            break;
        }
        let attempt_budget =
            coherent_block_attempt_budget(remaining, peer_count - peer_index);
        match node
            .request_mobile_block_probe_from_peer(altura, &peer, attempt_budget)
            .await
        {
            Ok(MobileBlockProbe::Found(respuesta)) => {
                match validar_bloque_sondeado(altura, &respuesta) {
                    Ok(()) => return Ok(true),
                    Err(error) => {
                        node.penalize_block_sync_peer(respuesta.source);
                        errores.push(error);
                    }
                }
            }
            Ok(MobileBlockProbe::NotFound) => not_found = not_found.saturating_add(1),
            Err(error) => errores.push(format!("el par {peer}: {error}")),
        }
    }

    if not_found == peer_count {
        Ok(false)
    } else {
        let detalle = if errores.is_empty() {
            height_discovery_timeout_error()
        } else {
            errores.join("; ")
        };
        Err(format!(
            "el sondeo de altura {altura} fue inconcluso ({not_found}/{peer_count} NotFound): {detalle}"
        ))
    }
}

fn height_discovery_timeout_error() -> String {
    format!(
        "la busqueda de altura no termino en {} segundos; reintenta",
        HEIGHT_DISCOVERY_WAIT.as_secs()
    )
}

/// Cuanto se le da a la conexion para tener un par antes de pedirle algo.
///
/// `connect` vuelve en cuanto la semilla acepta el marcado, pero el apreton de
/// manos de libp2p termina despues, dentro del bucle de eventos. Preguntarle
/// algo a la red en ese hueco no falla por falta de red: falla porque todavia
/// no hay a quien preguntarle.
const PEER_WAIT: Duration = Duration::from_secs(90);

/// Cada cuanto se vuelve a marcar mientras no haya ningun par.
const REDIAL_INTERVAL: Duration = Duration::from_secs(15);

/// Espera a tener al menos un par, volviendo a marcar a las semillas si hace
/// falta.
///
/// Un telefono pierde conexiones todo el tiempo: el circuito Tor se cae, la
/// red cambia de wifi a datos, la pantalla se apaga. Sin volver a marcar, la
/// wallet queda muerta hasta que el usuario reinicie la aplicacion, y ni
/// siquiera sabe por que. Reconectar es parte del trabajo, no un extra.
///
/// Devuelve `false` si se acabo el tiempo sin conseguir ninguno.
async fn esperar_par(node: &mut Node, seeds: &[String]) -> bool {
    if node.connected_peer_count() > 0 {
        return true;
    }
    let limite = tokio::time::Instant::now() + PEER_WAIT;
    let mut proximo_marcado = tokio::time::Instant::now();
    while tokio::time::Instant::now() < limite {
        if tokio::time::Instant::now() >= proximo_marcado {
            for seed in seeds {
                let _ = node.connect_to_peer(seed).await;
            }
            proximo_marcado = tokio::time::Instant::now() + REDIAL_INTERVAL;
        }
        let _ = node.pump_for(PUMP_SLICE).await;
        anotar_pares(node.connected_peer_count());
        if node.connected_peer_count() > 0 {
            return true;
        }
    }
    false
}

/// Descubre hasta donde llegan los pares preguntandoles, sin esperar a que
/// anuncien nada.
///
/// Esperar un anuncio era fragil por una razon medible: los bloques de esta
/// red tardan unos 138 segundos de mediana, con picos de media hora. Cualquier
/// espera fija o es mas corta que el intervalo -- y entonces la wallet falla
/// aunque todo funcione -- o es tan larga que el usuario cree que se colgo. Y
/// con la cadena quieta no hay espera que alcance.
///
/// Preguntar no tiene ese problema: los pares contestan al instante si tienen
/// el bloque. Se sube a saltos que se duplican hasta encontrar uno que no
/// tienen, y despues se afina por mitades. Son unos quince pedidos la primera
/// vez y queda cacheado; el anuncio, cuando lo hay, se usa como atajo.
async fn descubrir_altura(
    node: &mut Node,
    pista: u64,
    cache: &mut u64,
) -> Result<u64, String> {
    let deadline = tokio::time::Instant::now() + HEIGHT_DISCOVERY_WAIT;
    descubrir_altura_antes(node, pista, cache, deadline).await
}

async fn descubrir_altura_antes(
    node: &mut Node,
    pista: u64,
    cache: &mut u64,
    deadline: tokio::time::Instant,
) -> Result<u64, String> {
    let mut buena = 0u64;
    let mut mala = 0u64;
    // Tanto el anuncio como el cache son pistas, no una punta demostrada. La
    // altura solo es exacta despues de encontrar el primer bloque ausente.
    let inicial = pista.max(*cache).max(1);
    if tiene_bloque_antes(node, inicial, deadline).await? {
        buena = inicial;
    } else {
        mala = inicial;
    }

    if mala == 0 {
        if buena == u64::MAX {
            *cache = buena;
            return Ok(buena);
        }

        // Probar primero el sucesor hace barato el caso normal: el anuncio ya
        // era la punta. Si tambien existe, se busca un techo duplicando.
        let mut altura = buena.saturating_add(1);
        loop {
            if tiene_bloque_antes(node, altura, deadline).await? {
                buena = altura;
                if buena == u64::MAX {
                    *cache = buena;
                    return Ok(buena);
                }
                altura = buena.checked_mul(2).unwrap_or(u64::MAX);
            } else {
                mala = altura;
                break;
            }
        }
    }

    while buena.saturating_add(1) < mala {
        let medio = buena + (mala - buena) / 2;
        if tiene_bloque_antes(node, medio, deadline).await? {
            buena = medio;
        } else {
            mala = medio;
        }
    }
    *cache = buena;
    Ok(buena)
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
    let mut hash_anterior: Option<String> = None;
    let mut peer_anterior: Option<libp2p::PeerId> = None;

    // El genesis no lo tiene nadie que pedir: es el mismo para toda la red y se
    // deriva localmente, igual que hacia el servidor.
    let mut altura = from;
    if altura == 0 {
        let genesis = crate::state::chain::genesis_parent_for(crate::economics::MAINNET_CHAIN_ID);
        hash_anterior = Some(genesis.header.hash.clone());
        bloques.push(
            serde_json::to_value(&genesis)
                .map_err(|e| format!("no pude serializar el genesis: {e}"))?,
        );
        altura = 1;
    }

    while altura <= to {
        let hasta = altura.saturating_add(BLOCKS_BATCH as u64 - 1).min(to);
        let alturas: Vec<u64> = (altura..=hasta).collect();
        let mut peers = node.mobile_block_request_peers();
        if let Some(preferred) = peer_anterior.as_ref() {
            if let Some(position) = peers.iter().position(|peer| peer == preferred) {
                peers.swap(0, position);
            }
        }
        peers.truncate(COHERENT_BLOCK_PEER_ATTEMPTS);
        if peers.is_empty() {
            return Err(format!(
                "no hay pares conectados para los bloques {altura}..{hasta}"
            ));
        }

        let chunk_deadline = tokio::time::Instant::now() + BLOCKS_WAIT;
        let mut lote_valido = None;
        let mut proveedor_valido = None;
        let mut ultimo_error = String::new();

        let peer_count = peers.len();
        for (peer_index, peer) in peers.into_iter().enumerate() {
            let remaining = chunk_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let attempt_budget =
                coherent_block_attempt_budget(remaining, peer_count - peer_index);
            if attempt_budget.is_zero() {
                continue;
            }
            let respuestas = node
                .request_blocks_batch_direct_from_peer(&alturas, &peer, attempt_budget)
                .await;
            let mut candidato = Vec::with_capacity(alturas.len());
            let mut candidato_anterior = hash_anterior.clone();
            let mut candidato_error = None;

            for h in &alturas {
                let respuesta = match respuestas.get(h) {
                    Some(Ok(response)) => response.response.clone(),
                    Some(Err(error)) => {
                        candidato_error = Some(format!("el bloque {h} no llego: {error}"));
                        break;
                    }
                    None => {
                        candidato_error =
                            Some(format!("el par {peer} no respondio por el bloque {h}"));
                        break;
                    }
                };

                // La validacion ocurre antes de aceptar al proveedor para que
                // un peer corrupto no gane la carrera y anule el failover.
                let bloque: crate::types::Block =
                    match serde_json::from_slice(&respuesta.encrypted_block) {
                        Ok(block) => block,
                        Err(error) => {
                            node.penalize_block_sync_peer(peer.clone());
                            candidato_error = Some(format!(
                                "el par {peer} mando un bloque {h} ilegible: {error}"
                            ));
                            break;
                        }
                    };
                if bloque.header.block_height != *h
                    || bloque.header.hash != respuesta.block_hash
                    || bloque.compute_hash() != bloque.header.hash
                    || bloque.compute_merkle_root() != bloque.header.merkle_root
                {
                    node.penalize_block_sync_peer(peer.clone());
                    candidato_error = Some(format!(
                        "el bloque {h} del par {peer} no coincide con sus compromisos"
                    ));
                    break;
                }
                if let Some(anterior) = candidato_anterior.as_ref() {
                    if bloque.header.prev_hash != *anterior {
                        candidato_error = Some(format!(
                            "el par {peer} entrego otra rama entre {} y {h}",
                            h.saturating_sub(1)
                        ));
                        break;
                    }
                }
                candidato_anterior = Some(bloque.header.hash.clone());
                candidato.push((respuesta, bloque));
            }

            if let Some(error) = candidato_error {
                ultimo_error = error;
                continue;
            }
            if candidato.len() == alturas.len() {
                lote_valido = Some(candidato);
                proveedor_valido = Some(peer);
                hash_anterior = candidato_anterior;
                break;
            }
        }

        let lote_valido = lote_valido.ok_or_else(|| {
            if ultimo_error.is_empty() {
                format!("ningun par completo el tramo {altura}..{hasta}")
            } else {
                ultimo_error
            }
        })?;
        peer_anterior = proveedor_valido;

        for (respuesta, bloque) in lote_valido {
            for cuerpo in &respuesta.private_tx_bodies {
                if let Ok(tx) = serde_json::from_slice::<PrivateTransaction>(cuerpo) {
                    if bloque.private_tx_hashes.contains(&tx.tx_hash)
                        && crate::private_wallet::expected_private_tx_hash(&tx) == tx.tx_hash
                    {
                        cache.guardar(tx.tx_hash.clone(), cuerpo.clone());
                    }
                }
            }

            bloques.push(
                serde_json::to_value(&bloque)
                    .map_err(|e| format!("no pude reserializar un bloque: {e}"))?,
            );
        }
        if hasta == to {
            break;
        }
        altura = hasta.saturating_add(1);
    }

    serde_json::to_string(&serde_json::json!({
        "from_height": from,
        "to_height": to,
        "blocks_count": bloques.len(),
        "blocks": bloques,
    }))
    .map_err(|e| format!("no pude armar la respuesta: {e}"))
}

/// Baja los resumenes del tramo, cayendo a bloques enteros si el par no los
/// sirve.
///
/// La forma de la respuesta es la misma en los dos casos, para que el escaner
/// no tenga que saber por cual vino: los campos que un resumen no trae -- la
/// prueba de rango, las transacciones publicas -- son justo los que el escaneo
/// no mira.
async fn traer_resumenes(
    node: &mut Node,
    cache: &mut CacheCuerpos,
    from: u64,
    to: u64,
) -> Result<String, String> {
    let mut bloques: Vec<serde_json::Value> = Vec::new();
    let mut altura = from.max(1);
    let mut hash_anterior: Option<String> = None;

    while altura <= to {
        let hasta = altura.saturating_add(DIGESTS_BATCH as u64 - 1).min(to);
        let Some(resumenes) = node
            .request_wallet_digests_with_anchor(
                altura,
                hasta,
                hash_anterior.as_deref(),
                BLOCKS_WAIT,
            )
            .await
        else {
            // El par no sirve resumenes: se baja el tramo entero como siempre.
            // Lento, pero es la unica forma de que la wallet funcione contra
            // nodos que no conocen el protocolo nuevo.
            return traer_bloques(node, cache, from, to).await;
        };
        if resumenes.len() != (hasta - altura + 1) as usize {
            return traer_bloques(node, cache, from, to).await;
        }
        let mut por_altura = std::collections::BTreeMap::new();
        for r in resumenes {
            if r.block_height < altura
                || r.block_height > hasta
                || r.block_hash.trim().is_empty()
                || por_altura.insert(r.block_height, r).is_some()
            {
                return traer_bloques(node, cache, from, to).await;
            }
        }
        for esperada in altura..=hasta {
            let Some(r) = por_altura.remove(&esperada) else {
                return traer_bloques(node, cache, from, to).await;
            };
            if let Some(anterior) = hash_anterior.as_ref() {
                if r.prev_hash != *anterior {
                    return Err(format!(
                        "los resumenes mezclan ramas entre los bloques {} y {}",
                        esperada.saturating_sub(1),
                        esperada
                    ));
                }
            }
            hash_anterior = Some(r.block_hash.clone());
            bloques.push(serde_json::json!({
                "header": {
                    "block_height": r.block_height,
                    "hash": r.block_hash,
                    "prev_hash": r.prev_hash,
                    "timestamp": r.timestamp,
                },
                // El escaner mira `transactions[0]` para reconocer un coinbase
                // suyo, asi que el resumen se presenta con esa misma forma.
                "transactions": match (&r.coinbase_recipient, r.coinbase_amount) {
                    (Some(receiver), Some(amount)) => serde_json::json!([{
                        "receiver": receiver,
                        "amount": amount,
                    }]),
                    _ => serde_json::json!([]),
                },
                "private_coinbase": r.coinbase_output_id.as_ref().map(|id| serde_json::json!({
                    "output_id": id,
                    "commitment": r.coinbase_commitment.clone().unwrap_or_default(),
                    "unlock_height": r.coinbase_unlock_height.unwrap_or(0),
                    "recipient_address": r.coinbase_recipient.clone().unwrap_or_default(),
                })),
                "private_tx_hashes": r.private_tx_hashes,
                "private_nullifiers": r.private_nullifiers,
            }));
        }
        if hasta == to {
            break;
        }
        altura = hasta.saturating_add(1);
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
    let cuerpo: PrivateTransaction = serde_json::from_slice(&crudo)
        .map_err(|e| format!("el cuerpo de {hash} no es una transaccion privada valida: {e}"))?;
    if cuerpo.tx_hash != hash {
        return Err(format!(
            "el par devolvio la transaccion {} cuando se pidio {hash}",
            cuerpo.tx_hash
        ));
    }
    let hash_esperado = crate::private_wallet::expected_private_tx_hash(&cuerpo);
    if cuerpo.tx_hash != hash_esperado {
        return Err(format!(
            "el cuerpo de {hash} no coincide con su hash canonico"
        ));
    }
    serde_json::to_string(&cuerpo)
        .map_err(|e| format!("no pude serializar el cuerpo de {hash}: {e}"))
}

/// Cuantos pares conocidos se marcan al arrancar, ademas de las semillas.
///
/// Con uno alcanza para empezar a pedir bloques; el resto llega por
/// descubrimiento. Marcar de a muchos sobre Tor abre circuitos que cuestan
/// bateria y no aceleran nada.
const CANDIDATOS_AL_ARRANCAR: usize = 6;

/// Cada cuanto se anotan los pares que la red fue presentando.
///
/// Dos minutos: lo suficiente para no perder lo aprendido si la aplicacion se
/// cierra, y lo bastante espaciado para no escribir en la memoria del telefono
/// mas de lo necesario.
const INTERVALO_APRENDER_PARES: Duration = Duration::from_secs(120);

/// Abre el almacen de pares, o uno vacio si no se puede leer.
///
/// Que no se pueda leer no es motivo para no arrancar: se pierde la memoria de
/// pares, no la capacidad de conectarse. Las semillas siguen ahi.
fn peer_store_para(path: &std::path::Path) -> crate::network::peer_store::PeerStore {
    crate::network::peer_store::PeerStore::load_or_default(path).unwrap_or_else(|e| {
        tracing::info!("no pude leer los pares guardados: {e}");
        crate::network::peer_store::PeerStore::empty(path)
    })
}

/// A quien marcar, en orden: primero lo que ya funciono, despues las semillas.
///
/// Las semillas van igual aunque no esten en el almacen -- son la garantia de
/// que una wallet nueva, o una cuyos pares se cayeron todos, tenga por donde
/// entrar.
fn candidatos_a_marcar(
    conocidos: &crate::network::peer_store::PeerStore,
    seeds: &[String],
) -> Vec<String> {
    let mut lista = conocidos.dial_addrs(CANDIDATOS_AL_ARRANCAR);
    for seed in seeds {
        if !lista.iter().any(|a| a == seed) {
            lista.push(seed.clone());
        }
    }
    lista
}

/// Anota los pares que la red nos presento y los guarda para la proxima.
fn aprender_pares(node: &Node, conocidos: &mut crate::network::peer_store::PeerStore) {
    for record in node.observed_peer_records() {
        conocidos.mark_identified(
            &record.addr,
            Some(&record.peer_id),
            record.protocol_version.as_deref(),
            &record.protocols,
            "identify",
        );
    }
    let _ = guardar_pares(conocidos);
}

/// Guarda el almacen, podando lo que ya no sirve.
///
/// Un telefono no puede acumular pares muertos para siempre: el almacen se
/// poda solo, quedandose con los que respondieron alguna vez.
fn guardar_pares(conocidos: &mut crate::network::peer_store::PeerStore) -> Result<(), String> {
    conocidos.prune();
    conocidos
        .save()
        .map_err(|e| format!("no pude guardar los pares: {e}"))
}

async fn run_client(
    seeds: Vec<String>,
    tor_proxy: Option<String>,
    identity_path: Option<std::path::PathBuf>,
    peer_store_path: std::path::PathBuf,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    listo_tx: std_mpsc::Sender<Result<(), String>>,
) {
    // Puerto 0: este cliente no escucha. Un telefono no es alcanzable desde
    // afuera y no tiene por que serlo -- solo necesita salir.
    let mut node = match Node::new_with_profile(0, tor_proxy, identity_path, NodeProfile::Wallet).await {
        Ok(node) => node,
        Err(err) => {
            let _ = listo_tx.send(Err(format!("no pude crear el nodo: {err}")));
            return;
        }
    };

    // Los pares que ya conocemos de arranques anteriores, ordenados por cuales
    // sirvieron.
    //
    // Sin esto la wallet depende de que una maquina en particular este
    // encendida: la semilla del manifiesto. Con esto, cada par que alguna vez
    // funciono es otra puerta de entrada, y el almacen los ordena solo -- los
    // que respondieron primero, los que fallaron al final.
    let mut conocidos = peer_store_para(&peer_store_path);
    for seed in &seeds {
        conocidos.upsert_addr(seed, "manifest_seed");
    }

    let mut alguno = false;
    for addr in candidatos_a_marcar(&conocidos, &seeds) {
        match node.connect_to_peer(&addr).await {
            Ok(_) => {
                alguno = true;
                conocidos.mark_connected(&addr, None, "mobile");
            }
            Err(_) => conocidos.mark_failed(&addr),
        }
        // Bombear entre marcados: el apreton de manos ocurre dentro del bucle
        // de eventos, no dentro de connect_to_peer.
        let _ = node.pump_for(PUMP_SLICE).await;
        // Con uno alcanza para arrancar; el resto llega por descubrimiento sin
        // hacer esperar al usuario mirando una pantalla vacia.
        if alguno && node.connected_peer_count() > 0 {
            break;
        }
    }

    if !alguno {
        let _ = guardar_pares(&mut conocidos);
        let _ = listo_tx.send(Err(
            "ningun par conocido acepto la conexion; revisa Tor y la lista de pares".to_string(),
        ));
        return;
    }
    // Con un par en mano, pedirle a la red que nos presente a los demas.
    //
    // Hasta ahora la wallet marcaba la semilla y se quedaba ahi: si esa
    // maquina se caia quedaba ciega, y si mintiera nadie la contradiria. El
    // codigo de Kademlia ya viajaba compilado adentro; solo faltaba
    // arrancarlo. Un fallo aca no es grave -- se sigue con la semilla, que es
    // lo que habia antes.
    if let Err(e) = node.bootstrap_kademlia() {
        tracing::info!("no pude arrancar el descubrimiento de pares: {e}");
    }

    aprender_pares(&node, &mut conocidos);
    let mut ultimo_aprendizaje = tokio::time::Instant::now();
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

        // Anotar lo que la red fue presentando, cada tanto y no en cada vuelta:
        // el bucle gira cuatro veces por segundo y escribir un archivo a ese
        // ritmo seria castigar la memoria del telefono para nada. Los pares no
        // cambian tan rapido.
        if ultimo_aprendizaje.elapsed() >= INTERVALO_APRENDER_PARES {
            aprender_pares(&node, &mut conocidos);
            ultimo_aprendizaje = tokio::time::Instant::now();
        }

        tokio::select! {
            biased;
            cmd = cmd_rx.recv() => match cmd {
                Some(Cmd::PublishPrivateTx(tx, resp)) => {
                    let r = if esperar_par(&mut node, &seeds).await {
                        publicar_tx_privada(&mut node, &tx).await
                    } else {
                        Err("ningun par completo la conexion; la transaccion no se publico"
                            .to_string())
                    };
                    let _ = resp.send(r);
                }
                Some(Cmd::Height(resp)) => {
                    if !esperar_par(&mut node, &seeds).await {
                        let _ = resp.send(Err(format!(
                            "ningun par completo la conexion en {} segundos; revisa Tor",
                            PEER_WAIT.as_secs()
                        )));
                        continue;
                    }
                    // El anuncio es una pista gratis, no un requisito: se lee
                    // lo que haya llegado y se sigue sin esperar.
                    let anunciada = node.highest_announced_height();
                    let r = match descubrir_altura(
                        &mut node,
                        anunciada,
                        &mut tope_servible,
                    ).await {
                        Ok(0) => Err(
                            "ningun par pudo entregar un bloque; revisa la conexion".to_string()
                        ),
                        Ok(servible) => Ok((servible, anunciada.max(servible))),
                        Err(error) => Err(error),
                    };
                    let _ = resp.send(r);
                }
                Some(Cmd::Digests(from, to, resp)) => {
                    let r = if esperar_par(&mut node, &seeds).await {
                        traer_resumenes(&mut node, &mut cache, from, to).await
                    } else {
                        Err("ningun par completo la conexion; revisa Tor".to_string())
                    };
                    let _ = resp.send(r);
                }
                Some(Cmd::Blocks(from, to, resp)) => {
                    let r = if esperar_par(&mut node, &seeds).await {
                        traer_bloques(&mut node, &mut cache, from, to).await
                    } else {
                        Err("ningun par completo la conexion; revisa Tor".to_string())
                    };
                    let _ = resp.send(r);
                }
                Some(Cmd::PrivateTxBody(hash, resp)) => {
                    let r = if cache.buscar(&hash).is_some() {
                        traer_cuerpo_privado(&mut node, &cache, &hash).await
                    } else if esperar_par(&mut node, &seeds).await {
                        traer_cuerpo_privado(&mut node, &cache, &hash).await
                    } else {
                        Err("ningun par completo la conexion; revisa Tor".to_string())
                    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::block_request_response::BlockBodyResponse;
    use crate::network::message::BlockResponseMessage;
    use crate::types::{Block, PublicKey, Signature};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    async fn run_multi_peer_height_probe(
        found_on_second_peer: bool,
    ) -> (Result<bool, String>, Vec<Arc<AtomicUsize>>) {
        let mut servers = vec![
            Node::new(0).await.expect("first probe server"),
            Node::new(0).await.expect("second probe server"),
        ];
        let mut order = [0usize, 1usize];
        order.sort_by_key(|index| servers[*index].peer_id().to_string());
        let found_index = found_on_second_peer.then_some(order[1]);
        let block = Block::new(
            "probe-parent".to_string(),
            1_000,
            7,
            42,
            Vec::new(),
            PublicKey(Vec::new()),
            Signature(Vec::new()),
        );
        let found = BlockResponseMessage {
            block_height: block.header.block_height,
            encrypted_block: serde_json::to_vec(&block).expect("serialize probe block"),
            block_hash: block.header.hash.clone(),
            difficulty_bits: None,
            private_tx_bodies: Vec::new(),
            timestamp: block.header.timestamp,
        };
        let calls = vec![Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0))];

        for (index, server) in servers.iter_mut().enumerate() {
            let calls = calls[index].clone();
            let found = found.clone();
            let serves_block = found_index == Some(index);
            server.set_block_body_responder(move |request| {
                calls.fetch_add(1, Ordering::SeqCst);
                if serves_block {
                    BlockBodyResponse::Found(found.clone())
                } else {
                    BlockBodyResponse::NotFound {
                        block_height: request.block_height,
                        block_hash: request.block_hash.clone(),
                        reason: "peer atrasado".to_string(),
                    }
                }
            });
            server
                .listen("/ip4/127.0.0.1/tcp/0")
                .await
                .expect("listen probe server");
        }

        let mut addresses = Vec::new();
        for server in &mut servers {
            let address = tokio::time::timeout(
                Duration::from_secs(2),
                server.wait_for_listen_addr(),
            )
            .await
            .expect("probe server listen deadline")
            .expect("probe server listen address");
            addresses.push(format!("{address}/p2p/{}", server.peer_id()));
        }

        let mut client = Node::new(0).await.expect("height probe client");
        for address in &addresses {
            client.connect_to_peer(address).await.expect("dial probe server");
        }
        for _ in 0..100 {
            let _ = client.pump_for(Duration::from_millis(5)).await;
            for server in &mut servers {
                let _ = server.pump_for(Duration::from_millis(5)).await;
            }
            if client.connected_peer_count() == servers.len() {
                break;
            }
        }
        assert_eq!(client.connected_peer_count(), servers.len());

        let serving = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            while tokio::time::Instant::now() < deadline {
                for server in &mut servers {
                    let _ = server.pump_for(Duration::from_millis(2)).await;
                }
            }
        });
        let result = tiene_bloque_antes(
            &mut client,
            42,
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .await;
        serving.abort();
        let _ = serving.await;
        (result, calls)
    }

    #[test]
    fn range_batch_count_uses_inclusive_ranges_without_wrapping() {
        assert_eq!(range_batch_count(1, 1, BLOCKS_BATCH), 1);
        assert_eq!(range_batch_count(1, 32, BLOCKS_BATCH), 1);
        assert_eq!(range_batch_count(1, 33, BLOCKS_BATCH), 2);
        assert_eq!(range_batch_count(9, 8, BLOCKS_BATCH), 0);
        assert_eq!(range_batch_count(u64::MAX, u64::MAX, BLOCKS_BATCH), 1);
    }

    #[test]
    fn public_timeouts_cover_reconnect_and_bounded_network_work() {
        assert_eq!(BLOCKS_WAIT, MOBILE_P2P_OPERATION_WAIT);
        assert_eq!(
            blocks_call_timeout(1, 32),
            PEER_WAIT + BLOCKS_WAIT + MOBILE_CALL_SLACK
        );
        assert_eq!(
            digests_call_timeout(1, 1_000),
            PEER_WAIT + BLOCKS_WAIT + (BLOCKS_WAIT * 32) + MOBILE_CALL_SLACK
        );
        assert_eq!(
            private_body_call_timeout(),
            PEER_WAIT + BLOCKS_WAIT + MOBILE_CALL_SLACK
        );
        assert_eq!(
            publish_call_timeout(),
            PEER_WAIT
                + maximum_publish_retry_work()
                + PUBLISH_RETRY_BUDGET_SLACK
                + MOBILE_CALL_SLACK
        );
        assert_eq!(maximum_publish_retry_work(), Duration::from_millis(4_500));
        assert_eq!(
            maximum_publish_retry_work() + PUBLISH_RETRY_BUDGET_SLACK,
            Duration::from_secs(5)
        );
        assert_eq!(
            chain_height_call_timeout(),
            PEER_WAIT + HEIGHT_DISCOVERY_WAIT + MOBILE_CALL_SLACK
        );
        assert!(
            publish_call_timeout()
                > PEER_WAIT + maximum_publish_retry_work() + PUBLISH_RETRY_BUDGET_SLACK
        );
        assert!(chain_height_call_timeout() > PEER_WAIT + HEIGHT_DISCOVERY_WAIT);
    }

    #[test]
    fn coherent_block_deadline_reserves_time_for_the_second_peer() {
        assert_eq!(
            coherent_block_attempt_budget(BLOCKS_WAIT, 2),
            Duration::from_secs(30)
        );
        assert_eq!(
            coherent_block_attempt_budget(Duration::from_secs(30), 1),
            Duration::from_secs(30),
            "the last candidate receives all of the remaining deadline"
        );
    }

    #[tokio::test]
    async fn height_discovery_deadline_fails_closed_without_overwriting_cache() {
        let mut node = Node::new(0).await.expect("test node");
        let mut cache = 123;
        let result = descubrir_altura_antes(
            &mut node,
            200,
            &mut cache,
            tokio::time::Instant::now(),
        )
        .await;

        assert!(result.is_err(), "an expired search cannot return a lower bound");
        assert_eq!(cache, 123, "an incomplete search cannot replace the exact cache");
    }

    #[tokio::test]
    async fn height_probe_reaches_found_peer_after_lagging_peer_not_found() {
        let (result, calls) = run_multi_peer_height_probe(true).await;

        assert_eq!(result, Ok(true));
        assert_eq!(calls.iter().map(|calls| calls.load(Ordering::SeqCst)).sum::<usize>(), 2);
        assert!(calls.iter().all(|calls| calls.load(Ordering::SeqCst) == 1));
    }

    #[tokio::test]
    async fn height_probe_returns_false_only_after_all_peers_not_found() {
        let (result, calls) = run_multi_peer_height_probe(false).await;

        assert_eq!(result, Ok(false));
        assert_eq!(calls.iter().map(|calls| calls.load(Ordering::SeqCst)).sum::<usize>(), 2);
        assert!(calls.iter().all(|calls| calls.load(Ordering::SeqCst) == 1));
    }

    #[test]
    fn private_body_cache_can_be_checked_without_a_peer() {
        let mut cache = CacheCuerpos::default();
        cache.guardar("tx-1".to_string(), b"body".to_vec());
        assert_eq!(cache.buscar("tx-1").map(Vec::as_slice), Some(&b"body"[..]));
    }
}
