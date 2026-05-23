fn main() {
    // libudfread is only used for Blu-ray ISO playback. On Unix the FFI is unconditionally
    // compiled in (see src/lib.rs gated on cfg(unix)), so we must link the library on
    // every Unix host. Windows support TBD.
    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-lib=udfread");
    #[cfg(target_os = "macos")]
    {
        // Homebrew default prefix on Apple Silicon and Intel respectively.
        for prefix in ["/opt/homebrew", "/usr/local"] {
            println!("cargo:rustc-link-search=native={prefix}/lib");
        }
        println!("cargo:rustc-link-lib=udfread");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
