fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    assert!(
        target.contains("-musl"),
        "jellysink builds musl-only ({target}); see .cargo/config.toml. \
         Fix the build error rather than switching to a glibc target."
    );
}
