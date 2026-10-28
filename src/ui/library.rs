use super::controls::{self, ButtonKind};
use super::icons::Icon;
use super::*;
use crate::{
    backend::Command,
    domain::{Article, Origin, Query, ReadFilter, Sort, date_label},
};
use egui::{Align2, FontId, RichText, Sense, Ui, vec2};
pub struct State<'a> {
    pub view: View,
    pub query: &'a Query,
    pub total: u32,
    pub library_total: u32,
    pub grouped: bool,
    pub logos: &'a std::collections::HashMap<String, egui::TextureHandle>,
    pub rows: &'a [Article],
    pub publications: &'a [(String, u32)],
    pub selected: Option<&'a ArticleId>,
    pub selection: &'a [ArticleId],
    pub selecting: bool,
    pub selecting_delete: bool,
    pub loading: bool,
    pub has_more: bool,
    pub scroll: f32,
}
pub fn draw(ui: &mut Ui, state: State<'_>, actions: &mut Vec<Action>) -> f32 {
    let colors = crate::theme::palette(ui.ctx());
    let mut q = state.query.clone();
    let before = q.clone();
    egui::Frame::new()
        .fill(colors.article)
        .inner_margin(egui::Margin {
            left: 16,
            right: 16,
            top: TITLEBAR_INSET as i8 + 6,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(state.view.label())
                        .font(FontId::new(24., egui::FontFamily::Name("Georgia".into())))
                        .color(colors.ink),
                );
                ui.label(
                    RichText::new(state.total.to_string())
                        .size(12.)
                        .color(colors.muted),
                )
                .on_hover_text(format!(
                    "{} {}",
                    state.total,
                    if state.total == 1 {
                        "article"
                    } else {
                        "articles"
                    }
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let more = controls::icon_button(ui, Icon::More, "Library actions", false);
                    egui::Popup::menu(&more)
                        .align(egui::emath::RectAlign::BOTTOM_END)
                        .align_alternatives(&[])
                        .show(|ui| {
                            if controls::action(
                                ui,
                                "Choose articles for a magazine",
                                ButtonKind::Quiet,
                            )
                            .clicked()
                            {
                                actions.push(Action::SelectMagazine);
                                ui.close();
                            }
                            if controls::action(ui, "Select articles to delete", ButtonKind::Quiet)
                                .clicked()
                            {
                                actions.push(Action::SelectDelete);
                                ui.close();
                            }
                            if controls::action(ui, "Re-fetch all articles", ButtonKind::Quiet)
                                .clicked()
                            {
                                actions.push(Action::Command(Command::RefetchAll));
                                ui.close();
                            }
                            if state.grouped
                                && !q.publication.is_empty()
                                && controls::action(
                                    ui,
                                    "Delete matching articles",
                                    ButtonKind::Danger,
                                )
                                .clicked()
                            {
                                actions.push(Action::ConfirmDeleteMatching {
                                    query: q.clone(),
                                    expected: state.total,
                                });
                                ui.close();
                            }
                        });
                    let unread = q.read == ReadFilter::Unread;
                    if let Some(choice) =
                        controls::segmented(ui, &["Unread", "Read"], usize::from(!unread))
                    {
                        q.read = if choice == 0 {
                            ReadFilter::Unread
                        } else {
                            ReadFilter::Read
                        };
                    }
                });
            });
            ui.add_space(10.);
            let search = ui
                .horizontal(|ui| {
                    let search_width = (ui.available_width() - 38.).max(80.);
                    let search = ui
                        .allocate_ui(vec2(search_width, 34.), |ui| {
                            egui::Frame::new()
                                .fill(colors.reader)
                                .stroke(egui::Stroke::new(1., colors.divider))
                                .corner_radius(8)
                                .inner_margin(egui::Margin::symmetric(10, 3))
                                .show(ui, |ui| {
                                    ui.set_width(search_width - 22.);
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 7.;
                                        let (icon, _) =
                                            ui.allocate_exact_size(vec2(16., 24.), Sense::hover());
                                        icons::draw(
                                            ui,
                                            egui::Rect::from_center_size(
                                                icon.center(),
                                                vec2(15., 15.),
                                            ),
                                            Icon::Search,
                                            colors.muted,
                                        );
                                        let clear_width =
                                            if q.search.is_empty() { 0. } else { 37. };
                                        let edit = ui.add_sized(
                                            [(ui.available_width() - clear_width).max(20.), 24.],
                                            egui::TextEdit::singleline(&mut q.search)
                                                .id(egui::Id::new("library-search"))
                                                .hint_text("Search")
                                                .frame(egui::Frame::NONE)
                                                .margin(vec2(0., 5.)),
                                        );
                                        if !q.search.is_empty()
                                            && controls::icon_button(
                                                ui,
                                                Icon::Close,
                                                "Clear search",
                                                false,
                                            )
                                            .clicked()
                                        {
                                            q.search.clear();
                                            edit.request_focus();
                                        }
                                        edit
                                    })
                                    .inner
                                })
                        })
                        .inner;
                    let customized = q.origin != Origin::All || state.grouped;
                    let filter =
                        controls::icon_button(ui, Icon::Filter, "Filter and sort", customized);
                    egui::Popup::menu(&filter)
                        .align(egui::emath::RectAlign::BOTTOM_END)
                        .align_alternatives(&[])
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show(|ui| {
                            ui.set_min_width(210.);
                            ui.spacing_mut().interact_size.y = 22.;
                            ui.spacing_mut().item_spacing.y = 2.;
                            ui.spacing_mut().button_padding = vec2(8., 4.);
                            let heading = |ui: &mut Ui, text: &str| {
                                ui.label(RichText::new(text).size(10.).color(colors.muted));
                            };
                            heading(ui, "SHOW");
                            for (origin, label) in [
                                (Origin::All, "All articles"),
                                (Origin::Saved, "Deliberately saved"),
                                (Origin::Rss, "Discovered by RSS"),
                            ] {
                                if ui.selectable_label(q.origin == origin, label).clicked() {
                                    q.origin = origin;
                                }
                            }
                            ui.separator();
                            heading(ui, "SORT BY");
                            for sort in Sort::ALL {
                                let label = if q.sort == sort {
                                    format!(
                                        "{}  {}",
                                        sort.label(),
                                        if q.ascending { "↑" } else { "↓" }
                                    )
                                } else {
                                    sort.label().to_owned()
                                };
                                if ui
                                    .selectable_label(q.sort == sort, label)
                                    .on_hover_text("Choose the current sort again to reverse it")
                                    .clicked()
                                {
                                    q.sort_by(sort);
                                }
                            }
                            ui.separator();
                            if ui
                                .selectable_label(state.grouped, "Group by publication")
                                .clicked()
                            {
                                actions.push(Action::Group(!state.grouped));
                            }
                        });
                    search
                })
                .inner;
            if search.inner.has_focus() {
                ui.painter().rect_stroke(
                    search.response.rect,
                    8.,
                    egui::Stroke::new(1.5, colors.green),
                    egui::StrokeKind::Outside,
                );
            }
            if state.grouped && !q.publication.is_empty() {
                ui.horizontal(|ui| {
                    if controls::icon_button(ui, Icon::ArrowLeft, "Back to publications", false)
                        .clicked()
                    {
                        q.publication.clear();
                    }
                    ui.add(
                        egui::Label::new(RichText::new(&q.publication).size(12.).strong())
                            .truncate(),
                    );
                });
            }
            if state.selecting {
                ui.add_space(4.);
                egui::Frame::new()
                    .fill(colors.selected)
                    .corner_radius(8)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(if state.selecting_delete {
                                "Choose articles to delete"
                            } else {
                                "Choose articles for your magazine"
                            })
                            .size(12.)
                            .strong()
                            .color(colors.green),
                        );
                        if !state.selecting_delete {
                            ui.label(
                                RichText::new("Click articles in the order you want to read them.")
                                    .size(11.)
                                    .color(colors.green),
                            );
                        }
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{} selected", state.selection.len()))
                                    .size(11.)
                                    .color(colors.green),
                            );
                            if controls::action(ui, "Cancel", ButtonKind::Quiet).clicked() {
                                actions.push(Action::CancelSelection);
                            }
                        });
                        ui.add_enabled_ui(!state.selection.is_empty(), |ui| {
                            if controls::action(
                                ui,
                                if state.selecting_delete {
                                    "Delete selected"
                                } else {
                                    "Create draft"
                                },
                                if state.selecting_delete {
                                    ButtonKind::Danger
                                } else {
                                    ButtonKind::Primary
                                },
                            )
                            .clicked()
                            {
                                actions.push(if state.selecting_delete {
                                    Action::ConfirmDelete(state.selection.to_vec())
                                } else {
                                    Action::DraftDialog
                                });
                            }
                        });
                    });
            }
        });
    if q != before {
        actions.push(Action::Query(q));
    }
    let scroll = egui::ScrollArea::vertical()
        .id_salt(format!("list-{:?}", state.view))
        .vertical_scroll_offset(state.scroll)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.;
            if state.grouped && state.query.publication.is_empty() && !state.publications.is_empty()
            {
                for (host, count) in state.publications {
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 72.), Sense::click());
                    if response.hovered() {
                        ui.painter().rect_filled(rect, 0., colors.selected);
                    }
                    let mark = egui::Rect::from_min_size(rect.min + vec2(16., 18.), vec2(32., 32.));
                    ui.painter().rect_filled(mark, 8., colors.selected);
                    icons::draw(ui, mark.shrink(7.), Icon::Book, colors.green);
                    let mut title = egui::text::LayoutJob::simple(
                        host.clone(),
                        FontId::proportional(13.),
                        colors.ink,
                        (rect.width() - 128.).max(20.),
                    );
                    title.wrap.max_rows = 1;
                    let title = ui.fonts_mut(|f| f.layout_job(title));
                    ui.painter()
                        .galley(rect.min + vec2(60., 17.), title, colors.ink);
                    ui.painter().text(
                        rect.min + vec2(60., 40.),
                        Align2::LEFT_TOP,
                        format!("{count} articles"),
                        FontId::proportional(11.),
                        colors.muted,
                    );
                    icons::draw(
                        ui,
                        egui::Rect::from_center_size(
                            egui::pos2(rect.right() - 55., rect.center().y),
                            vec2(15., 15.),
                        ),
                        Icon::ChevronRight,
                        colors.muted,
                    );
                    let menu_rect = egui::Rect::from_min_size(
                        egui::pos2(rect.right() - 44., rect.top() + 20.),
                        vec2(30., 30.),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(menu_rect), |ui| {
                        let more = controls::icon_button(
                            ui,
                            Icon::More,
                            &format!("Actions for {host}"),
                            false,
                        );
                        egui::Popup::menu(&more)
                            .align(egui::emath::RectAlign::BOTTOM_END)
                            .align_alternatives(&[])
                            .show(|ui| {
                                if controls::action(
                                    ui,
                                    "Delete matching articles",
                                    ButtonKind::Danger,
                                )
                                .clicked()
                                {
                                    let mut q = state.query.clone();
                                    q.publication = host.clone();
                                    actions.push(Action::ConfirmDeleteMatching {
                                        query: q,
                                        expected: *count,
                                    });
                                    ui.close();
                                }
                            });
                    });
                    if response.clicked()
                        && !ui
                            .ctx()
                            .pointer_interact_pos()
                            .is_some_and(|p| menu_rect.contains(p))
                    {
                        let mut q = state.query.clone();
                        q.publication = host.clone();
                        actions.push(Action::Query(q));
                    }
                    controls::focus(ui, &response);
                    ui.painter().line_segment(
                        [rect.left_bottom(), rect.right_bottom()],
                        egui::Stroke::new(1., colors.divider),
                    );
                }
                return;
            }
            for article in state.rows {
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 86.), Sense::click());
                let selected = state.selected == Some(&article.id);
                let checked = state.selection.contains(&article.id);
                response.widget_info(|| {
                    let label = format!(
                        "{}, {}",
                        article.title,
                        if article.read_at.is_some() {
                            "Read"
                        } else {
                            "Unread"
                        }
                    );
                    if state.selecting {
                        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, checked, label)
                    } else {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label)
                    }
                });
                let painter = ui.painter().with_clip_rect(rect);
                painter.rect_filled(
                    rect,
                    0.,
                    if selected || checked {
                        colors.selected
                    } else if response.hovered() {
                        colors.hover
                    } else {
                        colors.article
                    },
                );
                painter.line_segment(
                    [rect.left_bottom(), rect.right_bottom()],
                    egui::Stroke::new(1., colors.divider),
                );
                if selected {
                    painter.rect_filled(
                        egui::Rect::from_min_size(rect.min, vec2(3., 86.)),
                        0.,
                        colors.green,
                    );
                }
                let icon = egui::Rect::from_min_size(rect.min + vec2(16., 25.), vec2(36., 36.));
                painter.rect_filled(icon, 5., colors.selected);
                if !state.selecting
                    && article
                        .logo_url
                        .as_ref()
                        .and_then(|url| state.logos.get(url))
                        .is_some()
                {
                    let image = state
                        .logos
                        .get(article.logo_url.as_ref().expect("Known logo"))
                        .expect("Cached logo");
                    painter.image(
                        image.id(),
                        icon.shrink(2.),
                        egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.)),
                        egui::Color32::WHITE,
                    );
                } else if state.selecting {
                    icons::draw(
                        ui,
                        egui::Rect::from_center_size(
                            icon.center()
                                - vec2(
                                    0.,
                                    if checked && !state.selecting_delete {
                                        5.
                                    } else {
                                        0.
                                    },
                                ),
                            vec2(20., 20.),
                        ),
                        if checked {
                            Icon::CheckSquare
                        } else {
                            Icon::Square
                        },
                        colors.green,
                    );
                    if checked && !state.selecting_delete {
                        let order = state
                            .selection
                            .iter()
                            .position(|id| id == &article.id)
                            .expect("Selected article has an order")
                            + 1;
                        painter.text(
                            icon.center() + vec2(0., 10.),
                            Align2::CENTER_CENTER,
                            order.to_string(),
                            FontId::proportional(10.),
                            colors.green,
                        );
                    }
                } else {
                    painter.text(
                        icon.center(),
                        Align2::CENTER_CENTER,
                        article
                            .site
                            .chars()
                            .next()
                            .unwrap_or('A')
                            .to_uppercase()
                            .to_string(),
                        FontId::new(20., egui::FontFamily::Name("Georgia".into())),
                        colors.green,
                    );
                }
                // Fade only read Inbox text. Icons, selection and hit targets stay unchanged.
                let mut text_painter = painter.clone();
                if state.view == View::Inbox && article.read_at.is_some() {
                    text_painter.set_opacity(0.72);
                }
                let x = rect.left() + 62.;
                let width = rect.width() - 78.;
                let title = egui::text::LayoutJob::simple(
                    article.title.clone(),
                    FontId::new(14., egui::FontFamily::Name("System Semibold".into())),
                    colors.ink,
                    width,
                );
                let mut title = title;
                title.wrap.max_rows = 1;
                title.wrap.break_anywhere = true;
                let galley = ui.fonts_mut(|f| f.layout_job(title));
                text_painter.galley(egui::pos2(x, rect.top() + 9.), galley, colors.ink);
                let mut excerpt = egui::text::LayoutJob::simple(
                    article.excerpt.clone(),
                    FontId::proportional(12.),
                    colors.muted,
                    width,
                );
                excerpt.wrap.max_rows = 2;
                let galley = ui.fonts_mut(|f| f.layout_job(excerpt));
                text_painter.galley(egui::pos2(x, rect.top() + 28.), galley, colors.muted);
                let mut host = if article.site.trim().is_empty() {
                    url::Url::parse(&article.url)
                        .ok()
                        .and_then(|u| {
                            u.host_str()
                                .map(|h| h.trim_start_matches("www.").to_owned())
                        })
                        .unwrap_or_default()
                } else {
                    article.site.trim_start_matches("www.").to_owned()
                };
                if article.word_count > 0 {
                    host.push_str(&format!(" · {} min", article.minutes()));
                }
                // Copy quality is stated in words, not a warning glyph. Partial copies
                // are still readable, so the reader explains them when opened.
                match article.status {
                    CopyStatus::Failed => host.push_str(" · Unavailable"),
                    CopyStatus::Pending => host.push_str(" · Fetching"),
                    CopyStatus::Partial | CopyStatus::Saved => {}
                }
                let date = date_label(&article.saved_at);
                let date_galley = ui
                    .fonts_mut(|f| f.layout_no_wrap(date, FontId::proportional(10.), colors.muted));
                let host_width = (width - date_galley.size().x - 12.).max(0.);
                let mut host_job = egui::text::LayoutJob::simple(
                    host,
                    FontId::proportional(10.),
                    colors.muted,
                    host_width,
                );
                host_job.wrap.max_rows = 1;
                host_job.wrap.break_anywhere = true;
                let host_galley = ui.fonts_mut(|f| f.layout_job(host_job));
                text_painter.galley(
                    egui::pos2(x, rect.bottom() - 8. - host_galley.size().y),
                    host_galley,
                    colors.muted,
                );
                text_painter.galley(
                    egui::pos2(
                        rect.right() - 16. - date_galley.size().x,
                        rect.bottom() - 8. - date_galley.size().y,
                    ),
                    date_galley,
                    colors.muted,
                );
                if state.view != View::Inbox && article.read_at.is_none() {
                    painter.circle_filled(
                        egui::pos2(icon.center().x, rect.bottom() - 13.),
                        2.3,
                        colors.green,
                    );
                }
                controls::focus(ui, &response);
                if response.clicked() {
                    actions.push(if state.selecting {
                        Action::ToggleSelection(article.id.clone())
                    } else {
                        Action::Open(article.id.clone())
                    });
                }
            }
            if state.loading {
                ui.add_space(16.);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new("Loading articles…")
                            .size(12.)
                            .color(colors.muted),
                    );
                });
            } else if state.has_more {
                ui.add_space(8.);
                let r = controls::action(ui, "Load more articles", ButtonKind::Quiet);
                if r.clicked() || ui.is_rect_visible(r.rect) {
                    actions.push(Action::LoadMore);
                }
            } else if state.rows.is_empty() {
                let empty_library = state.library_total == 0;
                let filtered = !state.query.search.is_empty()
                    || !state.query.publication.is_empty()
                    || state.query.origin != Origin::All;
                let unread = state.query.read == ReadFilter::Unread;
                let (title, description) = if empty_library {
                    (
                        "Your reading list starts here",
                        "Save an article to begin building your reading list.",
                    )
                } else if filtered {
                    (
                        "No matching articles",
                        "Try a different search or clear your filters.",
                    )
                } else if unread {
                    (
                        "You're all caught up",
                        "Switch to Read to revisit articles you've finished.",
                    )
                } else {
                    (
                        "No read articles here",
                        "Switch to Unread to find your next article.",
                    )
                };
                controls::empty(
                    ui,
                    if state.view == View::Archive {
                        Icon::Archive
                    } else {
                        Icon::Inbox
                    },
                    title,
                    description,
                );
                egui::Frame::new().inner_margin(16).show(ui, |ui| {
                    if empty_library {
                        if controls::action(ui, "Save a link", ButtonKind::Primary).clicked() {
                            actions.push(Action::SaveDialog);
                        }
                    } else if filtered {
                        if controls::action(ui, "Clear filters", ButtonKind::Secondary).clicked() {
                            let mut query = state.query.clone();
                            query.search.clear();
                            query.publication.clear();
                            query.origin = Origin::All;
                            actions.push(Action::Query(query));
                        }
                    } else if controls::action(
                        ui,
                        if unread {
                            "Show read articles"
                        } else {
                            "Show unread articles"
                        },
                        ButtonKind::Secondary,
                    )
                    .clicked()
                    {
                        let mut query = state.query.clone();
                        query.read = if unread {
                            ReadFilter::Read
                        } else {
                            ReadFilter::Unread
                        };
                        actions.push(Action::Query(query));
                    }
                });
            }
        });
    scroll.state.offset.y
}
