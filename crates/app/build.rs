//! Windows: put the app icon inside Codemux.exe, so Explorer, the taskbar
//! and Start menu shortcuts show it. Other platforms need nothing here.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon-256.png");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_icon();
    }
}

#[cfg(windows)]
fn windows_icon() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("codemux.ico");
    let png = std::fs::File::open("assets/icon-256.png").unwrap();
    let image = ico::IconImage::read_png(png).unwrap();
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    dir.add_entry(ico::IconDirEntry::encode(&image).unwrap());
    dir.write(std::fs::File::create(&out).unwrap()).unwrap();
    winresource::WindowsResource::new().set_icon(out.to_str().unwrap()).compile().unwrap();
}
