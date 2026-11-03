//! The update banner draws state only. The app owns checks and installation.
use super::controls::{self, ButtonKind};
use egui::RichText;

#[derive(Clone, Copy, Debug)]
pub enum Banner<'a> {
    Checking,
    Available(&'a str),
    Installing { version: &'a str, step: &'a str },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Install,
    Later,
    Skip,
}

/// A bottom panel reserves layout space so native reader views never cover it.
pub fn draw(root: &mut egui::Ui, banner: Banner<'_>) -> Option<Choice> {
    let colors = crate::theme::palette(root.ctx());
    let mut choice = None;
    egui::Panel::bottom("update-banner")
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(colors.selected)
                .inner_margin(egui::Margin::symmetric(16, 8)),
        )
        .show(root, |ui| {
            ui.set_min_height(controls::ACTION_HEIGHT);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.;
                match banner {
                    Banner::Checking => {
                        ui.add(egui::Spinner::new().size(12.).color(colors.green));
                        ui.label(
                            RichText::new("Checking for updates…")
                                .size(12.)
                                .color(colors.ink),
                        );
                    }
                    Banner::Installing { version, step } => {
                        ui.add(egui::Spinner::new().size(12.).color(colors.green));
                        ui.label(
                            RichText::new(format!("Installing Reader {version}"))
                                .size(12.)
                                .color(colors.ink),
                        );
                        ui.label(RichText::new(step).size(12.).color(colors.muted));
                    }
                    Banner::Available(version) => {
                        ui.label(
                            RichText::new(format!("Reader {version} is available"))
                                .size(12.)
                                .strong()
                                .color(colors.ink),
                        );
                        ui.label(
                            RichText::new(format!("You have {}.", env!("CARGO_PKG_VERSION")))
                                .size(12.)
                                .color(colors.muted),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if controls::action(ui, "Install and Relaunch", ButtonKind::Primary)
                                .clicked()
                            {
                                choice = Some(Choice::Install);
                            }
                            if controls::action(ui, "Later", ButtonKind::Quiet).clicked() {
                                choice = Some(Choice::Later);
                            }
                            if controls::action(ui, "Skip This Version", ButtonKind::Quiet)
                                .clicked()
                            {
                                choice = Some(Choice::Skip);
                            }
                        });
                    }
                }
            });
        });
    choice
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_banner_keeps_its_actions_on_one_line() {
        for width in [800., 1240.] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let run = || {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 700.),
                        )),
                        ..Default::default()
                    },
                    |root| {
                        draw(root, Banner::Available("9.9.9"));
                    },
                );
                // No renderer consumes atlas uploads in this harness.
                output.textures_delta.clear();
                output
            };
            let _ = run();
            let output = run();
            let nodes = &output.platform_output.accesskit_update.unwrap().nodes;
            let bounds = |label: &str| {
                nodes
                    .iter()
                    .find(|(_, n)| n.label() == Some(label) || n.value() == Some(label))
                    .and_then(|(_, n)| n.bounds())
                    .unwrap_or_else(|| panic!("missing {label}"))
            };
            let title = bounds("Reader 9.9.9 is available");
            for button in ["Install and Relaunch", "Later", "Skip This Version"] {
                let b = bounds(button);
                assert!(
                    b.y0 < title.y1 && title.y0 < b.y1,
                    "{button} wrapped at {width}"
                );
                assert!(b.x1 <= width as f64 && b.x0 > title.x1);
            }
            assert!(700. - title.y0 < 60., "banner is not a single compact row");
        }
    }
}
