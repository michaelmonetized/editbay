use editbay_app::studio::Studio;
use eframe::egui;
use std::path::PathBuf;

fn main() -> eframe::Result {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--picture-worker")) {
        if let Err(error) = editbay_media::picture_worker::serve() {
            eprintln!("editbay picture worker: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--media-worker")) {
        if let Err(error) = editbay_media::worker::serve() {
            eprintln!("editbay media worker: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let state = std::env::var_os("EDITBAY_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_STATE_HOME").map(|path| PathBuf::from(path).join("editbay"))
        })
        .unwrap_or_else(|| home.join(".local/state/editbay"));
    let root = std::env::var_os("EDITBAY_CATALOG_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.clone());
    let paths: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    eframe::run_native(
        "EditBay",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_app_id("editbay")
                .with_inner_size([1440., 900.])
                .with_min_inner_size([640., 480.]),
            renderer: eframe::Renderer::Wgpu,
            ..Default::default()
        },
        Box::new(move |cc| {
            Ok(Box::new(Studio::new(
                cc.egui_ctx.clone(),
                home,
                root,
                state,
                paths,
                rfd::AsyncFileDialog::new().set_parent(cc),
            )))
        }),
    )
}
