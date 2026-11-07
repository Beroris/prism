//! Regenerate the icon files: `cargo run -p prism-core --example make_ico`.
//! Writes `assets/app.ico`, `assets/icon.png` and the copy the Windows exe embeds,
//! `platforms/windows/res/app.ico`.
fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ico = prism_core::icon::build_ico(&[16, 20, 24, 32, 40, 48, 64, 256]);
    std::fs::write(root.join("assets/app.ico"), &ico).unwrap();
    std::fs::write(root.join("platforms/windows/res/app.ico"), &ico).unwrap();
    prism_core::icon::render_rgba(256).save_png(root.join("assets/icon.png")).unwrap();
    println!("wrote assets/app.ico, assets/icon.png, platforms/windows/res/app.ico");
}
