fn main() {
    println!("cargo::rerun-if-changed=link.x");
    if std::env::var("TARGET").as_deref() == Ok("armv4t-none-eabi") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        if std::env::var_os("CARGO_FEATURE_RAM_TRIAL").is_some() {
            println!("cargo::rustc-link-arg-bin=tekmar-482-firmware=--defsym=__ram_trial=1");
        }
        println!("cargo::rustc-link-arg-bin=tekmar-482-firmware=-T{dir}/link.x");
    }
}
