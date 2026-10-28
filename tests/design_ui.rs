//! Exercise the rendered UI with synthetic content and real egui input events.
use egui::{Context, Event, FullOutput, Pos2, Rect, Shape, Vec2, pos2, vec2};
use macos_read_later::{
    domain::{
        Article, ArticleId, CopyStatus, Feed, FeedId, Origin, Query, ReadFilter, Scope, Sort,
    },
    reader_style::ReaderFont,
    rich_content::RichReader,
    ui::{self, Action, View, library, reader},
    whatsapp::{ConnectionState, WhatsAppView},
};
use std::collections::HashMap;

fn context() -> Context {
    let context = Context::default();
    context.enable_accesskit();
    let mut fonts = egui::FontDefinitions::default();
    let sans = fonts.families[&egui::FontFamily::Proportional].clone();
    for name in ReaderFont::ALL
        .into_iter()
        .map(ReaderFont::family)
        .chain(std::iter::once("System Semibold"))
    {
        fonts
            .families
            .insert(egui::FontFamily::Name(name.into()), sans.clone());
    }
    context.set_fonts(fonts);
    context
}

struct Frame {
    output: FullOutput,
    actions: Vec<Action>,
    reader: reader::ReaderLayout,
    viewport: Rect,
}

fn frame(
    context: &Context,
    size: Vec2,
    events: Vec<Event>,
    mut draw: impl FnMut(&mut egui::Ui, &mut Vec<Action>) -> reader::ReaderLayout,
) -> Frame {
    let mut actions = Vec::new();
    let mut layout = reader::ReaderLayout::default();
    let mut viewport = Rect::NOTHING;
    let mut output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |root_ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(root_ui, |ui| {
                    viewport = ui.max_rect();
                    layout = draw(ui, &mut actions);
                });
        },
    );
    // There is no GPU renderer in these interaction tests. Acknowledge font
    // atlas uploads, including glyphs first encountered in later UI states.
    output.textures_delta.clear();
    Frame {
        output,
        actions,
        reader: layout,
        viewport,
    }
}

fn node_rect(output: &FullOutput, label: &str) -> Rect {
    let tree = output
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("accessibility tree");
    let node = tree
        .nodes
        .iter()
        .map(|(_, node)| node)
        .find(|node| node.label() == Some(label))
        .unwrap_or_else(|| panic!("Missing accessible control: {label}"));
    let bounds = node.bounds().expect("control bounds");
    Rect::from_min_max(
        pos2(bounds.x0 as f32, bounds.y0 as f32),
        pos2(bounds.x1 as f32, bounds.y1 as f32),
    )
}

fn text_input_rect(output: &FullOutput) -> Rect {
    let tree = output
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("accessibility tree");
    let node = tree
        .nodes
        .iter()
        .map(|(_, node)| node)
        .find(|node| node.role() == egui::accesskit::Role::TextInput)
        .expect("search input");
    let bounds = node.bounds().expect("input bounds");
    Rect::from_min_max(
        pos2(bounds.x0 as f32, bounds.y0 as f32),
        pos2(bounds.x1 as f32, bounds.y1 as f32),
    )
}

fn pointer(position: Pos2, pressed: bool) -> Vec<Event> {
    vec![
        Event::PointerMoved(position),
        Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

fn text_rects(output: &FullOutput) -> Vec<(String, Rect)> {
    fn collect(shape: &Shape, out: &mut Vec<(String, Rect)>) {
        match shape {
            Shape::Text(text) => out.push((
                text.galley.job.text.clone(),
                Rect::from_min_size(text.pos, text.galley.size()),
            )),
            Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, out)),
            _ => {}
        }
    }
    let mut result = Vec::new();
    for shape in &output.shapes {
        collect(&shape.shape, &mut result);
    }
    result
}

fn article(id: &str, title: &str) -> Article {
    Article {
        id: ArticleId(id.into()),
        url: format!("https://example.com/{id}"),
        title: title.into(),
        author: Some("A reader".into()),
        site: "Example publication".into(),
        logo_url: None,
        excerpt: "A bounded preview of an article waiting to be read.".into(),
        word_count: 460,
        status: CopyStatus::Saved,
        error: None,
        published_at: Some("2026-10-20T10:00:00Z".into()),
        saved_at: "2026-10-21T10:00:00Z".into(),
        read_at: None,
        archived_at: None,
        deliberate: true,
        excluded: false,
        printed: 0,
        drafted: 0,
    }
}

struct Library {
    query: Query,
    library_total: u32,
    rows: Vec<Article>,
    selection: Vec<ArticleId>,
    selecting: bool,
}
impl Library {
    fn new() -> Self {
        Self {
            query: Query::for_scope(Scope::Inbox),
            library_total: 2,
            rows: vec![
                article("first", "First article"),
                article("second", "Second article"),
            ],
            selection: Vec::new(),
            selecting: false,
        }
    }
    fn frame(&self, context: &Context, events: Vec<Event>) -> Frame {
        frame(context, vec2(340., 900.), events, |ui, actions| {
            library::draw(
                ui,
                library::State {
                    view: View::Inbox,
                    query: &self.query,
                    total: self.rows.len() as u32,
                    library_total: self.library_total,
                    grouped: false,
                    logos: &HashMap::new(),
                    rows: &self.rows,
                    publications: &[],
                    selected: None,
                    selection: &self.selection,
                    selecting: self.selecting,
                    selecting_delete: false,
                    loading: false,
                    has_more: false,
                    scroll: 0.,
                },
                actions,
            );
            reader::ReaderLayout::default()
        })
    }
    fn click(&self, context: &Context, position: Pos2) -> Frame {
        self.frame(context, pointer(position, true));
        self.frame(context, pointer(position, false))
    }
}

#[test]
fn search_clear_and_read_switch_preserve_other_query_preferences() {
    let context = context();
    let mut library = Library::new();
    library.query.origin = Origin::Saved;
    library.query.sort = Sort::Publication;
    library.query.ascending = true;
    library.query.publication = "example.com".into();
    library.frame(&context, Vec::new());
    let initial = library.frame(&context, Vec::new());
    library.click(&context, text_input_rect(&initial.output).center());
    let typed = library.frame(&context, vec![Event::Text("forest".into())]);
    let mut expected = library.query.clone();
    expected.search = "forest".into();
    assert!(matches!(typed.actions.as_slice(), [Action::Query(query)] if query == &expected));
    library.query = expected;
    let populated = library.frame(&context, Vec::new());
    let cleared = library.click(
        &context,
        node_rect(&populated.output, "Clear search").center(),
    );
    let mut expected = library.query.clone();
    expected.search.clear();
    assert!(matches!(cleared.actions.as_slice(), [Action::Query(query)] if query == &expected));
    library.query = expected;
    let ready = library.frame(&context, Vec::new());
    let switched = library.click(&context, node_rect(&ready.output, "Read").center());
    let mut expected = library.query.clone();
    expected.read = ReadFilter::Read;
    assert!(matches!(switched.actions.as_slice(), [Action::Query(query)] if query == &expected));
}

#[test]
fn magazine_selection_keeps_click_order_and_has_a_safe_exit() {
    let context = context();
    let mut library = Library::new();
    library.selecting = true;
    library.selection.push(library.rows[1].id.clone());
    library.frame(&context, Vec::new());
    let initial = library.frame(&context, Vec::new());
    let chosen = library.click(
        &context,
        node_rect(&initial.output, "First article, Unread").center(),
    );
    assert!(
        matches!(chosen.actions.as_slice(), [Action::ToggleSelection(id)] if id == &library.rows[0].id)
    );
    library.selection.push(library.rows[0].id.clone());
    let selected = library.frame(&context, Vec::new());
    let labels = text_rects(&selected.output);
    let first_number = labels
        .iter()
        .find(|(label, _)| label == "1")
        .expect("first chosen order")
        .1;
    let second_number = labels
        .iter()
        .find(|(label, _)| label == "2")
        .expect("second chosen order")
        .1;
    assert!(
        first_number.top() > second_number.top(),
        "reading order follows clicks, rather than list order"
    );
    let create = library.click(
        &context,
        node_rect(&selected.output, "Create draft").center(),
    );
    assert!(matches!(create.actions.as_slice(), [Action::DraftDialog]));
    let cancelled = library.click(&context, node_rect(&selected.output, "Cancel").center());
    assert!(matches!(
        cancelled.actions.as_slice(),
        [Action::CancelSelection]
    ));
    library.selection.clear();
    let empty = library.frame(&context, Vec::new());
    let disabled = library.click(&context, node_rect(&empty.output, "Create draft").center());
    assert!(
        disabled.actions.is_empty(),
        "an empty selection cannot open a draft"
    );
}

#[test]
fn empty_library_and_empty_read_filters_offer_the_relevant_next_action() {
    for (retained, read, title, action_label) in [
        (
            0,
            ReadFilter::Unread,
            "Your reading list starts here",
            "Save a link",
        ),
        (
            2,
            ReadFilter::Unread,
            "You're all caught up",
            "Show read articles",
        ),
        (
            2,
            ReadFilter::Read,
            "No read articles here",
            "Show unread articles",
        ),
    ] {
        let context = context();
        let mut library = Library::new();
        library.rows.clear();
        library.library_total = retained;
        library.query.read = read;
        library.query.sort = Sort::Published;
        library.query.ascending = true;
        library.frame(&context, Vec::new());
        let empty = library.frame(&context, Vec::new());
        assert!(
            text_rects(&empty.output)
                .iter()
                .any(|(label, _)| label == title)
        );
        let clicked = library.click(&context, node_rect(&empty.output, action_label).center());
        if retained == 0 {
            assert!(matches!(clicked.actions.as_slice(), [Action::SaveDialog]));
        } else {
            let mut expected = library.query.clone();
            expected.read = if read == ReadFilter::Unread {
                ReadFilter::Read
            } else {
                ReadFilter::Unread
            };
            assert!(
                matches!(clicked.actions.as_slice(), [Action::Query(query)] if query == &expected)
            );
        }
    }
    let context = context();
    let mut library = Library::new();
    library.rows.clear();
    library.query.search = "unmatched phrase".into();
    library.query.origin = Origin::Saved;
    library.query.publication = "example.com".into();
    library.query.sort = Sort::Publication;
    library.frame(&context, Vec::new());
    let filtered = library.frame(&context, Vec::new());
    assert!(
        text_rects(&filtered.output)
            .iter()
            .any(|(label, _)| label == "No matching articles")
    );
    let clicked = library.click(
        &context,
        node_rect(&filtered.output, "Clear filters").center(),
    );
    let mut expected = library.query.clone();
    expected.search.clear();
    expected.publication.clear();
    expected.origin = Origin::All;
    assert!(matches!(clicked.actions.as_slice(), [Action::Query(query)] if query == &expected));
}

fn reader_frame(
    context: &Context,
    a: &Article,
    rich: &RichReader,
    native: bool,
    loading: bool,
    size: f32,
    events: Vec<Event>,
) -> Frame {
    frame(context, vec2(850., 900.), events, |ui, actions| {
        reader::draw(
            ui,
            reader::ReaderView {
                article: Some(a),
                reader: rich,
                size,
                font: ReaderFont::Georgia,
                loading,
                error: None,
                has_copy: rich
                    .document
                    .as_ref()
                    .is_some_and(|document| !document.blocks.is_empty()),
                native,
                fetched_at: Some("2026-10-21T10:00:00Z"),
            },
            actions,
        )
    })
}

#[test]
fn reader_toolbar_obeys_size_limits_and_preserves_native_view_geometry() {
    let a = article("first", "A reader title");
    let rich = RichReader::default();
    for native in [false, true] {
        for size in [15., 27.] {
            let context = context();
            reader_frame(&context, &a, &rich, native, false, size, Vec::new());
            let ready = reader_frame(&context, &a, &rich, native, false, size, Vec::new());
            if native {
                let body = ready.reader.body.expect("native body slot");
                let picker = ready.reader.font_picker.expect("native font picker slot");
                assert_eq!(
                    body.top(),
                    ready.viewport.top() + ui::TITLEBAR_INSET + reader::TOOLBAR_HEIGHT
                );
                assert_eq!(body.bottom(), ready.viewport.bottom());
                assert!(ready.viewport.contains_rect(body));
                assert!(picker.bottom() <= body.top() && ready.viewport.contains_rect(picker));
            } else {
                assert!(ready.reader.body.is_none() && ready.reader.font_picker.is_none());
            }
            let blocked = if size == 15. { "A−" } else { "A+" };
            let position = node_rect(&ready.output, blocked).center();
            reader_frame(
                &context,
                &a,
                &rich,
                native,
                false,
                size,
                pointer(position, true),
            );
            let result = reader_frame(
                &context,
                &a,
                &rich,
                native,
                false,
                size,
                pointer(position, false),
            );
            assert!(
                result.actions.is_empty(),
                "text size stays within the supported range"
            );
            let allowed = if size == 15. { "A+" } else { "A−" };
            let position = node_rect(&ready.output, allowed).center();
            reader_frame(
                &context,
                &a,
                &rich,
                native,
                false,
                size,
                pointer(position, true),
            );
            let result = reader_frame(
                &context,
                &a,
                &rich,
                native,
                false,
                size,
                pointer(position, false),
            );
            let expected = if size == 15. { 16. } else { 26. };
            assert!(
                matches!(result.actions.as_slice(), [Action::FontSize(value)] if *value == expected)
            );
        }
    }
}

#[test]
fn acquisition_keeps_title_and_long_preview_in_the_reading_column() {
    let context = context();
    let mut a = article(
        "first",
        "A long article title that should wrap inside the editorial reading column",
    );
    a.excerpt = "This deliberately long preview must remain inside the reading column while the article is fetched on demand. ".repeat(8);
    let mut rich = RichReader::default();
    rich.set_html("<p>A retained paragraph with <em>emphasis</em> and a <a href='https://example.com/source'>source link</a>.</p>");
    reader_frame(&context, &a, &rich, false, true, 19., Vec::new());
    let loading = reader_frame(&context, &a, &rich, false, true, 19., Vec::new());
    let loading_text = text_rects(&loading.output);
    let title = loading_text
        .iter()
        .find(|(label, _)| label == &a.title)
        .expect("loading title")
        .1;
    let preview = loading_text
        .iter()
        .find(|(label, _)| label == &a.excerpt)
        .expect("loading preview")
        .1;
    assert!(title.width() <= 780. && preview.width() <= 780.);
    assert!(
        title.left() > loading.viewport.left() && preview.right() < loading.viewport.right(),
        "Loading content must fit the inset column: title={title:?}, preview={preview:?}, viewport={:?}",
        loading.viewport
    );
    let loaded = reader_frame(&context, &a, &rich, false, false, 19., Vec::new());
    let loaded_text = text_rects(&loaded.output);
    let loaded_title = loaded_text
        .iter()
        .find(|(label, _)| label == &a.title)
        .expect("loaded title")
        .1;
    assert!((title.left() - loaded_title.left()).abs() <= 1.);
    assert!((title.top() - loaded_title.top()).abs() <= 1.);
    assert!(
        loaded_text
            .iter()
            .any(|(label, _)| label.contains("A retained paragraph"))
    );
}

#[test]
fn secondary_pages_keep_narrow_content_and_primary_actions_reachable() {
    let feed = Feed {
        id: FeedId("feed".into()),
        url: "https://example.com/a/long/feed/path/for/a/publication.xml".into(),
        title: "A publication with a longer name".into(),
        active: true,
        activity: Default::default(),
        last_polled_at: None,
        error: Some("The publisher did not respond. Try checking this feed again.".into()),
    };
    for page in [View::Feeds, View::ImportExport, View::WhatsApp] {
        let context = context();
        let mut draw = |ui: &mut egui::Ui, actions: &mut Vec<Action>| {
            match page {
                View::Feeds => ui::secondary::feeds(ui, std::slice::from_ref(&feed), actions),
                View::ImportExport => ui::import_export::draw(ui, &[], None, actions),
                View::WhatsApp => ui::whatsapp::draw(
                    ui,
                    &WhatsAppView {
                        state: ConnectionState::Connected,
                        ..Default::default()
                    },
                    &mut String::new(),
                    false,
                    None,
                    actions,
                ),
                _ => unreachable!(),
            }
            reader::ReaderLayout::default()
        };
        frame(&context, vec2(520., 1200.), Vec::new(), &mut draw);
        let output = frame(&context, vec2(520., 1200.), Vec::new(), &mut draw);
        for (label, rect) in text_rects(&output.output) {
            if rect.top() < output.viewport.bottom() && rect.bottom() > output.viewport.top() {
                assert!(
                    rect.left() >= output.viewport.left() - 1.
                        && rect.right() <= output.viewport.right() + 1.,
                    "{page:?} text exceeds the narrow viewport: {label:?} at {rect:?}"
                );
            }
        }
        let primary = match page {
            View::Feeds => "Add feed",
            View::ImportExport => "Choose import file",
            View::WhatsApp => "Disconnect",
            _ => unreachable!(),
        };
        let position = node_rect(&output.output, primary).center();
        assert!(output.viewport.contains(position));
        frame(
            &context,
            vec2(520., 1200.),
            pointer(position, true),
            &mut draw,
        );
        let clicked = frame(
            &context,
            vec2(520., 1200.),
            pointer(position, false),
            &mut draw,
        );
        assert!(match page {
            View::Feeds => matches!(clicked.actions.as_slice(), [Action::FeedDialog]),
            View::ImportExport => matches!(clicked.actions.as_slice(), [Action::ChooseImport]),
            View::WhatsApp => matches!(clicked.actions.as_slice(), [Action::WhatsAppDisconnect]),
            _ => false,
        });
    }
}

#[test]
fn reader_error_details_preserve_the_available_copy_and_report_exact_failure() {
    for has_copy in [false, true] {
        let context = context();
        let a = article("recovery", "The article remains identified");
        let mut rich = RichReader::default();
        if has_copy {
            rich.set_html("<p>The existing reading copy remains available.</p>");
        }
        let before = rich
            .document
            .as_ref()
            .map(|document| document.source_sha256.clone());
        let error = "Synthetic fetch failed while preserving the current library state.";
        let mut draw = |ui: &mut egui::Ui, actions: &mut Vec<Action>| {
            reader::draw(
                ui,
                reader::ReaderView {
                    article: Some(&a),
                    reader: &rich,
                    size: 19.,
                    font: ReaderFont::Georgia,
                    loading: false,
                    error: Some(error),
                    has_copy,
                    fetched_at: None,
                    native: false,
                },
                actions,
            )
        };
        frame(&context, vec2(850., 900.), Vec::new(), &mut draw);
        let ready = frame(&context, vec2(850., 900.), Vec::new(), &mut draw);
        let title = if has_copy {
            "Your reading copy is still available"
        } else {
            "Reading copy unavailable"
        };
        assert!(
            text_rects(&ready.output)
                .iter()
                .any(|(text, _)| text == title)
        );
        if has_copy {
            assert!(
                text_rects(&ready.output)
                    .iter()
                    .any(|(text, _)| text.contains("The existing reading copy remains available."))
            );
        }
        let position = node_rect(&ready.output, "Details").center();
        frame(
            &context,
            vec2(850., 900.),
            pointer(position, true),
            &mut draw,
        );
        let details = frame(
            &context,
            vec2(850., 900.),
            pointer(position, false),
            &mut draw,
        );
        assert!(
            matches!(details.actions.as_slice(), [Action::ShowDetails { title: actual_title, message }] if actual_title == title && message == error)
        );
        assert_eq!(
            rich.document
                .as_ref()
                .map(|document| document.source_sha256.clone()),
            before
        );
        assert!(a.read_at.is_none() && a.archived_at.is_none());
    }
}

#[test]
fn pending_file_transfer_disables_both_choosers_at_normal_and_short_heights() {
    for height in [560., 900.] {
        for kind in [
            macos_read_later::backend::TransferKind::Import,
            macos_read_later::backend::TransferKind::Export,
        ] {
            let context = context();
            let mut draw = |ui: &mut egui::Ui, actions: &mut Vec<Action>| {
                ui::import_export::draw(ui, &[], Some(kind), actions);
                reader::ReaderLayout::default()
            };
            frame(&context, vec2(760., height), Vec::new(), &mut draw);
            let busy = frame(&context, vec2(760., height), Vec::new(), &mut draw);
            let tree = busy
                .output
                .platform_output
                .accesskit_update
                .as_ref()
                .expect("accessibility tree");
            for label in ["Choose import file", "Export library ZIP"] {
                let control = tree
                    .nodes
                    .iter()
                    .map(|(_, node)| node)
                    .find(|node| node.label() == Some(label))
                    .expect("chooser remains discoverable during transfer");
                assert!(
                    control.is_disabled(),
                    "both transfers must prevent competing chooser actions"
                );
                let bounds = node_rect(&busy.output, label);
                if busy.viewport.contains(bounds.center()) {
                    frame(
                        &context,
                        vec2(760., height),
                        pointer(bounds.center(), true),
                        &mut draw,
                    );
                    let clicked = frame(
                        &context,
                        vec2(760., height),
                        pointer(bounds.center(), false),
                        &mut draw,
                    );
                    assert!(clicked.actions.is_empty());
                }
            }
        }
    }
}
