use super::icons::{Icon, draw as icon};
use super::*;
use egui::{Sense, Ui, vec2};
fn control(ui: &mut Ui, symbol: Icon, label: &str, selected: bool, filled: bool) -> bool {
    let colors = crate::theme::palette(ui.ctx());
    let (rect, r) = ui.allocate_exact_size(vec2(44., 44.), Sense::click());
    if filled || selected {
        ui.painter().rect_filled(
            rect,
            6.,
            if filled {
                colors.green
            } else {
                colors.selected
            },
        );
    }
    super::controls::interaction_fill(ui, &r, selected);
    if selected {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.left_center() - vec2(8., 9.), vec2(3., 18.)),
            1.,
            colors.green,
        );
    }
    super::controls::focus(ui, &r);
    icon(
        ui,
        egui::Rect::from_center_size(rect.center(), vec2(18., 18.)),
        symbol,
        if filled {
            colors.on_accent
        } else {
            colors.green
        },
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    r.on_hover_text(label).clicked()
}
pub fn draw(ui: &mut Ui, current: View, actions: &mut Vec<Action>) {
    let colors = crate::theme::palette(ui.ctx());
    egui::Frame::new()
        .fill(colors.sidebar)
        .inner_margin(egui::Margin {
            left: 10,
            right: 10,
            top: TITLEBAR_INSET as i8 + 12,
            bottom: 16,
        })
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 0.;
                let (brand, _) = ui.allocate_exact_size(vec2(44., 36.), Sense::hover());
                icon(
                    ui,
                    egui::Rect::from_center_size(brand.center(), vec2(24., 24.)),
                    Icon::Book,
                    colors.green,
                );
                ui.add_space(20.);
                if control(ui, Icon::Plus, "Save link", false, true) {
                    actions.push(Action::SaveDialog);
                }
                ui.add_space(20.);
                for (view, symbol) in [
                    (View::Inbox, Icon::Inbox),
                    (View::Archive, Icon::Archive),
                    (View::Feeds, Icon::Rss),
                    (View::Magazines, Icon::Newspaper),
                    (View::WhatsApp, Icon::Message),
                    (View::ImportExport, Icon::Download),
                ] {
                    if view == View::Feeds {
                        ui.add_space(16.);
                        ui.separator();
                        ui.add_space(16.);
                    }
                    if control(ui, symbol, view.label(), view == current, false) {
                        actions.push(Action::Navigate(view));
                    }
                    ui.add_space(4.);
                }
                ui.allocate_space(vec2(0., (ui.available_height() - 48.).max(0.)));
                let dark = ui.visuals().dark_mode;
                if control(
                    ui,
                    if dark { Icon::Sun } else { Icon::Moon },
                    if dark {
                        "Switch to light theme"
                    } else {
                        "Switch to dark theme"
                    },
                    false,
                    false,
                ) {
                    actions.push(Action::Appearance(if dark {
                        crate::theme::Appearance::Light
                    } else {
                        crate::theme::Appearance::Dark
                    }));
                }
            });
        });
}
