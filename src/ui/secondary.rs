use super::*;
use crate::{
    backend::Command,
    domain::{Feed, FeedAction, date_label},
    ui::controls::{self, ButtonKind},
};
use egui::{RichText, Ui};

pub fn feeds(ui: &mut Ui, feeds: &[Feed], actions: &mut Vec<Action>) {
    let colors = crate::theme::palette(ui.ctx());
    if ui.available_width() >= 560. {
        ui.horizontal_top(|ui| {
            let heading_width =
                (ui.available_width() - 150. - ui.spacing().item_spacing.x).max(180.);
            ui.allocate_ui_with_layout(
                egui::vec2(heading_width, 100.),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_width(heading_width);
                    header(
                        ui,
                        "Your feeds",
                        "New stories from the publications you follow.",
                    )
                },
            );
            // A right-to-left child with the entire remaining viewport also
            // expands this horizontal row vertically. Bound the action to its
            // own control height instead of centering it in the page.
            ui.allocate_ui_with_layout(
                egui::vec2(150., 34.),
                egui::Layout::right_to_left(egui::Align::Min),
                |ui| {
                    ui.set_min_width(150.);
                    if controls::action(ui, "Add feed", ButtonKind::Primary).clicked() {
                        actions.push(Action::FeedDialog);
                    }
                },
            );
        });
    } else {
        header(
            ui,
            "Your feeds",
            "New stories from the publications you follow.",
        );
        if controls::action(ui, "Add feed", ButtonKind::Primary).clicked() {
            actions.push(Action::FeedDialog);
        }
        ui.add_space(16.);
    }
    ui.label(
        RichText::new(format!(
            "{} {} · Stories stay in your library when you pause or unsubscribe.",
            feeds.len(),
            if feeds.len() == 1 { "feed" } else { "feeds" }
        ))
        .size(12.)
        .color(colors.muted),
    );
    ui.add_space(12.);
    ui.separator();
    egui::ScrollArea::vertical().id_salt("feeds").show(ui, |ui| {
        if feeds.is_empty() {
            controls::empty(
                ui,
                icons::Icon::Rss,
                "Follow your curiosity",
                "Add an RSS or Atom feed to bring new stories into your Inbox. You can pause a publication at any time.",
            );
            return;
        }
        for feed in feeds {
            ui.push_id(&feed.id.0, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(0, 4))
                    .show(ui, |ui| {
                        let wide = ui.available_width() >= 620.;
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 56.), egui::Sense::hover());
                        let mark = egui::Rect::from_min_size(rect.left_top() + egui::vec2(0., 10.), egui::vec2(36., 36.));
                        ui.painter().rect_filled(mark, 8., colors.soft);
                        icons::draw(ui, mark.shrink(9.), icons::Icon::Rss, if feed.active { colors.green } else { colors.muted });
                        let text_x = rect.left() + 50.;
                        let text_width = (rect.width() - 50. - if wide { 240. } else { 0. }).max(60.);
                        let mut title = egui::text::LayoutJob::simple(feed.title.clone(), egui::FontId::proportional(15.), if feed.active { colors.ink } else { colors.muted }, text_width);
                        title.wrap.max_rows = 1;
                        let title = ui.fonts_mut(|f| f.layout_job(title));
                        ui.painter().galley(egui::pos2(text_x, rect.top() + 9.), title, colors.ink);
                        let state = match (feed.active, feed.activity) {
                            (_, FeedActivity::Running) => "Checking now · ",
                            (_, FeedActivity::Queued) => "Queued · ",
                            (false, FeedActivity::Idle) => "Paused · ",
                            (true, FeedActivity::Idle) => "",
                        };
                        let checked = feed.last_polled_at.as_ref().map(|value| format!("Checked {}", date_label(value))).unwrap_or_else(|| "Not checked yet".into());
                        let address = feed.url.trim_start_matches("https://").trim_start_matches("http://");
                        let mut meta = egui::text::LayoutJob::simple(format!("{state}{address} · {checked}"), egui::FontId::proportional(11.), colors.muted, text_width);
                        meta.wrap.max_rows = 1;
                        meta.wrap.break_anywhere = true;
                        let meta = ui.fonts_mut(|f| f.layout_job(meta));
                        ui.painter().galley(egui::pos2(text_x, rect.top() + 31.), meta, colors.muted);
                        if wide {
                            let actions_rect = egui::Rect::from_min_max(egui::pos2(rect.right() - 240., rect.top() + 10.), egui::pos2(rect.right(), rect.top() + 46.));
                            ui.scope_builder(
                                egui::UiBuilder::new().max_rect(actions_rect).layout(egui::Layout::right_to_left(egui::Align::Center)),
                                |ui| feed_actions(ui, feed, actions),
                            );
                        } else {
                            feed_actions(ui, feed, actions);
                        }
                        if let Some(error) = &feed.error {
                            ui.add_space(12.);
                            egui::Frame::new()
                                .fill(colors.danger_fill)
                                .corner_radius(8)
                                .inner_margin(12)
                                .show(ui, |ui| {
                                    ui.label(RichText::new("Couldn't check this feed").color(colors.danger).strong());
                                    egui::CollapsingHeader::new("Details").show(ui, |ui| { ui.label(RichText::new(error).size(12.).color(colors.ink)); });
                                    if ui.add_enabled_ui(feed.activity == FeedActivity::Idle, |ui| controls::action(ui, "Try again", ButtonKind::Quiet)).inner.clicked() {
                                        actions.push(Action::Command(Command::Feed { id: feed.id.clone(), action: FeedAction::Sync }));
                                    }
                                });
                        }
                    });
                ui.separator();
            });
        }
    });
}

fn feed_actions(ui: &mut Ui, feed: &Feed, actions: &mut Vec<Action>) {
    ui.horizontal_wrapped(|ui| {
        if controls::action(
            ui,
            if feed.active { "Pause" } else { "Resume" },
            ButtonKind::Quiet,
        )
        .clicked()
        {
            actions.push(Action::Command(Command::Feed {
                id: feed.id.clone(),
                action: if feed.active {
                    FeedAction::Pause
                } else {
                    FeedAction::Resume
                },
            }));
        }
        if ui
            .add_enabled_ui(feed.activity == FeedActivity::Idle, |ui| {
                controls::icon_button(ui, icons::Icon::Refresh, "Sync feed", false)
            })
            .inner
            .clicked()
        {
            actions.push(Action::Command(Command::Feed {
                id: feed.id.clone(),
                action: FeedAction::Sync,
            }));
        }
        ui.menu_button("More", |ui| {
            if controls::action(ui, "Unsubscribe", ButtonKind::Quiet).clicked() {
                actions.push(Action::Command(Command::Feed {
                    id: feed.id.clone(),
                    action: FeedAction::Remove,
                }));
                ui.close();
            }
        });
    });
}

pub fn header(ui: &mut Ui, title: &str, subtitle: &str) {
    let colors = crate::theme::palette(ui.ctx());
    ui.label(
        RichText::new(title)
            .font(egui::FontId::new(
                36.,
                egui::FontFamily::Name("Georgia".into()),
            ))
            .color(colors.ink),
    );
    ui.add_space(8.);
    ui.label(RichText::new(subtitle).size(14.).color(colors.muted));
    ui.add_space(24.);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_header_and_first_row_stay_visible_in_short_and_desktop_windows() {
        for size in [egui::vec2(800., 600.), egui::vec2(1100., 800.)] {
            let context = egui::Context::default();
            context.enable_accesskit();
            let mut fonts = egui::FontDefinitions::default();
            let proportional = fonts.families[&egui::FontFamily::Proportional].clone();
            fonts
                .families
                .insert(egui::FontFamily::Name("Georgia".into()), proportional);
            context.set_fonts(fonts);
            let feed = Feed {
                id: FeedId("visible-feed".into()),
                url: "https://example.org/feed.xml".into(),
                title: "The first publication".into(),
                active: true,
                last_polled_at: None,
                error: None,
                activity: FeedActivity::Idle,
            };
            let mut viewport = egui::Rect::NOTHING;
            // Warm a second frame so the accessibility and font geometry are
            // stable, just as in a running gallery window.
            let mut output = None;
            for _ in 0..2 {
                let mut frame = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |root| {
                        egui::Panel::top("gallery-controls")
                            .exact_size(56.)
                            .show(root, |ui| {
                                ui.label("Synthetic gallery controls");
                            });
                        egui::Panel::bottom("gallery-note")
                            .exact_size(40.)
                            .show(root, |ui| {
                                ui.label("Synthetic gallery note");
                            });
                        egui::CentralPanel::default()
                            .frame(egui::Frame::new().inner_margin(28))
                            .show(root, |ui| {
                                viewport = ui.max_rect();
                                feeds(ui, std::slice::from_ref(&feed), &mut Vec::new());
                            });
                    },
                );
                frame.textures_delta.clear();
                output = Some(frame);
            }
            let output = output.unwrap();
            let text = |label: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::Shape::Text(text) if text.galley.job.text == label => {
                            let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                            assert!(
                                viewport.contains_rect(rect),
                                "{label} is outside {size:?}: {rect:?}"
                            );
                            assert!(
                                clipped.clip_rect.contains_rect(rect),
                                "{label} is clipped in {size:?}"
                            );
                            Some(rect)
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("Missing visible text {label:?} in {size:?}"))
            };
            let heading = text("Your feeds");
            let add = text("Add feed");
            assert!(
                viewport.right() - add.right() < 30.,
                "Add feed must align with the right page margin"
            );
            let publication = text("The first publication");
            text("Pause");
            text("More");
            assert!(
                (add.top() - heading.top()).abs() < 20.,
                "Add feed drifted away from the heading in {size:?}"
            );
            assert!(
                publication.top() < 300.,
                "First feed row starts too far down in {size:?}: {publication:?}"
            );
            let tree = output.platform_output.accesskit_update.as_ref().unwrap();
            let sync = tree
                .nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.label() == Some("Sync feed"))
                .expect("feed sync action");
            let bounds = sync.bounds().unwrap();
            let rect = egui::Rect::from_min_max(
                egui::pos2(bounds.x0 as f32, bounds.y0 as f32),
                egui::pos2(bounds.x1 as f32, bounds.y1 as f32),
            );
            assert!(
                viewport.contains_rect(rect),
                "Sync feed is outside the visible first row in {size:?}"
            );
        }
    }
}
