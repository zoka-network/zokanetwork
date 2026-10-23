//! En Windows, `zoka.exe` y `zokahd.exe` llevan sus datos de versión: producto,
//! empresa, descripción y versión, lo que muestra Propiedades > Detalles.
//!
//! Sin ellos, Windows ve un ejecutable anónimo, sin firma, bajado de internet y
//! que mina: el 02/07/2026 Defender marcó un `zoka.exe` bajado de GitHub como
//! «PUA:Win32/Caypnamer.A!ml». Estos datos no lo evitan por sí solos (lo que
//! pesa es la firma de código y la reputación), pero un ejecutable que dice
//! quién es y qué hace es lo mínimo. La descripción dice que mina: esconderlo
//! sería peor.
//!
//! Si no está el compilador de recursos (`rc.exe` del SDK de Windows), el nodo
//! compila igual, sin los datos; `prepare-release.ps1` de ZSilent revisa que el
//! binario que se empaqueta los tenga.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    for (bin, description) in [
        ("zoka", "Zoka Quantic full node, wallet and CPU miner"),
        ("zokahd", "Zoka Quantic full node, wallet and CPU miner"),
    ] {
        let rc = out.join(format!("{bin}-version.rc"));
        fs::write(&rc, version_resource(bin, description)).expect("write the version resource");
        if let Err(error) = embed_resource::compile_for(&rc, [bin], embed_resource::NONE).manifest_optional() {
            println!("cargo:warning={bin}.exe without version data: {error}");
        }
    }
}

/// El recurso VERSIONINFO, en el formato de `rc.exe`, con la versión del
/// paquete. Sin `#include`: las constantes van en número para no depender de
/// las rutas del SDK.
fn version_resource(bin: &str, description: &str) -> String {
    let version = env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let part = |name: &str| env::var(name).ok().and_then(|v| v.parse::<u16>().ok()).unwrap_or(0);
    let (major, minor, patch) = (
        part("CARGO_PKG_VERSION_MAJOR"),
        part("CARGO_PKG_VERSION_MINOR"),
        part("CARGO_PKG_VERSION_PATCH"),
    );
    format!(
        r#"1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "Zoka Network"
            VALUE "FileDescription", "{description}"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "{bin}"
            VALUE "LegalCopyright", "Copyright (C) 2026 Zoka Network"
            VALUE "OriginalFilename", "{bin}.exe"
            VALUE "ProductName", "Zoka"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    )
}
