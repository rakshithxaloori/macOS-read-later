//! Application appearance. Magazine typography, artwork and PDF bytes are independent.
use egui::Color32;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", from = "String")]
pub enum Appearance {
    #[default]
    Light,
    Dark,
}
impl From<String> for Appearance {
    fn from(value: String) -> Self {
        if value == "dark" {
            Self::Dark
        } else {
            Self::Light
        }
    }
}
impl Appearance {
    pub fn egui(self) -> egui::Theme {
        match self {
            Self::Light => egui::Theme::Light,
            Self::Dark => egui::Theme::Dark,
        }
    }
    pub fn palette(self) -> Palette {
        let rgb = Color32::from_rgb;
        match self {
            Self::Light => Palette {
                paper: rgb(252, 251, 248),
                sidebar: rgb(243, 242, 236),
                ink: rgb(44, 48, 44),
                muted: rgb(133, 135, 127),
                divider: rgb(231, 230, 223),
                green: rgb(49, 84, 68),
                selected: rgb(229, 234, 222),
                article: rgb(248, 248, 244),
                reader: rgb(255, 254, 251),
                soft: rgb(242, 244, 237),
                danger: rgb(153, 61, 50),
                danger_fill: rgb(252, 239, 235),
                danger_border: rgb(231, 202, 195),
                warning: rgb(153, 103, 61),
                warning_fill: rgb(252, 242, 235),
                hover: rgb(238, 239, 232),
                on_accent: rgb(255, 254, 251),
            },
            Self::Dark => Palette {
                // Warm charcoal paper with three distinct depths: rail, list, reader.
                paper: rgb(30, 29, 27),
                sidebar: rgb(22, 21, 20),
                ink: rgb(236, 232, 223),
                muted: rgb(164, 158, 147),
                divider: rgb(54, 52, 48),
                green: rgb(159, 195, 164),
                selected: rgb(44, 58, 48),
                article: rgb(27, 26, 24),
                reader: rgb(32, 31, 29),
                soft: rgb(42, 41, 38),
                danger: rgb(237, 158, 145),
                danger_fill: rgb(65, 39, 34),
                danger_border: rgb(112, 69, 60),
                warning: rgb(220, 176, 128),
                warning_fill: rgb(61, 47, 32),
                hover: rgb(38, 37, 34),
                on_accent: rgb(24, 35, 27),
            },
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub paper: Color32,
    pub sidebar: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub divider: Color32,
    pub green: Color32,
    pub selected: Color32,
    pub article: Color32,
    pub reader: Color32,
    pub soft: Color32,
    pub danger: Color32,
    pub danger_fill: Color32,
    pub danger_border: Color32,
    pub warning: Color32,
    pub warning_fill: Color32,
    pub hover: Color32,
    pub on_accent: Color32,
}
pub fn palette(ctx: &egui::Context) -> Palette {
    if ctx.theme() == egui::Theme::Dark {
        Appearance::Dark
    } else {
        Appearance::Light
    }
    .palette()
}

pub fn apply(ctx: &egui::Context, appearance: Appearance) {
    ctx.set_theme(appearance.egui());
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(match appearance {
        Appearance::Light => egui::SystemTheme::Light,
        Appearance::Dark => egui::SystemTheme::Dark,
    }));
    ctx.request_repaint();
}

/// Configure both variants once so switching does not reload system fonts.
pub fn configure(ctx: &egui::Context, reduce_motion: bool) {
    ctx.set_theme(egui::Theme::Light);
    for appearance in [Appearance::Light, Appearance::Dark] {
        let colors = appearance.palette();
        let mut style = (*ctx.style_of(appearance.egui())).clone();
        style.visuals = match appearance {
            Appearance::Light => egui::Visuals::light(),
            Appearance::Dark => egui::Visuals::dark(),
        };
        style.visuals.override_text_color = Some(colors.ink);
        style.visuals.panel_fill = colors.paper;
        style.visuals.window_fill = colors.paper;
        style.visuals.selection.bg_fill = colors.selected;
        style.visuals.selection.stroke = egui::Stroke::new(1., colors.green);
        style.visuals.hyperlink_color = colors.green;
        style.visuals.extreme_bg_color = colors.reader;
        let widgets = &mut style.visuals.widgets;
        widgets.noninteractive.bg_stroke = egui::Stroke::new(1., colors.divider);
        widgets.noninteractive.fg_stroke.color = colors.ink;
        widgets.inactive.bg_fill = colors.reader;
        widgets.inactive.weak_bg_fill = colors.reader;
        widgets.inactive.bg_stroke = egui::Stroke::new(1., colors.divider);
        widgets.hovered.bg_fill = colors.soft;
        widgets.hovered.weak_bg_fill = colors.soft;
        widgets.hovered.bg_stroke = egui::Stroke::new(1., colors.muted);
        widgets.active.bg_fill = colors.selected;
        widgets.active.weak_bg_fill = colors.selected;
        widgets.active.bg_stroke = egui::Stroke::new(1.5, colors.green);
        widgets.open = widgets.active;
        for widget in [
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.open,
        ] {
            widget.corner_radius = egui::CornerRadius::same(8);
            widget.fg_stroke.color = colors.ink;
        }
        style.visuals.window_corner_radius = egui::CornerRadius::same(14);
        style.visuals.window_stroke = egui::Stroke::new(1., colors.divider);
        style.visuals.window_fill = colors.reader;
        style.visuals.faint_bg_color = colors.soft;
        style.visuals.text_cursor.stroke = egui::Stroke::new(1.5, colors.green);
        style.spacing.item_spacing = egui::vec2(8., 8.);
        style.spacing.button_padding = egui::vec2(12., 8.);
        style.spacing.interact_size.y = 30.;
        style.spacing.combo_height = 240.;
        style.animation_time = if reduce_motion { 0. } else { 0.12 };
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(13.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(12.));
        ctx.set_style_of(appearance.egui(), style);
    }
}
