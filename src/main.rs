#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

fn main() -> eframe::Result {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1320.0, 820.0])
            .with_min_inner_size([1040.0, 680.0]),
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "行业划型核对工具",
        native_options,
        Box::new(|creation_context| {
            let app = app::IndustryCheckApp::new(creation_context)?;
            Ok(Box::new(app))
        }),
    )
}
