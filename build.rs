fn main() {
    // libudfread is bundled inside tokimo-lib (transitive dep of libbluray).
    // FFMPEG_LIBS_DIR points to tokimo-lib/current/lib (set by .cargo/config.toml or CI).
    if let Ok(libs_dir) = std::env::var("FFMPEG_LIBS_DIR") {
        println!("cargo:rustc-link-search=native={libs_dir}");
    }
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    println!("cargo:rustc-link-lib=udfread");
    println!("cargo:rerun-if-changed=build.rs");
}
