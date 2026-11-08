// Windows: embed the app icon and version into the exe (Explorer, taskbar, shortcuts).
fn main() {
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("assets/icon.ico")
        .set("ProductName", "UWIN Studio")
        .set("FileDescription", "UWIN Studio - DGUS screen editor")
        .compile()
        .expect("embed icon");
}
