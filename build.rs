fn main() {
    println!("cargo:rerun-if-changed=resources/AppIcon.ico");
    println!("cargo:rerun-if-changed=resources/windows.manifest");

    let target = std::env::var("TARGET").unwrap_or_default();
    let host = std::env::var("HOST").unwrap_or_default();
    if !target.contains("windows") || !host.contains("windows") {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("resources/AppIcon.ico")
        .set_manifest_file("resources/windows.manifest")
        .set("ProductName", "Harbor Light")
        .set("FileDescription", "AI coding agent traffic light status window")
        .set("LegalCopyright", "MIT")
        .set("ProductVersion", env!("CARGO_PKG_VERSION"))
        .set("FileVersion", env!("CARGO_PKG_VERSION"));
    resource.compile().expect("compile Windows resources");
}
