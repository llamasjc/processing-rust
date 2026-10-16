// Older libprocessing checkouts have no Texture draw command. When the
// checkout the build uses (LIBPROCESSING_DIR, or sketchbook/libprocessing
// next to the modes folder) lacks it, textured shapes draw untextured.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(lp_no_texture)");
    println!("cargo:rerun-if-env-changed=LIBPROCESSING_DIR");
    let dir = std::env::var("LIBPROCESSING_DIR").ok().map(std::path::PathBuf::from).or_else(|| {
        let here = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").ok()?);
        Some(here.parent()?.parent()?.join("libprocessing"))
    });
    let Some(dir) = dir else { return };
    let file = dir.join("crates/processing_render/src/render/command.rs");
    println!("cargo:rerun-if-changed={}", file.display());
    if let Ok(src) = std::fs::read_to_string(&file) {
        if !src.contains("NoTexture") {
            println!("cargo:rustc-cfg=lp_no_texture");
            println!("cargo:warning=your libprocessing checkout has no texture commands; textured shapes draw untextured. Update it to get textures.");
        }
    }
}
