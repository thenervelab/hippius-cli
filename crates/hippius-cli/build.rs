//! Records the target triple the binary is built for, which `hipc version` reports so it
//! can be matched against the release asset names.

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for every build script");
    println!("cargo::rustc-env=HIPC_TARGET={target}");
    println!("cargo::rerun-if-changed=build.rs");
}
