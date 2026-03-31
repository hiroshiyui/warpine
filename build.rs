// build.rs — Ensure SDL2 library path is on the linker search path.
//
// On Debian/Ubuntu the SDL2 shared library lives in
// /usr/lib/x86_64-linux-gnu/ which rust-lld does not search by default.
// We query pkg-config for the library directory and emit the appropriate
// cargo:rustc-link-search directive so the build works without requiring
// LIBRARY_PATH to be set manually.

fn main() {
    if let Ok(lib_dir) = pkg_config_libdir("sdl2") {
        println!("cargo:rustc-link-search=native={}", lib_dir);
    }
}

fn pkg_config_libdir(lib: &str) -> Result<String, std::io::Error> {
    let out = std::process::Command::new("pkg-config")
        .args(["--variable=libdir", lib])
        .output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(std::io::Error::other("pkg-config failed"))
    }
}
