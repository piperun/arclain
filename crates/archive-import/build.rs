fn main() {
    // The UnRAR native archive library uses Windows registry and CryptoAPI
    // symbols. Its sys crate does not declare this import library itself.
    if std::env::var_os("CARGO_FEATURE_WORKER").is_some()
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
    {
        println!("cargo:rustc-link-lib=advapi32");
    }
}
