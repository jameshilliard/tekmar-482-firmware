fn main() {
    println!("cargo::rerun-if-env-changed=TEKMAR_BUILD_REVISION");
    let rev = std::env::var("TEKMAR_BUILD_REVISION").unwrap_or_else(|_| "unversioned".into());
    assert!(
        rev == "unversioned"
            || (rev.len() == 40 && rev.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))),
        "TEKMAR_BUILD_REVISION must be a lowercase Git commit ID"
    );
    println!("cargo::rustc-env=TEKMAR_BUILD_REVISION={rev}");
    println!("cargo::rerun-if-changed=link.x");
    if std::env::var("TARGET").as_deref() == Ok("armv4t-none-eabi") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        if std::env::var_os("CARGO_FEATURE_RAM_TRIAL").is_some() {
            println!("cargo::rustc-link-arg-bin=tekmar-482-firmware=--defsym=__ram_trial=1");
        }
        println!("cargo::rustc-link-arg-bin=tekmar-482-firmware=-T{dir}/link.x");
    }
}
