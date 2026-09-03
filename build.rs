//! Build-time guard: jellysink is built against musl, never glibc.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("musl") {
        return;
    }

    let target = std::env::var("TARGET").unwrap_or_else(|_| "<unknown>".to_owned());
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "x86_64".to_owned());

    panic!(
        "jellysink must be built against musl, but the target is `{target}`.

Build it like this:

    rustup target add {arch}-unknown-linux-musl
    cargo build --release --target {arch}-unknown-linux-musl

The repository's `.cargo/config.toml` already selects a musl target for a bare
`cargo build`; something overrode it (an explicit `--target`, a
`CARGO_BUILD_TARGET` in the environment, or a build from outside the repo).
A musl C toolchain is required as well: `musl` on Arch, `musl-tools` on Debian."
    );
}
