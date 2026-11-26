fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo::rerun-if-changed=../../link.x");
    println!("cargo::rustc-link-arg=--defsym=__ram_trial=1");
    println!("cargo::rustc-link-arg=-T{dir}/../../link.x");
}
