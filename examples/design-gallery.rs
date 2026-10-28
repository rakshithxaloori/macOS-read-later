//! Read-only synthetic states for repeatable native design review. No database or transport.
use egui::{RichText, vec2};
use macos_read_later::{
    backend::TransferKind,
    domain::{Feed, FeedActivity, FeedId},
    imports::{FeedItem, Item, Problem, Progress},
    ui,
    whatsapp::{ConnectionState, WhatsAppView},
};

struct Gallery {
    screen: usize,
    state: usize,
    phone: String,
    qr: egui::TextureHandle,
    last_action: String,
}
impl Gallery {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        macos_read_later::app::configure(&cc.egui_ctx);
        let wake = cc.egui_ctx.clone();
        macos_read_later::native_menu::install(std::sync::Arc::new(move || wake.request_repaint()));
        let code = qrcode::QrCode::new(b"Synthetic design preview. Not a pairing code.")
            .expect("Fixed sample fits QR");
        let side = code.width() + 8;
        let mut pixels = vec![egui::Color32::WHITE; side * side];
        for y in 0..code.width() {
            for x in 0..code.width() {
                if code[(x, y)] == qrcode::Color::Dark {
                    pixels[(y + 4) * side + x + 4] = egui::Color32::BLACK;
                }
            }
        }
        Self {
            screen: 0,
            state: 0,
            phone: "14155550123".into(),
            qr: cc.egui_ctx.load_texture(
                "synthetic-qr",
                egui::ColorImage::new([side, side], pixels),
                egui::TextureOptions::NEAREST,
            ),
            last_action: String::new(),
        }
    }
}
impl eframe::App for Gallery {
    fn persist_egui_memory(&self) -> bool {
        false
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        for command in macos_read_later::native_menu::drain() {
            match command {
                macos_read_later::native_menu::Command::Show => {
                    macos_read_later::native_menu::activate();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                macos_read_later::native_menu::Command::Quit => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                _ => {}
            }
        }
        egui::Panel::top("preview-controls").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Design gallery · synthetic data only").strong());
                for (n, title) in ["Feeds", "Import / export", "WhatsApp"]
                    .into_iter()
                    .enumerate()
                {
                    if ui.selectable_label(self.screen == n, title).clicked() {
                        self.screen = n;
                        self.state = 0;
                    }
                }
                let states: &[&str] = match self.screen {
                    0 => &["Populated", "Empty", "Queued", "Checking"],
                    1 => &[
                        "Preview",
                        "Importing",
                        "Completed",
                        "Failed",
                        "Empty",
                        "Queued",
                        "Preparing preview",
                        "Exporting",
                    ],
                    _ => &[
                        "Disconnected",
                        "Connecting",
                        "Pairing",
                        "Connected",
                        "Disconnecting",
                        "Failed",
                        "Expired QR",
                        "Waiting for QR",
                    ],
                };
                egui::ComboBox::from_id_salt("sample-state")
                    .selected_text(states[self.state])
                    .show_ui(ui, |ui| {
                        for (n, label) in states.iter().enumerate() {
                            ui.selectable_value(&mut self.state, n, *label);
                        }
                    });
                if ui.button("Compact").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(800., 600.)));
                }
                if ui.button("Desktop").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(1100., 800.)));
                }
            });
        });
        egui::Panel::bottom("preview-note").show(ui, |ui| {
            ui.label("Preview only. No connections, messages, imports, exports or library changes occur.");
            if !self.last_action.is_empty() { ui.label(&self.last_action); }
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(macos_read_later::theme::palette(&ctx).paper).inner_margin(28)).show(ui, |ui| {
            let mut actions = vec![];
            match self.screen {
                0 => {
                    let feeds = if self.state == 1 { vec![] } else { vec![
                        Feed { id: FeedId("sample-active".into()), url: "https://example.com/essays/feed.xml".into(), title: "Essays on making things".into(), active: true, last_polled_at: Some("2026-10-21T09:00:00Z".into()), error: None, activity: match self.state { 2 => FeedActivity::Queued, 3 => FeedActivity::Running, _ => FeedActivity::Idle } },
                        Feed { id: FeedId("sample-paused".into()), url: "https://example.org/notes/rss".into(), title: "Notes from the field".into(), active: false, last_polled_at: None, error: None, activity: FeedActivity::Idle },
                        Feed { id: FeedId("sample-failed".into()), url: "https://example.net/feed".into(), title: "A publication that needs attention".into(), active: true, last_polled_at: None, error: Some("Publisher returned HTTP 503. Try again when the publication is available.".into()), activity: FeedActivity::Idle },
                    ]};
                    ui::secondary::feeds(ui, &feeds, &mut actions);
                }
                1 => {
                    let run = Progress {
                        id: "sample-import".into(), filename: "weekend-reading.csv".into(), format: "URL CSV".into(),
                        state: ["preview", "running", "completed", "failed", "preview", "queued", "preview", "completed"][self.state].into(), total: 12, invalid: 1,
                        pending: match self.state { 0 | 5 => 12, 1 => 7, _ => 0 },
                        added: match self.state { 1 | 3 => 4, 2 | 7 => 11, _ => 0 },
                        duplicates: u32::from(matches!(self.state, 1 | 2 | 3 | 7)),
                        failed: if self.state == 3 { 7 } else { 0 },
                        error: (self.state == 3).then(|| "Some rows could not be imported. Successful rows are already saved.".into()),
                        warnings: serde_json::json!(["Metadata and previews are retained. Full article copies are fetched on demand."]),
                        validation_errors: serde_json::json!([{"row":12,"message":"This row does not contain a valid HTTP or HTTPS address."}]),
                        preview_rows: vec![Item::Feed(FeedItem { key: "sample".into(), url: "https://example.com/feed".into(), title: "Essays on making things".into(), active: true, metadata: serde_json::json!({}) })],
                        row_errors: if self.state == 3 { vec![Problem { row: 7, message: "The import was interrupted. Retry this row.".into() }] } else { vec![] },
                    };
                    let activity = match self.state { 6 => Some(TransferKind::Import), 7 => Some(TransferKind::Export), _ => None };
                    ui::import_export::draw(ui, if matches!(self.state, 4 | 6) { &[] } else { std::slice::from_ref(&run) }, activity, &mut actions);
                }
                _ => {
                    let state = WhatsAppView {
                        state: [ConnectionState::Disconnected, ConnectionState::Connecting, ConnectionState::Pairing, ConnectionState::Connected, ConnectionState::Disconnecting, ConnectionState::Failed, ConnectionState::Pairing, ConnectionState::Pairing][self.state],
                        phone: (self.state > 0).then(|| self.phone.clone()), qr: None,
                        qr_expires_at: Some(chrono::Utc::now().timestamp() + if self.state == 6 { -1 } else { 120 }),
                        error: (self.state == 5).then(|| "The connection was interrupted. Try connecting again.".into()),
                    };
                    ui::whatsapp::draw(ui, &state, &mut self.phone, false, if self.state == 7 { None } else { Some(&self.qr) }, &mut actions);
                }
            }
            if !actions.is_empty() { self.last_action = "Preview action received. No operation was performed.".into(); }
        });
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Q)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
fn main() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Read Later design gallery")
            .with_inner_size(vec2(1100., 800.))
            .with_min_inner_size(vec2(700., 550.)),
        persist_window: false,
        persistence_path: Some(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(".local/design-polish/gallery.ron"),
        ),
        ..Default::default()
    };
    eframe::run_native(
        "Read Later design gallery",
        options,
        Box::new(|cc| Ok(Box::new(Gallery::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
