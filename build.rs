// Older libprocessing checkouts have no Texture draw command; with one of
// those, textured shapes draw untextured. LIBPROCESSING_DIR names the
// checkout the build swaps in (RustMode's IDE build and LuaMode's
// build-libprocessing.sh set it); empty or unset means the pinned upstream
// commit, which has them.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(lp_no_texture)");
    println!("cargo:rerun-if-env-changed=LIBPROCESSING_DIR");
    let dir = std::env::var("LIBPROCESSING_DIR").ok().filter(|d| !d.is_empty()).map(std::path::PathBuf::from);
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
