use super::{
    Action,
    controls::{self, ButtonKind},
    icons::Icon,
};
use crate::whatsapp::{ConnectionState, WhatsAppView};
use egui::{RichText, Ui};

pub fn draw(
    ui: &mut Ui,
    state: &WhatsAppView,
    phone: &mut String,
    offline: bool,
    qr: Option<&egui::TextureHandle>,
    actions: &mut Vec<Action>,
) {
    let colors = crate::theme::palette(ui.ctx());
    egui::ScrollArea::vertical().id_salt("whatsapp").show(ui, |ui| {
        ui.set_max_width(ui.available_width().min(680.));
        super::secondary::header(ui, "WhatsApp", "Save a link from your own chat.");
        controls::panel(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Your connection").size(18.).color(colors.ink));
                controls::status(ui, match state.state {
                    ConnectionState::Connected => controls::Status::Connected,
                    ConnectionState::Failed => controls::Status::Failed,
                    ConnectionState::Connecting => controls::Status::Connecting,
                    ConnectionState::Disconnecting => controls::Status::Disconnecting,
                    ConnectionState::Pairing => controls::Status::Pairing,
                    ConnectionState::Disconnected => controls::Status::Disconnected,
                });
            });
            ui.add_space(10.);
            ui.label(RichText::new(match state.state {
                ConnectionState::Disconnected => "Connect your phone to save articles without leaving WhatsApp.",
                ConnectionState::Connecting => "Preparing a secure connection to your phone.",
                ConnectionState::Pairing => "Link this app from WhatsApp on your phone.",
                ConnectionState::Connected => "Ready for links from your own chat. Intake continues when this window is closed.",
                ConnectionState::Disconnecting => "Closing your WhatsApp connection.",
                ConnectionState::Failed => "The connection needs your attention. You can try connecting again.",
            }).size(13.).color(colors.muted));
            if let Some(error) = &state.error {
                ui.add_space(12.);
                ui.label(RichText::new("WhatsApp couldn't complete the connection.").color(colors.danger));
                egui::CollapsingHeader::new("Connection details").show(ui, |ui| { ui.label(error); });
            }
            ui.add_space(18.);
            match state.state {
                ConnectionState::Disconnected | ConnectionState::Failed => {
                    let label = ui.label(RichText::new("Country code and phone number").size(12.).color(colors.ink));
                    controls::field(ui, phone, "For example, 14155550123").labelled_by(label.id);
                    ui.label(RichText::new("Include the country code. Use digits only.").size(11.).color(colors.muted));
                    let validation = crate::whatsapp::validate_phone(phone);
                    if !phone.is_empty() && let Err(error) = &validation {
                        ui.label(RichText::new(error.to_string()).size(12.).color(colors.danger));
                    }
                    ui.add_space(14.);
                    ui.add_enabled_ui(!offline && validation.is_ok(), |ui| {
                        if controls::action(ui, "Connect WhatsApp", ButtonKind::Primary).clicked() {
                            actions.push(Action::WhatsAppConnect(phone.clone()));
                        }
                    });
                    if state.state == ConnectionState::Failed
                        && controls::action(ui, "Disconnect", ButtonKind::Quiet).clicked()
                    {
                        actions.push(Action::WhatsAppDisconnect);
                    }
                }
                ConnectionState::Connecting | ConnectionState::Pairing | ConnectionState::Connected => {
                    if let Some(number) = &state.phone {
                        ui.label(RichText::new(format!("Phone · {number}")).size(12.).color(colors.muted));
                        ui.add_space(8.);
                    }
                    if state.state == ConnectionState::Pairing {
                        pairing(ui, state, qr);
                        ui.add_space(16.);
                    }
                    if controls::action(ui, "Disconnect", ButtonKind::Secondary).clicked() {
                        actions.push(Action::WhatsAppDisconnect);
                    }
                }
                ConnectionState::Disconnecting => {}
            }
            if offline {
                ui.add_space(12.);
                ui.label(RichText::new("Offline verification mode. Relaunch without --offline to connect.").size(12.).color(colors.muted));
            }
        });
        ui.add_space(20.);
        controls::panel(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(20., 20.), egui::Sense::hover());
                super::icons::draw(ui, rect, Icon::Message, colors.green);
                ui.label(RichText::new("Save from your self-chat").size(18.).color(colors.ink));
            });
            ui.add_space(10.);
            ui.label("Send exactly one HTTP or HTTPS link and the standalone word save, separated by whitespace.");
            ui.add_space(12.);
            egui::Frame::new().fill(colors.soft).corner_radius(8).inner_margin(14).show(ui, |ui| {
                ui.label(RichText::new("save https://example.com/article").monospace().size(12.).color(colors.green));
            });
            ui.add_space(10.);
            ui.label(RichText::new("Only new outgoing messages in your own chat are accepted. Bare links and messages with multiple links are ignored.").size(12.).color(colors.muted));
        });
    });
}

fn pairing(ui: &mut Ui, state: &WhatsAppView, qr: Option<&egui::TextureHandle>) {
    let colors = crate::theme::palette(ui.ctx());
    let remaining = state
        .qr_expires_at
        .map(|expiry| expiry.saturating_sub(chrono::Utc::now().timestamp()));
    if remaining.is_some_and(|seconds| seconds <= 0) {
        ui.label(RichText::new("This QR code has expired.").color(colors.danger));
        ui.label("Disconnect, then connect again to request a fresh code.");
        return;
    }
    if let Some(qr) = qr {
        ui.label("1. Open WhatsApp on your phone.");
        ui.label("2. Open Settings → Linked devices → Link a device.");
        ui.label("3. Scan this code.");
        ui.add_space(12.);
        egui::Frame::new()
            .fill(egui::Color32::WHITE)
            .corner_radius(10)
            .inner_margin(16)
            .show(ui, |ui| {
                let side = (ui.available_width() - 4.).clamp(80., 240.);
                ui.add(egui::Image::new(qr).fit_to_exact_size(egui::vec2(side, side)));
            });
        if let Some(seconds) = remaining {
            ui.add_space(8.);
            ui.label(
                RichText::new(format!("Code expires in {seconds} seconds"))
                    .size(11.)
                    .color(colors.muted),
            );
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
        }
    } else {
        ui.label("Waiting for your QR code…");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> egui::Context {
        let context = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        let proportional = fonts.families[&egui::FontFamily::Proportional].clone();
        fonts
            .families
            .insert(egui::FontFamily::Name("Georgia".into()), proportional);
        context.set_fonts(fonts);
        context.set_theme(egui::Theme::Light);
        context.style_mut_of(egui::Theme::Light, |style| style.animation_time = 0.);
        context
    }

    fn frame(
        context: &egui::Context,
        state: &WhatsAppView,
        offline: bool,
        events: Vec<egui::Event>,
        actions: &mut Vec<Action>,
    ) -> egui::FullOutput {
        let mut output = context.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    draw(ui, state, &mut "14155550123".into(), offline, None, actions);
                });
            },
        );
        // This headless interaction proof deliberately has no texture renderer.
        output.textures_delta.clear();
        output
    }

    fn label_center(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
    }

    fn click(
        context: &egui::Context,
        state: &WhatsAppView,
        offline: bool,
        position: egui::Pos2,
        actions: &mut Vec<Action>,
    ) {
        for pressed in [true, false] {
            frame(
                context,
                state,
                offline,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                actions,
            );
        }
    }

    #[test]
    fn invalid_phone_stays_inline_and_cannot_queue_a_connection() {
        let context = context();
        let state = WhatsAppView::default();
        let mut phone = "not a number".to_owned();
        let mut actions = Vec::new();
        let mut render = |events| {
            let mut output = context.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |root| {
                    egui::CentralPanel::default().show(root, |ui| {
                        draw(ui, &state, &mut phone, false, None, &mut actions)
                    });
                },
            );
            output.textures_delta.clear();
            output
        };
        let output = render(Vec::new());
        assert!(
            label_center(
                &output,
                "Enter your country code and phone number using 8–15 digits"
            )
            .is_some()
        );
        let position = label_center(&output, "Connect WhatsApp")
            .expect("disabled connect remains discoverable");
        for pressed in [true, false] {
            render(vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
        assert!(
            actions.is_empty(),
            "invalid phone must not reach the transport owner"
        );
    }

    #[test]
    fn connection_card_routes_actions_and_respects_offline_mode() {
        for offline in [false, true] {
            let context = context();
            let state = WhatsAppView::default();
            let mut actions = Vec::new();
            let output = frame(&context, &state, offline, Vec::new(), &mut actions);
            assert!(label_center(&output, "Disconnect").is_none());
            let position = label_center(&output, "Connect WhatsApp").expect("connect action");
            click(&context, &state, offline, position, &mut actions);
            if offline {
                assert!(actions.is_empty(), "offline verification must not connect");
            } else {
                assert!(
                    matches!(actions.as_slice(), [Action::WhatsAppConnect(phone)] if phone == "14155550123")
                );
            }
        }
        let context = context();
        let state = WhatsAppView {
            state: ConnectionState::Connected,
            ..Default::default()
        };
        let mut actions = Vec::new();
        let output = frame(&context, &state, false, Vec::new(), &mut actions);
        assert!(label_center(&output, "Connect WhatsApp").is_none());
        let position = label_center(&output, "Disconnect").expect("disconnect action");
        click(&context, &state, false, position, &mut actions);
        assert!(matches!(actions.as_slice(), [Action::WhatsAppDisconnect]));
    }
}
