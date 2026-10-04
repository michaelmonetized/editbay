fn main() {
    let mut adapter = cc::Build::new();
    adapter
        .files(["native/codec.c", "native/reader.c"])
        .flag_if_supported("-std=c11")
        .warnings_into_errors(true);
    for library in [
        "libavformat",
        "libavcodec",
        "libavutil",
        "libswscale",
        "libswresample",
    ] {
        let native = pkg_config::Config::new()
            .probe(library)
            .expect("FFmpeg development libraries are required");
        for path in native.include_paths {
            adapter.include(path);
        }
    }
    adapter.compile("editbay_codec");
    println!("cargo:rerun-if-changed=native/codec.c");
    println!("cargo:rerun-if-changed=native/reader.c");
}
