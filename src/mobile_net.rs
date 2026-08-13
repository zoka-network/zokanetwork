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

/// Cuanto esperar las respuestas de un lote de bloques.
const BLOCKS_WAIT: Duration = Duration::from_secs(60);

/// Cuantos bloques pedir de una. El escaner pide rangos de 250, pero cada
/// bloque es un pedido punto a punto: mandarlos todos juntos abre 250 flujos
/// simultaneos sobre un circuito Tor, que es justo como se pierde una tanda
/// entera. De a 32 el rango tarda un poco mas y llega.
const BLOCKS_BATCH: usize = 32;

/// Cuantos resumenes pedir de una. Son un pedido solo -- no uno por bloque --
/// asi que el limite lo pone el tamaño de la respuesta, no la cantidad de
/// flujos. Mil resumenes son medio mega.
const DIGESTS_BATCH: usize = 1000;

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
    let (servible, anunciada) = pedir(Cmd::Height, Duration::from_secs(240))??;
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
    let espera = BLOCKS_WAIT * (((to - from) as u32 / DIGESTS_BATCH as u32) + 2);
    pedir(
        |tx| Cmd::Digests(from, to, tx),
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
async fn descubrir_altura(node: &mut Node, pista: u64, cache: &mut u64) -> u64 {
    // El atajo: si algun par anuncio una punta y resulta que la tiene, listo.
    if pista > 0 && tiene_bloque(node, pista).await {
        *cache = pista;
        return pista;
    }

    let mut buena = 0u64;
    let mut mala = pista;

    if mala == 0 {
        // Sin pista hay que encontrar un techo: subir duplicando hasta fallar.
        let mut altura = if *cache > 0 { *cache } else { 1 };
        loop {
            if tiene_bloque(node, altura).await {
                buena = altura;
            } else {
                break;
            }
            match altura.checked_mul(2) {
                Some(siguiente) => altura = siguiente,
                None => break,
            }
        }
        mala = altura;
        if buena == 0 {
            // Ni el bloque 1: no hay ningun par util del otro lado.
            return 0;
        }
    } else if *cache > 0 && *cache < mala && tiene_bloque(node, *cache).await {
        buena = *cache;
    }

    while buena + 1 < mala {
        let medio = buena + (mala - buena) / 2;
        if tiene_bloque(node, medio).await {
            buena = medio;
        } else {
            mala = medio;
        }
    }
    *cache = buena;
    buena
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

    while altura <= to {
        let hasta = (altura + DIGESTS_BATCH as u64 - 1).min(to);
        let Some(resumenes) = node
            .request_wallet_digests(altura, hasta, BLOCKS_WAIT)
            .await
        else {
            // El par no sirve resumenes: se baja el tramo entero como siempre.
            // Lento, pero es la unica forma de que la wallet funcione contra
            // nodos que no conocen el protocolo nuevo.
            return traer_bloques(node, cache, from, to).await;
        };
        if resumenes.is_empty() && hasta >= altura {
            return traer_bloques(node, cache, from, to).await;
        }
        for r in resumenes {
            bloques.push(serde_json::json!({
                "header": {
                    "block_height": r.block_height,
                    "hash": r.block_hash,
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
                    let servible =
                        descubrir_altura(&mut node, anunciada, &mut tope_servible).await;
                    let r = if servible == 0 {
                        Err("ningun par pudo entregar un bloque; revisa la conexion".to_string())
                    } else {
                        Ok((servible, anunciada.max(servible)))
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
