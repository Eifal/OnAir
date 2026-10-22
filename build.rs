fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    println!("cargo:rerun-if-changed=assets/onair.ico");

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/onair.ico");
    res.set("ProductName", "OnAir");
    res.set("FileDescription", "OnAir: Voicemeeter mic overlay");
    res.set("LegalCopyright", "Copyright (C) 2026 Eifal");
    res.set("FileVersion", env!("CARGO_PKG_VERSION"));
    res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
    res.compile().expect("failed to compile Windows resources");
}
