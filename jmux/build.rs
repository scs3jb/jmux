fn main() {
    println!("cargo:rustc-link-arg-bin=jmux-app=-Wl,-rpath,$ORIGIN");
}
