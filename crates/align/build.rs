//! Compile la bibliothèque C++ « flibuste-align » (paquet `native/`, livré par la conversation Neptune) : les fichiers listés dans
//! `native/sources.txt`. C++17 avec exceptions (elles servent à l'annulation, jamais exposées au C). Rubber Band est GPL v2+, le reste MIT :
//! l'ensemble est sous GPLv3 (`native/LICENSE-GPLv3`). Aucune bibliothèque à lier.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=native");
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let native = manifest.join("native");
    let list = fs::read_to_string(native.join("sources.txt")).expect("native/sources.txt");
    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").warnings(false).include(native.join("include")).include(native.join("third_party")).include(native.join("third_party/rubberband"));
    for line in list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        build.file(native.join(line));
    }
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build.define("NOMINMAX", None).flag("/EHsc");
    }
    build.compile("flibuste_align");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-lib=pthread");
    }
}
