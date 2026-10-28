use clap::Parser;
#[derive(Parser)]
#[command(version, about = "Native local Read Later")]
struct Cli {
    #[arg(long)]
    data: Option<std::path::PathBuf>,
    #[arg(long)]
    offline: bool,
}
/// The running Dock icon matches the bundle's AppIcon.icns instead of eframe's default.
fn app_icon() -> std::sync::Arc<egui::IconData> {
    let image = image::load_from_memory(include_bytes!("../packaging/macos/icon/icon-256.png"))
        .expect("Bundled app icon is a valid PNG")
        .into_rgba8();
    let (width, height) = image.dimensions();
    std::sync::Arc::new(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "storage-audit") {
        return macos_read_later::storage::audit_cli(&args[2..]);
    }
    if args.get(1).is_some_and(|a| a == "render-magazine") {
        let value = |flag: &str| {
            args.windows(2)
                .find(|p| p[0] == flag)
                .map(|p| std::path::PathBuf::from(&p[1]))
                .ok_or_else(|| anyhow::anyhow!("Missing {flag}"))
        };
        return macos_read_later::production_renderer::render_job(
            &value("--input")?,
            &value("--output")?,
        )
        .map_err(anyhow::Error::msg);
    }
    let cli = Cli::parse();
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.parent()?
                .parent()
                .map(|p| p.join("Resources/default-data-directory"))
        })
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|p| std::path::PathBuf::from(p.trim()))
        .filter(|p| p.is_absolute());
    let dirs =
        macos_read_later::paths::AppDirs::discover(cli.data.as_deref().or(bundled.as_deref()))?;
    std::fs::create_dir_all(
        dirs.data
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Data parent missing"))?,
    )?;
    let _maintenance = macos_read_later::instance::MaintenanceGuard::claim(&dirs.data)?;
    dirs.ensure()?;
    let Some(instance) = macos_read_later::instance::Instance::claim(&dirs.data)? else {
        return Ok(());
    };
    {
        let c = macos_read_later::db_compat::open(&dirs.database())?;
        let has_articles: i64 = c.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='articles'",
            [],
            |r| r.get(0),
        )?;
        if has_articles == 0 {
            macos_read_later::db_compat::initialize_lean(&c)?;
        } else {
            anyhow::ensure!(
                macos_read_later::db_compat::is_lean(&c)?,
                "This library uses the retired full-copy format. Open it with Reading List v0.1.0 to convert it first."
            );
            macos_read_later::db_compat::initialize(&c)?;
        }
        if macos_read_later::db_compat::is_lean(&c)? {
            let storage = macos_read_later::storage::Storage::for_database(&c)?;
            if let Ok(exe) = std::env::current_exe()
                && let Some(bundle) = exe
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.parent())
                    .filter(|p| p.extension().is_some_and(|ext| ext == "app"))
            {
                let bytes = macos_read_later::storage::measured_bytes(bundle)?;
                storage.set_bundle_bytes(bytes)?;
            }
            anyhow::ensure!(
                storage.snapshot()?.total_bytes() < macos_read_later::storage::LIMIT_BYTES,
                "The app and library exceed the 100 MB storage limit"
            );
        }
    }
    let settings = macos_read_later::settings::Settings::load(&dirs.settings());
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Read Later")
        .with_inner_size(settings.window_size)
        .with_min_inner_size([800., 600.])
        // One continuous surface: the panes run under a transparent title bar.
        .with_fullsize_content_view(true)
        .with_title_shown(false)
        .with_titlebar_shown(false)
        .with_icon(app_icon());
    if let Some(p) = settings.window_position
        && p.iter()
            .all(|v| v.is_finite() && (-1000.0..=5000.0).contains(v))
    {
        viewport = viewport.with_position(p);
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        persist_window: false,
        persistence_path: Some(dirs.config.join("egui.ron")),
        ..Default::default()
    };
    eframe::run_native(
        "Read Later",
        options,
        Box::new(move |cc| {
            Ok(Box::new(macos_read_later::app::App::new(
                cc,
                dirs,
                cli.offline,
                instance,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("Native window failed: {e}"))
}
