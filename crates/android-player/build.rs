fn main() {
    println!("cargo:rerun-if-changed=src/native_output.cpp");
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("src/native_output.cpp")
        .cpp_link_stdlib("c++_static")
        .compile("node_forge_native_output");
    println!("cargo:rustc-link-lib=static=c++abi");
    println!("cargo:rustc-link-arg=-Wl,--no-undefined");
    println!("cargo:rustc-link-lib=android");
    println!("cargo:rustc-link-lib=vulkan");
    println!("cargo:rustc-link-lib=dl");
}
