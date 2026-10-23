fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=logo.ico");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        if let Err(err) = winresource::WindowsResource::new()
            .set_icon("logo.ico")
            .set("FileDescription", "Unknown Dispatch")
            .set("ProductName", "Unknown Dispatch")
            .set("OriginalFilename", "dispatch.exe")
            .set("LegalCopyright", "MIT License")
            .compile()
        {
            println!("cargo:warning=Could not embed the Windows resources (app icon): {err}");
        }
    }
}
