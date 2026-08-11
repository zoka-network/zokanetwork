//! Prueba el cliente de red movil contra la red real, desde una PC.
//!
//! El cliente movil vive dentro del `.so` de Android, y probarlo ahi significa
//! recompilar, reinstalar y leer el registro del telefono a ciegas -- Arti no
//! escribe en logcat, asi que un arranque de Tor lento y uno fallido se ven
//! igual. Este ejemplo corre exactamente el mismo codigo en la PC, donde se ve
//! todo.
//!
//! No reemplaza la prueba en el telefono: ahi hay que verificar que Arti
//! levante en un ARM de 32 bits y que el puente JNI funcione. Lo que si hace es
//! separar las dos preguntas. Si esto anda, un fallo en el telefono es de Arti
//! o del puente, no de la logica de red.
//!
//! Necesita un Tor escuchando. Con el que trae el repo:
//!
//! ```text
//! third_party/tor/windows-x86_64/tor.exe --SocksPort 9251 --DataDirectory <dir>
//! cargo run --example probar_cliente_movil -- 127.0.0.1:9251
//! ```

use std::time::Instant;
use zoka::mobile_net;

const SEED: &str = "/onion3/d5pk3nhesk5wlvz4f5unbhcozxgtj7u6mhagauyprjrxndzpwbaoeiid:28080";

fn main() {
    let proxy = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("uso: cargo run --example probar_cliente_movil -- <host:puerto del proxy SOCKS5>");
        std::process::exit(2);
    });

    // Un directorio propio, como el que le pasa la aplicacion de Android. Ahi
    // el nodo guarda su identidad de red; sin decirselo la escribe relativa al
    // directorio de trabajo, que en un telefono es de solo lectura.
    let data_dir = std::env::temp_dir().join("zoka-prueba-movil");
    let _ = std::fs::create_dir_all(&data_dir);
    let data_dir_json = data_dir.display().to_string().replace('\\', "\\\\");
    let config = format!(
        r#"{{"seeds":["{SEED}"],"tor_proxy":"{proxy}","data_dir":"{data_dir_json}"}}"#
    );
    println!("directorio de datos: {}", data_dir.display());

    paso("conectar a la semilla", || mobile_net::connect_json(&config));

    let altura = match paso("pedir la altura de la red", mobile_net::chain_height_json) {
        Some(json) => json
            .split("\"height\":")
            .nth(1)
            .map(|resto| {
                resto
                    .trim_start()
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
            })
            .and_then(|d| d.parse::<u64>().ok())
            .unwrap_or(0),
        None => 0,
    };

    // Un rango corto y reciente: alcanza para saber si los bloques llegan y se
    // leen. Bajar la cadena entera es trabajo del escaner, no de esta prueba.
    // Se prueba a varias distancias de la punta a proposito. Un par puede
    // anunciar por gossip un bloque que todavia no guardo como canonico, o
    // relayar anuncios de otros sin tener esos bloques: en ese caso lo reciente
    // falla y lo viejo anda, y conviene saber cual de los dos casos es.
    for atras in [0u64, 4, 100] {
        if altura <= atras + 2 {
            continue;
        }
        let hasta = altura - atras;
        let desde = hasta - 2;
        paso(&format!("bloques {desde}..{hasta} ({atras} bajo la punta)"), || {
            mobile_net::blocks_range_json(desde, hasta).map(|json| {
                let cuantos = json.matches("\"block_height\"").count();
                format!("{cuantos} bloques leidos, {} bytes", json.len())
            })
        });
    }

    // Hasta donde puede servir de verdad el par, que no tiene por que ser lo
    // que anuncia: gossip propaga los anuncios de toda la malla, asi que un par
    // atrasado reenvia alturas que el mismo no guardo.
    if altura > 0 {
        print!("{:.<44}", "punta real que puede servir el par");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let (mut bajo, mut alto) = (0u64, altura);
        while bajo < alto {
            let medio = bajo + (alto - bajo + 1) / 2;
            if mobile_net::blocks_range_json(medio, medio).is_ok() {
                bajo = medio;
            } else {
                alto = medio - 1;
            }
        }
        println!(" {bajo}  (anunciado: {altura}, atraso: {})", altura - bajo);
    }

    let clave = data_dir.join("zoka-red").join("node_key");
    println!(
        "identidad guardada en el directorio indicado.... {}",
        if clave.is_file() { "SI" } else { "NO -- se escribio en otro lado" }
    );

    // Si se pasa un hash, se le pide el cuerpo a los pares. Es la unica forma
    // de saber si una transaccion privada propago de verdad: la red anuncia el
    // hash y solo entrega el cuerpo a quien lo pide, asi que si un par ajeno
    // puede darlo, el anuncio llego y alguien mas la tiene.
    if let Some(hash) = std::env::args().nth(2) {
        paso(&format!("pedirle a la red el cuerpo de {}", &hash[..8.min(hash.len())]), || {
            mobile_net::private_tx_body_json(&hash).map(|j| format!("la tiene ({} bytes)", j.len()))
        });
    }

    paso("estado final", mobile_net::status_json);
    let _ = mobile_net::disconnect();
}

/// Corre un paso, lo cronometra y cuenta como salio.
fn paso<F>(que: &str, f: F) -> Option<String>
where
    F: FnOnce() -> Result<String, String>,
{
    print!("{que:.<44}");
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let t0 = Instant::now();
    match f() {
        Ok(salida) => {
            println!(" OK en {:.0}s\n    {}", t0.elapsed().as_secs_f32(), salida);
            Some(salida)
        }
        Err(err) => {
            println!(" FALLO en {:.0}s\n    {}", t0.elapsed().as_secs_f32(), err);
            None
        }
    }
}
