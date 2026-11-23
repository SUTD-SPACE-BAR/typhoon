// A windowed app on Windows: no console window pops up, in any build.
// (Tests keep the console so their output is visible.)
#![cfg_attr(not(test), windows_subsystem = "windows")]

mod app;
mod background;
mod designer;
mod job;
mod planner;
mod preview;
mod serial;
mod settings;
mod sound;
mod theme;

use eframe::egui;

fn main() -> eframe::Result {
    install_crash_log();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Typhoon — Filament Winder")
        .with_app_id("typhoon")
        // Fits a 1024x768 or 1366x768 laptop when un-maximised; starts maximised
        // on first run, after which the saved window geometry wins.
        .with_inner_size([1100.0, 680.0])
        .with_min_inner_size([760.0, 520.0])
        .with_maximized(true);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        // A second copy (e.g. for testing) can keep its own settings.
        persistence_path: std::env::var_os("TYPHOON_DATA_DIR")
            .map(|dir| std::path::PathBuf::from(dir).join("app.ron")),
        ..Default::default()
    };
    eframe::run_native(
        "Typhoon",
        options,
        Box::new(|cc| Ok(Box::new(app::TyphoonApp::new(cc)))),
    )
}

/// With no console window, a crash would vanish without a trace, so panics
/// are also appended to `crash.log` next to the saved settings.
fn install_crash_log() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(dir) = eframe::storage_dir("Typhoon") {
            let _ = std::fs::create_dir_all(&dir);
            let text = format!(
                "[{:?}] Typhoon {} crashed: {info}\n{}\n\n",
                std::time::SystemTime::now(),
                env!("CARGO_PKG_VERSION"),
                std::backtrace::Backtrace::force_capture()
            );
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("crash.log"))
            {
                let _ = f.write_all(text.as_bytes());
            }
        }
        default_hook(info);
    }));
}

/// Window/taskbar icon rendered from the same logo SVG.
fn window_icon() -> Option<egui::IconData> {
    let img = background::rasterize_logo(64)?;
    let rgba = img
        .pixels
        .iter()
        .flat_map(|c| c.to_srgba_unmultiplied())
        .collect();
    Some(egui::IconData {
        rgba,
        width: img.size[0] as u32,
        height: img.size[1] as u32,
    })
}
