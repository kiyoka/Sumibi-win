fn main() {
    println!("cargo:rerun-if-changed=assets/Sumibi.rc");
    println!("cargo:rerun-if-changed=assets/Sumibi.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile_for("assets/Sumibi.rc", ["sumibi-host"], embed_resource::NONE)
            .manifest_required()
            .expect("compile the Sumibi app icon");
    }
}
