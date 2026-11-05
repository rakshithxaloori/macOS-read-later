use super::controls::{self, ButtonKind};
use super::icons::Icon;
use super::*;
use crate::{
    backend::Command,
    domain::{Article, date_label},
    reader_style::ReaderFont,
    rich_content::RichReader,
};
use egui::{RichText, Ui};
pub struct ReaderView<'a> {
    pub article: Option<&'a Article>,
    pub reader: &'a RichReader,
    pub size: f32,
    pub font: ReaderFont,
    pub loading: bool,
    pub error: Option<&'a str>,
    pub has_copy: bool,
    pub fetched_at: Option<&'a str>,
    pub local_copy: bool,
    pub saving_copy: bool,
    pub native: bool,
}
/// Height of the reader toolbar below the title-bar inset.
pub const TOOLBAR_HEIGHT: f32 = 52.;

#[derive(Default)]
pub struct ReaderLayout {
    pub body: Option<egui::Rect>,
    pub font_picker: Option<egui::Rect>,
}

pub fn draw(ui: &mut Ui, view: ReaderView<'_>, actions: &mut Vec<Action>) -> ReaderLayout {
    let colors = crate::theme::palette(ui.ctx());
    let ReaderView {
        article,
        reader,
        size,
        font,
        loading,
        error,
        has_copy,
        fetched_at,
        local_copy,
        saving_copy,
        native,
    } = view;
    let viewport = ui.max_rect();
    let mut layout = ReaderLayout::default();
    egui::Frame::new().fill(colors.reader).show(ui, |ui| {
        ui.set_min_size(ui.available_size());
        ui.spacing_mut().item_spacing.y = 0.;
        if article.is_some() || loading {
            ui.add_space(TITLEBAR_INSET);
            let (toolbar, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), TOOLBAR_HEIGHT), egui::Sense::hover());
            ui.scope_builder(egui::UiBuilder::new().max_rect(toolbar.shrink2(egui::vec2(20., 11.))), |ui| {
                ui.horizontal(|ui| {
                    if controls::toolbar_button(ui, Icon::ArrowLeft, "Close article", false).clicked() { actions.push(Action::CloseReader); }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(a) = article {
                            if controls::toolbar_button(ui, Icon::External, "Open original article", false).clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(&a.url));
                            }
                            if controls::toolbar_button(ui, Icon::Trash, "Delete article", false).clicked() {
                                actions.push(Action::ConfirmDelete(vec![a.id.clone()]));
                            }
                            let archived = a.archived_at.is_some();
                            if controls::toolbar_button(ui, Icon::Archive, if archived { "Restore to Inbox" } else { "Archive" }, archived).clicked() {
                                actions.push(Action::Command(Command::Archive { id: a.id.clone(), archived: !archived }));
                            }
                            let read = a.read_at.is_some();
                            if controls::toolbar_button(ui, Icon::Check, if read { "Mark as unread" } else { "Mark as read" }, read).clicked() {
                                actions.push(Action::Command(Command::Read { id: a.id.clone(), read: !read }));
                            }
                            if controls::toolbar_button(ui, Icon::Refresh, if error.is_some() { "Retry article acquisition" } else { "Re-fetch article" }, false).clicked() {
                                actions.push(Action::Command(Command::Refetch(a.id.clone())));
                            }
                            if saving_copy {
                                ui.add_sized(egui::vec2(30., 30.), egui::Spinner::new().size(14.))
                                    .on_hover_text("Saving local copy…");
                            } else if controls::toolbar_button(ui, Icon::Download, if local_copy { "Remove local copy" } else { "Save local copy (keeps this article readable offline)" }, local_copy).clicked() {
                                actions.push(if local_copy { Action::RemoveLocalCopy(a.id.clone()) } else { Action::SaveLocalCopy(a.id.clone()) });
                            }
                            let (gap, _) = ui.allocate_exact_size(egui::vec2(13., 20.), egui::Sense::hover());
                            ui.painter().line_segment([gap.center_top(), gap.center_bottom()], egui::Stroke::new(1., colors.divider));
                        }
                        text_controls(ui, size, font, native, &mut layout, actions);
                    });
                });
            });
            ui.painter().line_segment([toolbar.left_bottom(), toolbar.right_bottom()], egui::Stroke::new(1., colors.divider));
        }
        if loading {
            egui::ScrollArea::vertical().id_salt("reader-loading").auto_shrink([false, false]).show(ui, |ui| {
                reader_column(ui, |ui| {
                    if let Some(a) = article {
                        article_heading(ui, a, font);
                        ui.add_space(24.);
                        if !a.excerpt.is_empty() { ui.label(RichText::new(&a.excerpt).size(14.).color(colors.muted)); ui.add_space(20.); }
                    }
                    ui.horizontal(|ui| { ui.spinner(); ui.label(RichText::new("Loading reading copy…").size(13.).color(colors.green)); });
                    ui.add_space(8.);
                    ui.label(RichText::new("Fetching this article on demand. You can keep browsing your library.").size(12.).color(colors.muted));
                });
            });
            return;
        }
        if let Some(a) = article {
            if native {
                layout.body = Some(egui::Rect::from_min_max(
                    egui::pos2(viewport.left(), viewport.top() + TITLEBAR_INSET + TOOLBAR_HEIGHT),
                    viewport.right_bottom(),
                ));
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt(&a.id.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    reader_column(ui, |ui| {
                            article_heading(ui, a, font);
                            ui.add_space(24.);
                            if error.is_some() || a.status != CopyStatus::Saved || a.error.is_some() {
                                let (title, summary) = if error.is_some() && has_copy {
                                    ("Your reading copy is still available", "The latest fetch did not succeed. You can keep reading this copy, retry, or open the original.")
                                } else if error.is_some() || a.status == CopyStatus::Failed {
                                    ("Reading copy unavailable", "This article could not be fetched. Retry, or open the original on the publisher's website.")
                                } else if a.status == CopyStatus::Partial {
                                    ("Partial reading copy", "This copy is incomplete. You can read the available text, retry, or open the original.")
                                } else if a.status == CopyStatus::Pending {
                                    ("Reading copy pending", "This article is waiting to be fetched. Retry, or open the original on the publisher's website.")
                                } else {
                                    ("Your reading copy is still available", "A recent fetch reported a problem. You can keep reading this copy, retry, or open the original.")
                                };
                                let details = error.or(a.error.as_deref()).unwrap_or(summary);
                                ui.add_space(16.);
                                controls::panel(ui).show(ui, |ui| {
                                    if has_copy && (error.is_some() || a.error.is_some()) {
                                        controls::status(ui, controls::Status::Failed);
                                    } else {
                                        controls::copy_status(ui, if error.is_some() { CopyStatus::Failed } else { a.status });
                                    }
                                    ui.add_space(8.);
                                    ui.label(RichText::new(title).size(14.).strong().color(colors.ink));
                                    ui.add_space(6.);
                                    ui.label(RichText::new(summary).size(12.).color(colors.ink));
                                    ui.add_space(12.);
                                    ui.horizontal_wrapped(|ui| {
                                        if controls::action(ui, "Retry", ButtonKind::Secondary).clicked() { actions.push(Action::Command(Command::Refetch(a.id.clone()))); }
                                        if controls::action(ui, "Open original", ButtonKind::Quiet).clicked() { ui.ctx().open_url(egui::OpenUrl::new_tab(&a.url)); }
                                        if controls::action(ui, "Details", ButtonKind::Quiet).clicked() {
                                            actions.push(Action::ShowDetails { title: title.into(), message: details.into() });
                                        }
                                    });
                                });
                            }
                            if let Some(at) = fetched_at {
                                ui.add_space(12.);
                                ui.label(RichText::new(format!("Fetched {}", date_label(at))).size(11.).color(colors.muted));
                            }
                            ui.add_space(24.);
                            ui.separator();
                            ui.add_space(24.);
                            reader.show(ui, size, font);
                        });
                });
        } else {
            let rect = ui.available_rect_before_wrap();
            let empty = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 136.));
            ui.scope_builder(egui::UiBuilder::new().max_rect(empty), |ui| {
                ui.vertical_centered(|ui| {
                    let (icon, _) =
                        ui.allocate_exact_size(egui::vec2(36., 36.), egui::Sense::hover());
                    super::icons::draw(ui, icon, super::icons::Icon::Book, colors.muted);
                    ui.add_space(24.);
                    ui.label(RichText::new("Select an article").size(22.).color(colors.ink));
                    ui.add_space(12.);
                    ui.label(
                        RichText::new("Choose a title from the list to read the article.")
                            .size(13.)
                            .color(colors.muted),
                    );
                });
            });
        }
    });
    layout
}

/// Font style and size stay behind an "Aa" button until the pointer hovers it,
/// so the toolbar stays quiet while reading. Keyboard focus, an open font menu
/// or a click on "Aa" keep them visible too.
fn text_controls(
    ui: &mut Ui,
    size: f32,
    font: ReaderFont,
    native: bool,
    layout: &mut ReaderLayout,
    actions: &mut Vec<Action>,
) {
    const GRACE: f64 = 0.6;
    let colors = crate::theme::palette(ui.ctx());
    let id = egui::Id::new("reader-text-controls");
    let (previous, pinned, focused, left_at): (Option<egui::Rect>, bool, bool, f64) = ui
        .ctx()
        .data(|d| d.get_temp(id))
        .unwrap_or((None, false, false, f64::NEG_INFINITY));
    let now = ui.input(|i| i.time);
    let hovered = previous
        .zip(ui.input(|i| i.pointer.latest_pos()))
        .is_some_and(|(rect, pointer)| rect.expand(6.).contains(pointer));
    let popup = !native && egui::Popup::is_any_open(ui.ctx());
    let held = hovered || pinned || focused || popup;
    let left_at = if held { now } else { left_at };
    let lingering = now - left_at < GRACE;
    let expanded = held || lingering;
    if lingering && !held {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(GRACE - (now - left_at)));
    }
    let toggle = compact_button(ui, "Aa", "Text style and size");
    let mut rect = toggle.rect;
    let mut any_focus = toggle.has_focus();
    // A click elsewhere unpins; "Aa" itself toggles for keyboard and VoiceOver.
    let clicked_away = ui.input(|i| i.pointer.any_click()) && !hovered && !toggle.hovered();
    let pinned = (pinned && !clicked_away) ^ toggle.clicked();
    if expanded || pinned {
        let increase = ui
            .add_enabled_ui(size < 27., |ui| {
                compact_button(ui, "A+", "Increase reader text size")
            })
            .inner;
        if increase.clicked() {
            actions.push(Action::FontSize((size + 1.).min(27.)));
        }
        let decrease = ui
            .add_enabled_ui(size > 15., |ui| {
                compact_button(ui, "A−", "Decrease reader text size")
            })
            .inner;
        if decrease.clicked() {
            actions.push(Action::FontSize((size - 1.).max(15.)));
        }
        any_focus |= increase.has_focus() || decrease.has_focus();
        rect = rect.union(increase.rect).union(decrease.rect);
        if native {
            let picker = ui
                .allocate_exact_size(egui::vec2(134., 28.), egui::Sense::hover())
                .0;
            layout.font_picker = Some(picker);
            rect = rect.union(picker);
        } else {
            let combo = egui::ComboBox::from_id_salt("reader-font")
                .selected_text(font.family())
                .width(134.)
                .show_ui(ui, |ui| {
                    for (label, choices) in [
                        ("Classic", [ReaderFont::Georgia, ReaderFont::Baskerville]),
                        (
                            "Modern",
                            [ReaderFont::AvenirNext, ReaderFont::HelveticaNeue],
                        ),
                    ] {
                        ui.label(RichText::new(label).small().color(colors.muted));
                        for choice in choices {
                            if ui
                                .selectable_label(font == choice, choice.family())
                                .clicked()
                            {
                                actions.push(Action::ReaderFont(choice));
                            }
                        }
                    }
                })
                .response
                .on_hover_text("Reader font: two classic and two modern styles");
            any_focus |= combo.has_focus();
            rect = rect.union(combo.rect);
        }
    }
    ui.ctx()
        .data_mut(|d| d.insert_temp(id, (Some(rect), pinned, any_focus, left_at)));
    // The pointer only reaches the revealed controls on a later frame.
    if (expanded || pinned) != previous.is_some_and(|p| p.width() > toggle.rect.width() + 1.) {
        ui.ctx().request_repaint();
    }
}

fn compact_button(ui: &mut Ui, label: &str, hint: &str) -> egui::Response {
    let colors = crate::theme::palette(ui.ctx());
    let response = ui.add(
        egui::Button::new("")
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE)
            .corner_radius(7)
            .min_size(egui::vec2(30., 30.)),
    );
    controls::interaction_fill(ui, &response, false);
    ui.painter().text(
        response.rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(12.),
        if response.enabled() {
            colors.green
        } else {
            colors.muted
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), label)
    });
    controls::focus(ui, &response);
    controls::side_tooltip(&response, hint);
    response
}

/// Match the native reader's inset and maximum prose width during loading and fallback rendering.
fn reader_column(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    let minimum = if width > 700. { 76. } else { 28. };
    let inset = ((width - 780.) / 2.).max(minimum);
    ui.add_space(38.);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.;
        ui.add_space(inset);
        ui.allocate_ui_with_layout(
            egui::vec2((width - inset * 2.).max(80.), 0.),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_max_width(ui.available_width().min(780.));
                add(ui);
            },
        );
    });
    ui.add_space(38.);
}

fn article_heading(ui: &mut Ui, a: &Article, font: ReaderFont) {
    let colors = crate::theme::palette(ui.ctx());
    ui.label(RichText::new(&a.site).size(10.).color(colors.muted));
    ui.add_space(18.);
    ui.label(
        RichText::new(&a.title)
            .font(egui::FontId::new(
                40.,
                egui::FontFamily::Name(font.family().into()),
            ))
            .color(colors.ink),
    );
    ui.add_space(18.);
    ui.label(
        RichText::new(format!(
            "{}{} · {}",
            a.author
                .as_ref()
                .map(|s| format!("{s} · "))
                .unwrap_or_default(),
            a.reading_label(),
            a.published_at
                .as_ref()
                .map(|s| format!("Published {}", date_label(s)))
                .unwrap_or_else(|| format!("Saved {}", date_label(&a.saved_at)))
        ))
        .size(11.)
        .color(colors.muted),
    );
}
