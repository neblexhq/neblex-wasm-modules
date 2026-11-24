// Patched for sandboxed modules: the original asks git for the release tag, which inside
// another repository would stamp that repository's commit into the build. Use the crate version.
fn main() {
    println!("cargo:rustc-env=GIT_VERSION=v{}", env!("CARGO_PKG_VERSION"));
}
