fn main() {
    // DuckDB's AdditionalLockInfo() uses Windows Restart Manager API.
    // The bundled build doesn't link rstrtmgr.lib automatically.
    #[cfg(target_os = "windows")]
    println!("cargo:rustc-link-lib=rstrtmgr");
}
