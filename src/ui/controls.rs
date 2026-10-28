//! Shared presentation only. Screens own actions; backend owners own effects.
use super::icons;
use egui::{Color32, Response, RichText, Stroke, Ui, vec2};

pub const ACTION_HEIGHT: f32 = 34.;

/// Egui uses the inactive widget fill for slider rails. Scope that palette so
/// paper-filled buttons and fields do not make the rail disappear.
pub fn slider(ui: &mut Ui, slider: egui::Slider<'_>) -> Response {
    let colors = crate::theme::palette(ui.ctx());
    ui.scope(|ui| {
        let width = (ui.available_width() * 0.36).clamp(96., 160.);
        ui.spacing_mut().slider_width = width;
        ui.spacing_mut().slider_rail_height = 4.;
        let visuals = ui.visuals_mut();
        visuals.widgets.inactive.bg_fill = colors.divider;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1., colors.green);
        visuals.selection.bg_fill = colors.green;
        ui.add(
            slider
                .trailing_fill(true)
                .handle_shape(egui::style::HandleShape::Circle),
        )
    })
    .inner
}

/// A right-aligned footer that cannot expand into its parent's remaining height.
pub fn action_row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        vec2(width, ACTION_HEIGHT),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.set_min_width(width);
            add(ui)
        },
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Quiet,
    Danger,
}

/// Semantic button styles share geometry and interaction handling.
pub fn action(ui: &mut Ui, label: &str, kind: ButtonKind) -> Response {
    let colors = crate::theme::palette(ui.ctx());
    let (fill, border, ink) = match kind {
        ButtonKind::Primary => (colors.green, colors.green, colors.on_accent),
        ButtonKind::Secondary => (colors.reader, colors.divider, colors.ink),
        ButtonKind::Quiet => (Color32::TRANSPARENT, Color32::TRANSPARENT, colors.green),
        ButtonKind::Danger => (colors.danger_fill, colors.danger_border, colors.danger),
    };
    let response = ui.add(
        egui::Button::new(RichText::new(label).size(12.).color(ink))
            .fill(fill)
            .stroke(Stroke::new(1., border))
            .corner_radius(8)
            .min_size(vec2(0., ACTION_HEIGHT)),
    );
    interaction_fill(ui, &response, false);
    focus(ui, &response);
    response
}

pub fn icon_button(ui: &mut Ui, icon: icons::Icon, label: &str, selected: bool) -> Response {
    icon_button_plain(ui, icon, label, selected).on_hover_text(label)
}

/// Toolbar buttons sit directly above the native article view, which covers
/// egui layers. Their labels open beside the button, inside the toolbar band.
pub fn toolbar_button(ui: &mut Ui, icon: icons::Icon, label: &str, selected: bool) -> Response {
    let response = icon_button_plain(ui, icon, label, selected);
    side_tooltip(&response, label);
    response
}

pub fn side_tooltip(response: &Response, label: &str) {
    let mut tooltip = egui::Tooltip::for_enabled(response);
    tooltip.popup = tooltip
        .popup
        .align(egui::emath::RectAlign::LEFT)
        .align_alternatives(&[egui::emath::RectAlign::RIGHT]);
    tooltip.show(|ui| {
        ui.label(label);
    });
}

fn icon_button_plain(ui: &mut Ui, icon: icons::Icon, label: &str, selected: bool) -> Response {
    let colors = crate::theme::palette(ui.ctx());
    let response = ui.add(
        egui::Button::new("")
            .fill(if selected {
                colors.selected
            } else {
                Color32::TRANSPARENT
            })
            .stroke(Stroke::NONE)
            .corner_radius(7)
            .min_size(vec2(30., 30.)),
    );
    interaction_fill(ui, &response, selected);
    icons::draw(
        ui,
        egui::Rect::from_center_size(response.rect.center(), vec2(17., 17.)),
        icon,
        if response.enabled() {
            colors.green
        } else {
            colors.muted
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), label)
    });
    focus(ui, &response);
    response
}

/// A compact segmented choice. Each segment is its own accessible button, so
/// keyboard and VoiceOver users reach every option directly.
pub fn segmented(ui: &mut Ui, labels: &[&str], selected: usize) -> Option<usize> {
    let colors = crate::theme::palette(ui.ctx());
    let font = egui::FontId::proportional(11.5);
    let widths: Vec<f32> = labels
        .iter()
        .map(|label| {
            ui.fonts_mut(|f| f.layout_no_wrap((*label).to_owned(), font.clone(), colors.ink))
                .size()
                .x
                + 20.
        })
        .collect();
    let (outer, _) = ui.allocate_exact_size(
        vec2(widths.iter().sum::<f32>() + 4., 26.),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(outer, 7., colors.soft);
    let mut chosen = None;
    let mut x = outer.left() + 2.;
    for (index, (label, width)) in labels.iter().zip(&widths).enumerate() {
        let rect = egui::Rect::from_min_size(egui::pos2(x, outer.top() + 2.), vec2(*width, 22.));
        x += width;
        let response = ui.interact(
            rect,
            ui.id().with(("segment", *label)),
            egui::Sense::click(),
        );
        let active = index == selected;
        if active {
            ui.painter().rect_filled(rect, 6., colors.reader);
            ui.painter().rect_stroke(
                rect,
                6.,
                Stroke::new(1., colors.divider),
                egui::StrokeKind::Inside,
            );
        } else if response.hovered() {
            ui.painter().rect_filled(rect, 6., colors.hover);
        }
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            font.clone(),
            if active { colors.ink } else { colors.muted },
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, active, *label)
        });
        focus(ui, &response);
        if response.clicked() && !active {
            chosen = Some(index);
        }
    }
    chosen
}

/// The caller paints its base fill first. Only the hover transition repaints;
/// a settled control schedules no frames. Zero animation time honors reduced motion.
pub fn interaction_fill(ui: &Ui, response: &Response, selected: bool) {
    let duration = ui.style().animation_time;
    let hover = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        response.enabled() && response.hovered(),
        duration,
    );
    let pressed = ui.ctx().animate_bool_with_time(
        response.id.with("pressed"),
        response.enabled() && response.is_pointer_button_down_on(),
        duration.min(0.07),
    );
    let alpha = ((if selected { 10. } else { 8. }) * hover + 12. * pressed).round() as u8;
    if alpha > 0 {
        ui.painter().rect_filled(
            response.rect,
            8.,
            if ui.visuals().dark_mode {
                Color32::from_white_alpha(alpha)
            } else {
                Color32::from_black_alpha(alpha)
            },
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Loading,
    Queued,
    Complete,
    Partial,
    Unavailable,
    PreviewUnavailable,
    Failed,
    Paused,
    Completed,
    Draft,
    Finalized,
    Active,
    Verified,
    Waiting,
    Checking,
    Connecting,
    Connected,
    Disconnected,
    Pairing,
    Disconnecting,
}
impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Loading => "Loading",
            Self::Queued => "Queued",
            Self::Complete => "Complete copy",
            Self::Partial => "Partial copy",
            Self::Unavailable => "Copy unavailable",
            Self::PreviewUnavailable => "Preview unavailable",
            Self::Failed => "Needs attention",
            Self::Paused => "Paused",
            Self::Completed => "Completed",
            Self::Draft => "Draft",
            Self::Finalized => "Finalized",
            Self::Active => "Active",
            Self::Verified => "Verified",
            Self::Waiting => "Waiting",
            Self::Checking => "Checking",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Disconnected => "Disconnected",
            Self::Pairing => "Pairing",
            Self::Disconnecting => "Disconnecting",
        }
    }
    pub fn color(self, ctx: &egui::Context) -> Color32 {
        let colors = crate::theme::palette(ctx);
        match self {
            Self::Failed | Self::Unavailable | Self::PreviewUnavailable => colors.danger,
            Self::Partial => colors.warning,
            Self::Loading
            | Self::Queued
            | Self::Paused
            | Self::Draft
            | Self::Waiting
            | Self::Checking
            | Self::Connecting
            | Self::Disconnected
            | Self::Pairing
            | Self::Disconnecting => colors.muted,
            Self::Complete
            | Self::Completed
            | Self::Finalized
            | Self::Active
            | Self::Verified
            | Self::Connected => colors.green,
        }
    }
    fn icon(self) -> icons::Icon {
        match self {
            Self::Failed | Self::Unavailable | Self::PreviewUnavailable | Self::Partial => {
                icons::Icon::Warning
            }
            Self::Loading
            | Self::Queued
            | Self::Waiting
            | Self::Checking
            | Self::Connecting
            | Self::Disconnecting => icons::Icon::Refresh,
            Self::Pairing => icons::Icon::Message,
            Self::Draft => icons::Icon::Newspaper,
            Self::Paused | Self::Disconnected => icons::Icon::Square,
            Self::Complete
            | Self::Completed
            | Self::Finalized
            | Self::Active
            | Self::Verified
            | Self::Connected => icons::Icon::Check,
        }
    }
}
pub fn status(ui: &mut Ui, state: Status) {
    let color = state.color(ui.ctx());
    egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            20,
        ))
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.;
                let (rect, _) = ui.allocate_exact_size(vec2(12., 12.), egui::Sense::hover());
                icons::draw(ui, rect, state.icon(), color);
                ui.label(RichText::new(state.label()).size(11.).color(color));
            });
        });
}
pub fn copy_status(ui: &mut Ui, state: crate::domain::CopyStatus) {
    status(
        ui,
        match state {
            crate::domain::CopyStatus::Pending => Status::Queued,
            crate::domain::CopyStatus::Saved => Status::Complete,
            crate::domain::CopyStatus::Partial => Status::Partial,
            crate::domain::CopyStatus::Failed => Status::Unavailable,
        },
    );
}

pub fn focus(ui: &Ui, response: &Response) {
    let colors = crate::theme::palette(ui.ctx());
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.),
            9.,
            Stroke::new(1.5, colors.green),
            egui::StrokeKind::Outside,
        );
    }
}

pub fn panel(ui: &Ui) -> egui::Frame {
    let colors = crate::theme::palette(ui.ctx());
    egui::Frame::new()
        .fill(colors.reader)
        .stroke(Stroke::new(1., colors.divider))
        .corner_radius(12)
        .inner_margin(22)
}

pub fn badge(ui: &mut Ui, label: &str, color: Color32) {
    egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            20,
        ))
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(11.).color(color));
        });
}

pub fn field(ui: &mut Ui, value: &mut String, hint: &str) -> Response {
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(ui.available_width())
            .margin(vec2(12., 9.)),
    )
}

pub fn empty(ui: &mut Ui, symbol: icons::Icon, title: &str, description: &str) {
    let colors = crate::theme::palette(ui.ctx());
    ui.add_space(24.);
    egui::Frame::new()
        .fill(colors.paper)
        .inner_margin(24)
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(32., 32.), egui::Sense::hover());
            icons::draw(ui, rect, symbol, colors.green);
            ui.add_space(14.);
            ui.label(RichText::new(title).size(18.).color(colors.ink));
            ui.add_space(6.);
            ui.label(RichText::new(description).size(13.).color(colors.muted));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_footer_stays_compact_and_right_aligned_in_tall_parents() {
        for height in [600., 800., 1200.] {
            let context = egui::Context::default();
            let mut footer = egui::Rect::NOTHING;
            let mut primary = egui::Rect::NOTHING;
            let mut content_right = 0.;
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        vec2(800., height),
                    )),
                    ..Default::default()
                },
                |root| {
                    egui::CentralPanel::default().show(root, |ui| {
                        ui.set_width(440.);
                        content_right = ui.max_rect().right();
                        ui.label("Save a link");
                        ui.add_space(18.);
                        footer = action_row(ui, |ui| {
                            primary = action(ui, "Save link", ButtonKind::Primary).rect;
                            action(ui, "Cancel", ButtonKind::Quiet);
                        })
                        .response
                        .rect;
                    });
                },
            );
            output.textures_delta.clear();
            assert!((footer.height() - ACTION_HEIGHT).abs() < 1.);
            assert!((primary.right() - content_right).abs() < 1.);
            assert!(footer.contains_rect(primary));
        }
    }
}
