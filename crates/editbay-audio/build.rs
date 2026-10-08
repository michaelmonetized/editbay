fn main() {
    let native = pkg_config::Config::new()
        .atleast_version("1.4.0")
        .probe("libpipewire-0.3")
        .expect("PipeWire 1.4.0 development libraries are required");
    let mut adapter = cc::Build::new();
    adapter
        .file("native/output.c")
        .flag_if_supported("-std=c11")
        .warnings_into_errors(true);
    for path in native.include_paths {
        adapter.include(path);
    }
    adapter.compile("editbay_pipewire_output");
    println!("cargo:rerun-if-changed=native/output.c");
}
