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
}
