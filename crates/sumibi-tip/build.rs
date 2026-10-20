fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // COM entry points are resolved with GetProcAddress, not linked by clients.
        println!("cargo:rustc-link-arg-cdylib=/EXPORT:DllGetClassObject,PRIVATE");
        println!("cargo:rustc-link-arg-cdylib=/EXPORT:DllCanUnloadNow,PRIVATE");
    }
}
