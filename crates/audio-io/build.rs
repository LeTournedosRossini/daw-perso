//! Compile la couche C++ ASIO (Windows seulement) si le SDK Steinberg est trouvé.
//!
//! Le SDK n'est pas dans le dépôt (sa licence : « Steinberg ASIO License » ou GPLv3). On le cherche dans
//! la variable d'environnement `ASIO_SDK_DIR`, sinon dans `third_party/asiosdk` à la racine du workspace.
//! Sans SDK, la compilation réussit mais le backend ASIO est absent (cfg `daw_asio` non défini).

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(daw_asio)");
    println!("cargo:rerun-if-env-changed=ASIO_SDK_DIR");
    println!("cargo:rerun-if-changed=native");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let sdk = env::var_os("ASIO_SDK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../../third_party/asiosdk"));
    let common = sdk.join("common");
    if !common.join("iasiodrv.h").exists() {
        println!("cargo:warning=SDK ASIO introuvable ({}) : backend ASIO désactivé. Définir ASIO_SDK_DIR.", sdk.display());
        return;
    }
    cc::Build::new()
        .cpp(true)
        .file("native/asio_shim.cpp")
        .include(&common)
        .include("native")
        .flag_if_supported("/std:c++17")
        .compile("daw_asio_shim");
    for lib in ["ole32", "advapi32", "user32"] {
        println!("cargo:rustc-link-lib={lib}");
    }
    println!("cargo:rustc-cfg=daw_asio");
}
