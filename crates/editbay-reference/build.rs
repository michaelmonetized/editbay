use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=EDITBAY_LIBTORCH");
    println!("cargo:rerun-if-changed=native/reference.cpp");
    if std::env::var_os("CARGO_FEATURE_LIBTORCH").is_none() {
        return;
    }
    let root = PathBuf::from(
        std::env::var_os("EDITBAY_LIBTORCH")
            .expect("select verified native libtorch with EDITBAY_LIBTORCH"),
    );
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("native/reference.cpp")
        .flag(format!("-isystem{}", root.join("include").display()))
        .flag(format!(
            "-isystem{}",
            root.join("include/torch/csrc/api/include").display()
        ))
        .warnings_into_errors(true)
        .compile("editbay_torch_reference");
    let library = root.join("lib");
    println!("cargo:rustc-link-search=native={}", library.display());
    for name in ["torch", "torch_cpu", "c10"] {
        println!("cargo:rustc-link-lib=dylib={name}");
    }
}
