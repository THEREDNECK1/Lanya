fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/lanya.ico");
        res.set("ProductName", "Lanya");
        res.set("FileDescription", "Lanya - Collaborative DaVinci Resolve Suite");
        res.set("LegalCopyright", "Copyright \u{00a9} Redneck Wrench Customs");
        let _ = res.compile();
    }
}
