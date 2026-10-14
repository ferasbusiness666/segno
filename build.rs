fn main() {
    // Embed the icon, manifest and version info into the .exe when building for Windows.
    // The version comes from Cargo.toml, so a release only needs the version bumped there.
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target != "windows" {
        return;
    }
    println!("cargo:rerun-if-changed=assets/segno.rc");
    println!("cargo:rerun-if-changed=assets/segno.manifest");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap().replace('\\', "/");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let num = |k: &str| std::env::var(k).unwrap_or_else(|_| "0".into());
    let commas = format!(
        "{},{},{},0",
        num("CARGO_PKG_VERSION_MAJOR"),
        num("CARGO_PKG_VERSION_MINOR"),
        num("CARGO_PKG_VERSION_PATCH")
    );
    let rc = std::fs::read_to_string("assets/segno.rc")
        .unwrap()
        .replace("{ICON}", &format!("{dir}/assets/icon.ico"))
        .replace("{MANIFEST}", &format!("{dir}/assets/segno.manifest"))
        .replace("{VERSION_COMMAS}", &commas)
        .replace("{VERSION}", &version);
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("segno.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
