use super::*;
use crate::{
    backend::Command,
    domain::{Feed, FeedAction, date_label},
    ui::controls::{self, ButtonKind},
};
use egui::{Align2, FontFamily, FontId, RichText, Stroke, Ui, pos2, text::TextFormat, vec2};

const ROW_HEIGHT: f32 = 68.;
const ROW_INSET: f32 = 18.;
const TILE: f32 = 40.;
const ACTIONS_WIDTH: f32 = 196.;
const CARD_RADIUS: u8 = 12;

/// Feeds are grouped by what they need from the reader, not by fetch order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Attention,
    Following,
    Paused,
}
impl Section {
    const ALL: [Self; 3] = [Self::Attention, Self::Following, Self::Paused];
    fn of(feed: &Feed) -> Self {
        if feed.error.is_some() {
            Self::Attention
        } else if feed.active {
            Self::Following
        } else {
            Self::Paused
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Attention => "NEEDS ATTENTION",
            Self::Following => "FOLLOWING",
            Self::Paused => "PAUSED",
        }
    }
}

pub fn feeds(ui: &mut Ui, feeds: &[Feed], actions: &mut Vec<Action>) {
    let colors = crate::theme::palette(ui.ctx());
    let now = chrono::Utc::now();
    feeds_header(ui, actions);
    let filter_id = ui.id().with("feed-filter");
    let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
    overview(ui, feeds, &mut filter);
    ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
    ui.add_space(14.);
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
        let needle = filter.trim().to_lowercase();
        let visible: Vec<&Feed> = feeds
            .iter()
            .filter(|feed| {
                needle.is_empty()
                    || feed.title.to_lowercase().contains(&needle)
                    || feed.url.to_lowercase().contains(&needle)
            })
            .collect();
        if visible.is_empty() {
            ui.add_space(24.);
            ui.label(
                RichText::new(format!("No feeds match “{}”.", filter.trim()))
                    .size(13.)
                    .color(colors.muted),
            );
            return;
        }
        for section in Section::ALL {
            let members: Vec<&Feed> = visible
                .iter()
                .copied()
                .filter(|feed| Section::of(feed) == section)
                .collect();
            if members.is_empty() {
                continue;
            }
            ui.label(eyebrow(
                &format!("{}  ·  {}", section.label(), members.len()),
                if section == Section::Attention {
                    colors.danger
                } else {
                    colors.muted
                },
            ));
            ui.add_space(2.);
            egui::Frame::new()
                .fill(colors.reader)
                .stroke(Stroke::new(1., colors.divider))
                .corner_radius(CARD_RADIUS)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 0.;
                    let count = members.len();
                    for (index, feed) in members.into_iter().enumerate() {
                        ui.push_id(&feed.id.0, |ui| {
                            feed_row(ui, feed, now, (index == 0, index + 1 == count), actions)
                        });
                    }
                });
            ui.add_space(22.);
        }
        ui.horizontal_top(|ui| {
            let (mark, _) = ui.allocate_exact_size(vec2(14., 14.), egui::Sense::hover());
            icons::draw(ui, mark, icons::Icon::Rss, colors.muted);
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Once a story arrives, it stays in your library. Pausing, feed cutoffs and unsubscribing never remove it.",
                    )
                    .size(12.)
                    .color(colors.muted),
                )
                .wrap(),
            );
        });
        ui.add_space(8.);
    });
}

/// Small spaced capitals used for section labels and the page eyebrow.
fn eyebrow(text: &str, color: egui::Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.,
        TextFormat {
            font_id: FontId::proportional(10.5),
            color,
            extra_letter_spacing: 1.4,
            ..Default::default()
        },
    );
    job
}

fn feeds_header(ui: &mut Ui, actions: &mut Vec<Action>) {
    let colors = crate::theme::palette(ui.ctx());
    let button_width = 118.;
    ui.horizontal_top(|ui| {
        let heading_width =
            (ui.available_width() - button_width - ui.spacing().item_spacing.x).max(180.);
        ui.allocate_ui_with_layout(
            vec2(heading_width, 100.),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_width(heading_width);
                ui.spacing_mut().item_spacing.y = 4.;
                ui.label(eyebrow("FOLLOW YOUR CURIOSITY", colors.accent));
                let mut title = egui::text::LayoutJob::default();
                let font = FontId::new(40., FontFamily::Name("Georgia".into()));
                title.append(
                    "Your feeds",
                    0.,
                    TextFormat::simple(font.clone(), colors.ink),
                );
                title.append(".", 0., TextFormat::simple(font, colors.accent));
                ui.label(title);
                ui.label(
                    RichText::new(
                        "New writing from the publications you follow, delivered to your Inbox.",
                    )
                    .size(14.)
                    .color(colors.muted),
                );
            },
        );
        // Bound the action to its own height so the row does not stretch.
        ui.allocate_ui_with_layout(
            vec2(button_width, 60.),
            egui::Layout::top_down(egui::Align::Max),
            |ui| {
                ui.set_min_width(button_width);
                ui.add_space(22.);
                if add_button(ui).clicked() {
                    actions.push(Action::FeedDialog);
                }
            },
        );
    });
    ui.add_space(18.);
}

fn add_button(ui: &mut Ui) -> egui::Response {
    let colors = crate::theme::palette(ui.ctx());
    let font = FontId::proportional(12.5);
    let label = ui.fonts_mut(|f| f.layout_no_wrap("Add feed".into(), font, colors.on_accent));
    let size = vec2(label.size().x + 50., 36.);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    ui.painter().rect_filled(rect, 9., colors.green);
    controls::interaction_fill(ui, &response, false);
    icons::draw(
        ui,
        egui::Rect::from_center_size(pos2(rect.left() + 22., rect.center().y), vec2(14., 14.)),
        icons::Icon::Plus,
        colors.on_accent,
    );
    ui.painter().galley(
        pos2(rect.left() + 36., rect.center().y - label.size().y / 2.),
        label,
        colors.on_accent,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Add feed"));
    controls::focus(ui, &response);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A quiet summary of the collection, with a filter once the list gets long.
fn overview(ui: &mut Ui, feeds: &[Feed], filter: &mut String) {
    let colors = crate::theme::palette(ui.ctx());
    if feeds.is_empty() {
        return;
    }
    let count = |section| feeds.iter().filter(|f| Section::of(f) == section).count();
    let checking = feeds
        .iter()
        .filter(|f| f.activity != FeedActivity::Idle)
        .count();
    let stats = [
        (count(Section::Following), "following", colors.green),
        (count(Section::Paused), "paused", colors.muted),
        (
            count(Section::Attention),
            if count(Section::Attention) == 1 {
                "needs attention"
            } else {
                "need attention"
            },
            colors.danger,
        ),
        (checking, "checking now", colors.accent),
    ];
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        vec2(width, 34.),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(width);
            ui.spacing_mut().item_spacing.x = 18.;
            for (value, label, color) in stats {
                if value == 0 && label != "following" {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 7.;
                    let (dot, _) = ui.allocate_exact_size(vec2(7., 7.), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.5, color);
                    let mut job = egui::text::LayoutJob::default();
                    job.append(
                        &value.to_string(),
                        0.,
                        TextFormat::simple(
                            FontId::new(12.5, FontFamily::Name("System Semibold".into())),
                            colors.ink,
                        ),
                    );
                    job.append(
                        &format!(" {label}"),
                        0.,
                        TextFormat::simple(FontId::proportional(12.5), colors.muted),
                    );
                    ui.label(job);
                });
            }
            if feeds.len() >= 6 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let field_width = ui.available_width().min(220.);
                    let response = ui.add(
                        egui::TextEdit::singleline(filter)
                            .hint_text("Filter feeds")
                            .desired_width(field_width - 40.)
                            .margin(egui::Margin {
                                left: 30,
                                right: 10,
                                top: 7,
                                bottom: 7,
                            }),
                    );
                    icons::draw(
                        ui,
                        egui::Rect::from_center_size(
                            pos2(response.rect.left() + 16., response.rect.center().y),
                            vec2(13., 13.),
                        ),
                        icons::Icon::Search,
                        colors.muted,
                    );
                });
            }
        },
    );
}

fn feed_row(
    ui: &mut Ui,
    feed: &Feed,
    now: chrono::DateTime<chrono::Utc>,
    (first, last): (bool, bool),
    actions: &mut Vec<Action>,
) {
    let colors = crate::theme::palette(ui.ctx());
    let wide = ui.available_width() >= 600.;
    let background = ui.painter().add(egui::Shape::Noop);
    let top = ui.cursor().top();
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), egui::Sense::hover());
    let painter = ui.painter().clone();
    let paused = !feed.active;
    let tile = egui::Rect::from_min_size(
        pos2(rect.left() + ROW_INSET, rect.center().y - TILE / 2.),
        vec2(TILE, TILE),
    );
    painter.rect_filled(
        tile,
        10.,
        if paused { colors.soft } else { colors.selected },
    );
    painter.text(
        tile.center() + vec2(0., 1.),
        Align2::CENTER_CENTER,
        initial(&feed.title),
        FontId::new(20., FontFamily::Name("Georgia".into())),
        if paused { colors.muted } else { colors.green },
    );
    let dot = if feed.error.is_some() {
        colors.danger
    } else if feed.activity != FeedActivity::Idle {
        colors.accent
    } else if paused {
        colors.muted
    } else {
        colors.green
    };
    let dot_center = tile.right_bottom() - vec2(3., 3.);
    painter.circle_filled(dot_center, 6., colors.reader);
    painter.circle_filled(dot_center, 4., dot);

    let text_x = tile.right() + 14.;
    let text_width =
        (rect.right() - ROW_INSET - if wide { ACTIONS_WIDTH } else { 0. } - text_x).max(60.);
    let mut title = egui::text::LayoutJob::simple(
        feed.title.clone(),
        FontId::new(14.5, FontFamily::Name("System Semibold".into())),
        if paused { colors.muted } else { colors.ink },
        text_width,
    );
    title.wrap.max_rows = 1;
    let title = ui.fonts_mut(|f| f.layout_job(title));
    painter.galley(pos2(text_x, rect.center().y - 18.), title, colors.ink);
    let address = feed
        .url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    let state = match (feed.active, feed.activity) {
        (_, FeedActivity::Running) => "Checking now…".to_owned(),
        (_, FeedActivity::Queued) => "Queued to check".to_owned(),
        (false, FeedActivity::Idle) => "Paused".to_owned(),
        (true, FeedActivity::Idle) => feed
            .last_polled_at
            .as_deref()
            .map(|value| checked_label(value, now))
            .unwrap_or_else(|| "Not checked yet".into()),
    };
    let mut meta = egui::text::LayoutJob::default();
    meta.wrap.max_width = text_width;
    meta.wrap.max_rows = 1;
    meta.wrap.break_anywhere = true;
    let small = FontId::proportional(11.5);
    meta.append(
        &state,
        0.,
        TextFormat::simple(
            small.clone(),
            if feed.activity != FeedActivity::Idle {
                colors.accent
            } else {
                colors.muted
            },
        ),
    );
    meta.append(
        "  ·  ",
        0.,
        TextFormat::simple(small.clone(), colors.divider),
    );
    meta.append(address, 0., TextFormat::simple(small, colors.muted));
    let meta = ui.fonts_mut(|f| f.layout_job(meta));
    painter.galley(pos2(text_x, rect.center().y + 4.), meta, colors.muted);

    if wide {
        let actions_rect = egui::Rect::from_min_max(
            pos2(rect.right() - ROW_INSET - ACTIONS_WIDTH, rect.top()),
            pos2(rect.right() - ROW_INSET, rect.bottom()),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(actions_rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| feed_actions(ui, feed, actions),
        );
    } else {
        ui.horizontal(|ui| {
            ui.add_space(text_x - rect.left());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(ROW_INSET);
                feed_actions(ui, feed, actions);
            });
        });
        ui.add_space(10.);
    }
    if let Some(error) = &feed.error {
        feed_error(ui, feed, error, text_x - rect.left(), actions);
    }
    let full = egui::Rect::from_min_max(
        pos2(rect.left(), top),
        pos2(rect.right(), ui.cursor().top()),
    );
    if ui.rect_contains_pointer(full) {
        let radius = |round: bool| if round { CARD_RADIUS } else { 0 };
        painter.set(
            background,
            egui::Shape::rect_filled(
                full.shrink(1.),
                egui::CornerRadius {
                    nw: radius(first),
                    ne: radius(first),
                    sw: radius(last),
                    se: radius(last),
                },
                colors.hover,
            ),
        );
    }
    if !last {
        painter.line_segment(
            [
                pos2(text_x, full.bottom()),
                pos2(rect.right() - ROW_INSET, full.bottom()),
            ],
            Stroke::new(1., colors.divider),
        );
    }
}

fn feed_error(ui: &mut Ui, feed: &Feed, error: &str, indent: f32, actions: &mut Vec<Action>) {
    let colors = crate::theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        ui.add_space(indent);
        let width = ui.available_width() - ROW_INSET;
        ui.vertical(|ui| {
            ui.set_width(width);
            egui::Frame::new()
                .fill(colors.danger_fill)
                .stroke(Stroke::new(1., colors.danger_border))
                .corner_radius(9)
                .inner_margin(egui::Margin {
                    left: 12,
                    right: 6,
                    top: 6,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 2.;
                    ui.horizontal(|ui| {
                        let (mark, _) =
                            ui.allocate_exact_size(vec2(14., 14.), egui::Sense::hover());
                        icons::draw(ui, mark, icons::Icon::Warning, colors.danger);
                        ui.label(
                            RichText::new("Couldn't check this feed")
                                .size(12.5)
                                .color(colors.danger),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled_ui(feed.activity == FeedActivity::Idle, |ui| {
                                    controls::action(ui, "Try again", ButtonKind::Quiet)
                                })
                                .inner
                                .clicked()
                            {
                                actions.push(Action::Command(Command::Feed {
                                    id: feed.id.clone(),
                                    action: FeedAction::Sync,
                                }));
                            }
                        });
                    });
                    let summary = error.lines().next().unwrap_or(error);
                    ui.add(
                        egui::Label::new(RichText::new(summary).size(11.5).color(colors.ink))
                            .truncate(),
                    );
                    if summary.len() < error.len() {
                        egui::CollapsingHeader::new(RichText::new("Details").size(11.5)).show(
                            ui,
                            |ui| {
                                ui.label(RichText::new(error).size(11.5).color(colors.ink));
                            },
                        );
                    }
                });
        });
    });
    ui.add_space(14.);
}

fn initial(title: &str) -> String {
    title
        .chars()
        .find(|c| c.is_alphanumeric())
        .unwrap_or('·')
        .to_uppercase()
        .to_string()
}

/// Recent checks read as relative times; older ones fall back to the date.
fn checked_label(value: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(value) else {
        return format!("Checked {}", date_label(value));
    };
    let seconds = (now - at.with_timezone(&chrono::Utc)).num_seconds().max(0);
    let plural = |n: i64, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    match seconds {
        0..60 => "Checked just now".into(),
        60..3_600 => format!("Checked {} ago", plural(seconds / 60, "minute")),
        3_600..86_400 => format!("Checked {} ago", plural(seconds / 3_600, "hour")),
        86_400..172_800 => "Checked yesterday".into(),
        172_800..604_800 => format!("Checked {} days ago", seconds / 86_400),
        _ => format!("Checked {}", date_label(value)),
    }
}

fn feed_actions(ui: &mut Ui, feed: &Feed, actions: &mut Vec<Action>) {
    let colors = crate::theme::palette(ui.ctx());
    ui.spacing_mut().item_spacing.x = 4.;
    let more = controls::icon_button(ui, icons::Icon::More, "More actions", false);
    egui::Popup::menu(&more)
        .align(egui::emath::RectAlign::BOTTOM_END)
        .align_alternatives(&[])
        .show(|ui| {
            if controls::action(ui, "Copy feed address", ButtonKind::Quiet).clicked() {
                ui.ctx().copy_text(feed.url.clone());
                ui.close();
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("Unsubscribe").size(12.).color(colors.danger))
                        .frame(false)
                        .min_size(vec2(0., controls::ACTION_HEIGHT)),
                )
                .clicked()
            {
                actions.push(Action::Command(Command::Feed {
                    id: feed.id.clone(),
                    action: FeedAction::Remove,
                }));
                ui.close();
            }
        });
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
    let (label, kind) = if feed.active {
        ("Pause", ButtonKind::Quiet)
    } else {
        ("Resume", ButtonKind::Secondary)
    };
    if controls::action(ui, label, kind).clicked() {
        actions.push(Action::Command(Command::Feed {
            id: feed.id.clone(),
            action: if feed.active {
                FeedAction::Pause
            } else {
                FeedAction::Resume
            },
        }));
    }
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
            fonts.families.insert(
                egui::FontFamily::Name("Georgia".into()),
                proportional.clone(),
            );
            fonts.families.insert(
                egui::FontFamily::Name("System Semibold".into()),
                proportional,
            );
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
            let heading = text("Your feeds.");
            let add = text("Add feed");
            assert!(
                viewport.right() - add.right() < 30.,
                "Add feed must align with the right page margin"
            );
            let publication = text("The first publication");
            text("Pause");
            assert!(
                (add.top() - heading.top()).abs() < 40.,
                "Add feed drifted away from the heading in {size:?}"
            );
            assert!(
                publication.top() < 320.,
                "First feed row starts too far down in {size:?}: {publication:?}"
            );
            let tree = output.platform_output.accesskit_update.as_ref().unwrap();
            for label in ["Add feed", "More actions"] {
                assert!(
                    tree.nodes
                        .iter()
                        .any(|(_, node)| node.label() == Some(label)),
                    "Missing accessible {label:?}"
                );
            }
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

#[cfg(test)]
mod label_tests {
    use super::checked_label;

    #[test]
    fn recent_checks_read_as_relative_times() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-30T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        for (at, label) in [
            ("2026-10-30T11:59:30Z", "Checked just now"),
            ("2026-10-30T11:59:00Z", "Checked 1 minute ago"),
            ("2026-10-30T09:00:00.000Z", "Checked 3 hours ago"),
            ("2026-10-29T10:00:00Z", "Checked yesterday"),
            ("2026-10-26T12:00:00Z", "Checked 4 days ago"),
            ("2026-10-01T12:00:00Z", "Checked Oct 1, 2026"),
            ("2026-10-31T12:00:00Z", "Checked just now"),
        ] {
            assert_eq!(checked_label(at, now), label);
        }
    }
}
