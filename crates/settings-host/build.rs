//! Embeds the RedSnap icon (packaging/icons/redsnap.ico) in the Windows
//! executable, so Explorer, Start and the taskbar show it.

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/redsnap.ico");
    // The target, not the host: this runs on Linux for cross builds too.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let ico = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/icons/redsnap.ico");
    let ico = ico.canonicalize().expect("packaging/icons/redsnap.ico");
    let rc = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("redsnap.rc");
    // Forward slashes: the resource compiler reads backslashes as escapes.
    std::fs::write(&rc, format!("1 ICON \"{}\"\n", ico.display().to_string().replace('\\', "/"))).unwrap();
    embed_resource::compile(&rc, embed_resource::NONE).manifest_optional().unwrap();
}
