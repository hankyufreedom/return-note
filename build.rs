fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-changed=assets/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("app-resource.o");
    let rc = std::env::var("WINDRES").unwrap_or_else(|_| "x86_64-w64-mingw32-windres".into());
    let result = std::process::Command::new(rc)
        .args(["-i", "assets/app.rc", "-o"])
        .arg(&out)
        .status()
        .expect("Windows build needs llvm-mingw windres on PATH");
    assert!(result.success(), "Resource compiler failed: {result}");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}
