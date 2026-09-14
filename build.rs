use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=VIPS_LIB_DIR");

    if std::env::var_os("CARGO_FEATURE_LIBVIPS_BACKEND").is_none() {
        return;
    }

    if let Some(path) = std::env::var_os("VIPS_LIB_DIR") {
        let path = PathBuf::from(path);
        println!("cargo:rustc-link-search=native={}", path.display());
    }

    // rs-vips links libvips on Windows, but its generated bindings also call
    // GLib/GObject APIs directly. The crate's build script only emits these
    // transitive links on non-Windows targets, so add the matching import
    // libraries from the official Windows development bundle here.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-lib=dylib=libglib-2.0");
        println!("cargo:rustc-link-lib=dylib=libgobject-2.0");
    }
}
