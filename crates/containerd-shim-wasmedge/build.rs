fn main() {
    // The wasmedge static library is not compiled with -fPIE.
    // We need to override rustc default of generating PIE executables.
    println!("cargo:rustc-link-arg-bins=-no-pie");
}
