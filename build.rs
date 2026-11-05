fn main() {
    println!("cargo:rerun-if-changed=packaging/windows/filetree233.ico");
    #[cfg(windows)]
    {
        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
        let mut numbers = version
            .split(['-', '+'])
            .next()
            .unwrap_or("0.0.0")
            .split('.')
            .map(|part| part.parse::<u16>().unwrap_or(0));
        let major = numbers.next().unwrap_or(0);
        let minor = numbers.next().unwrap_or(0);
        let patch = numbers.next().unwrap_or(0);
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("packaging/windows/filetree233.ico");
        resource.set("FileDescription", "filetree233 disk space explorer");
        resource.set("ProductName", "filetree233");
        resource.set("CompanyName", "neko233");
        resource.set("LegalCopyright", "Copyright (c) 2026 neko233");
        let packed = (u64::from(major) << 48) | (u64::from(minor) << 32) | (u64::from(patch) << 16);
        resource.set_version_info(winresource::VersionInfo::FILEVERSION, packed);
        resource.set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed);
        if let Err(error) = resource.compile() {
            println!("cargo:warning=could not embed Windows resources: {error}");
        }
    }
}
