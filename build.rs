// Older libprocessing checkouts have no Texture draw command; with one of
// those, textured shapes draw untextured. LIBPROCESSING_DIR names the
// checkout the build swaps in (RustMode's IDE build and LuaMode's
// build-libprocessing.sh set it); empty or unset means the pinned upstream
// commit, which has them.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(lp_no_texture)");
    println!("cargo::rustc-check-cfg=cfg(lp_attribs)");
    println!("cargo::rustc-check-cfg=cfg(lp_window_state)");
    println!("cargo::rustc-check-cfg=cfg(lp_depth_test)");
    println!("cargo:rerun-if-env-changed=LIBPROCESSING_DIR");
    let dir = std::env::var("LIBPROCESSING_DIR").ok().filter(|d| !d.is_empty()).map(std::path::PathBuf::from);
    let Some(dir) = dir else { return };
    // the lp-fork passes a material's own vertex attributes (attrib())
    let custom = dir.join("crates/processing_render/src/material/custom.rs");
    println!("cargo:rerun-if-changed={}", custom.display());
    if std::fs::read_to_string(&custom).is_ok_and(|s| s.contains("pub fn custom_attribute")) {
        println!("cargo:rustc-cfg=lp_attribs");
    }
    // the lp-fork reports focus, hover and position for window events
    let glfw = dir.join("crates/processing_glfw/src/lib.rs");
    println!("cargo:rerun-if-changed={}", glfw.display());
    if std::fs::read_to_string(&glfw).is_ok_and(|s| s.contains("pub fn window_state")) {
        println!("cargo:rustc-cfg=lp_window_state");
    }
    let file = dir.join("crates/processing_render/src/render/command.rs");
    println!("cargo:rerun-if-changed={}", file.display());
    if let Ok(src) = std::fs::read_to_string(&file) {
        // the lp-fork can turn the depth test off (hint(DISABLE_DEPTH_TEST))
        if src.contains("DepthTest(bool)") { println!("cargo:rustc-cfg=lp_depth_test"); }
        if !src.contains("NoTexture") {
            println!("cargo:rustc-cfg=lp_no_texture");
            println!("cargo:warning=your libprocessing checkout has no texture commands; textured shapes draw untextured. Update it to get textures.");
        }
    }
}
