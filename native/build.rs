fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        if std::path::Path::new("assets/larrez.ico").exists() {
            res.set_icon("assets/larrez.ico");
        }
        res.set("ProductName", "Larrez Player");
        res.set("FileDescription", "Larrez Player - lightweight multi-format video player");
        res.set("LegalCopyright", "MIT Licensed");
        let _ = res.compile();
    }
}
