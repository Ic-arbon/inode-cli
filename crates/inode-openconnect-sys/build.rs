fn main() {
    println!("cargo:rerun-if-changed=src/progress_shim.c");
    println!("cargo:rerun-if-env-changed=OPENCONNECT_H3C_LIB");
    println!("cargo:rerun-if-env-changed=OPENCONNECT_H3C_INCLUDE");

    cc::Build::new()
        .file("src/progress_shim.c")
        .warnings(false)
        .compile("inode_oc_progress_shim");

    if std::env::var_os("CARGO_FEATURE_FFI").is_some() {
        if let Ok(lib) = std::env::var("OPENCONNECT_H3C_LIB") {
            println!("cargo:rustc-link-search=native={lib}/lib");
        }
        println!("cargo:rustc-link-lib=dylib=openconnect");
    }
}
