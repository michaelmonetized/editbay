fn main() {
    let mut adapter = cc::Build::new();
    adapter
        .files(["native/codec.c", "native/reader.c", "native/color.c"])
        .flag_if_supported("-std=c11")
        .warnings_into_errors(true);
    let libraries = [
        "libavformat",
        "libavcodec",
        "libavutil",
        "libswscale",
        "libswresample",
        "lcms2",
    ];
    for library in libraries {
        let native = pkg_config::Config::new()
            .cargo_metadata(false)
            .probe(library)
            .expect("Native media/color development libraries are required");
        for path in native.include_paths {
            adapter.include(path);
        }
    }
    adapter.compile("editbay_codec");
    for library in libraries {
        pkg_config::Config::new()
            .probe(library)
            .expect("Native media/color development libraries are required");
    }
    println!("cargo:rerun-if-changed=native/codec.c");
    println!("cargo:rerun-if-changed=native/reader.c");
    println!("cargo:rerun-if-changed=native/color.c");
    println!("cargo:rerun-if-changed=native/color_metadata.h");
}
