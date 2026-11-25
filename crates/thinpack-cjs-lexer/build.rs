fn main() {
    cc::Build::new()
        .cpp(true)
        .std("c++20")
        .file("vendor/merve.cpp")
        .include("vendor")
        .warnings(false)
        .compile("merve");
    println!("cargo:rerun-if-changed=vendor/merve.cpp");
    println!("cargo:rerun-if-changed=vendor/merve.h");
}
