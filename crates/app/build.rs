fn main() {
    println!("cargo:rustc-check-cfg=cfg(cocoa_check)");
    tauri_build::build();
    // macOS : intègre Info.plist à l'exécutable (section __TEXT,__info_plist), pour que le système y trouve
    // NSMicrophoneUsageDescription même sans paquet d'application. `tauri-build` ne le fait pas dans ce cas.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        println!("cargo:rerun-if-changed=Info.plist");
        println!("cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{dir}/Info.plist");
    }
}
