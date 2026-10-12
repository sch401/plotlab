#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--smoke-test") {
        if let Err(error) = app::smoke_test() {
            let _ = std::fs::write("smoke-test-error.txt", error);
            std::process::exit(1);
        }
        return Ok(());
    }
    let screenshot = args
        .windows(2)
        .find(|a| a[0] == "--screenshot")
        .map(|a| a[1].clone());
    let demo = args.iter().any(|a| a == "--demo");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1536.0, 930.0])
            .with_min_inner_size([1100.0, 720.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "PlotLab - UNTITLED - [Book1]",
        options,
        Box::new(move |cc| Ok(Box::new(app::PlotLab::new(cc, screenshot, demo)))),
    )
}
