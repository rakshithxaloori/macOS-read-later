//! Modal presentation and input only. The app owns validation and effects.
use super::controls::{self, ButtonKind};
use egui::{Context, RichText, vec2};

#[derive(Clone, Copy)]
pub enum Kind<'a> {
    SaveLink,
    AddFeed,
    Delete { description: &'a str },
    Details { title: &'a str, message: &'a str },
}

#[derive(Default)]
pub struct Input {
    pub value: String,
    pub error: String,
    pub focus: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Submit,
    Cancel,
}

pub fn draw(ctx: &Context, kind: Kind<'_>, input: &mut Input) -> Option<Decision> {
    let colors = crate::theme::palette(ctx);
    // TextEdit consumes Escape to release focus. Capture it before drawing so
    // the first press also closes this modal when the URL field owns focus.
    let escape = ctx.input(|state| state.key_pressed(egui::Key::Escape));
    let mut decision = None;
    let details = matches!(kind, Kind::Details { .. });
    let title = match kind {
        Kind::SaveLink => "Save a link",
        Kind::AddFeed => "Add a feed",
        Kind::Delete { .. } => "Delete articles",
        Kind::Details { title, .. } => title,
    };
    let response = egui::Modal::new(egui::Id::new(if details {
        "reading-details"
    } else {
        "library-dialog"
    }))
    .frame(egui::Frame::popup(&ctx.global_style()).inner_margin(24))
    .show(ctx, |ui| {
        ui.set_width(if details {
            (ctx.content_rect().width() - 64.).clamp(280., 520.)
        } else {
            440.
        });
        ui.label(
            RichText::new(title)
                .size(if details { 22. } else { 24. })
                .color(colors.ink),
        );
        ui.add_space(16.);
        match kind {
            Kind::Details { message, .. } => {
                egui::ScrollArea::vertical()
                    .id_salt("diagnostic-message")
                    .max_height((ctx.content_rect().height() * 0.5).max(120.))
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(message).wrap().selectable(true));
                    });
            }
            Kind::Delete { description } => {
                ui.label(description);
                ui.add_space(6.);
                ui.label(
                    RichText::new("Saved magazine PDFs remain available.")
                        .size(12.)
                        .color(colors.muted),
                );
            }
            Kind::SaveLink | Kind::AddFeed => {
                let feed = matches!(kind, Kind::AddFeed);
                let label = ui.label(if feed {
                    "RSS or Atom feed URL"
                } else {
                    "Article URL"
                });
                let field = ui
                    .add_sized(
                        [440., 38.],
                        egui::TextEdit::singleline(&mut input.value)
                            .id(egui::Id::new("dialog-input"))
                            .margin(vec2(12., 10.))
                            .hint_text(if feed {
                                "https://example.com/feed.xml"
                            } else {
                                "https://example.com/article"
                            }),
                    )
                    .labelled_by(label.id);
                if input.focus {
                    // Modal focus is established before this field exists.
                    // Request its actual response after creation, not from
                    // the previous screen's focus scope.
                    field.request_focus();
                    input.focus = false;
                }
                if field.changed() {
                    input.error.clear();
                }
                if (field.has_focus() || field.lost_focus())
                    && ui.input(|state| state.key_pressed(egui::Key::Enter))
                {
                    decision = Some(Decision::Submit);
                }
                ui.add_space(6.);
                if input.error.is_empty() {
                    ui.label(
                        RichText::new(if feed {
                            "Paste the publication's RSS or Atom feed address."
                        } else {
                            "Paste a link to keep it in your reading list."
                        })
                        .size(12.)
                        .color(colors.muted),
                    );
                } else {
                    ui.label(RichText::new(&input.error).size(12.).color(colors.danger));
                }
            }
        }
        ui.add_space(18.);
        controls::action_row(ui, |ui| {
            if details {
                if controls::action(ui, "Close", ButtonKind::Primary).clicked() {
                    decision = Some(Decision::Cancel);
                }
            } else {
                let label = match kind {
                    Kind::SaveLink => "Save link",
                    Kind::AddFeed => "Add feed",
                    Kind::Delete { .. } => "Delete articles",
                    Kind::Details { .. } => unreachable!(),
                };
                if controls::action(
                    ui,
                    label,
                    if matches!(kind, Kind::Delete { .. }) {
                        ButtonKind::Danger
                    } else {
                        ButtonKind::Primary
                    },
                )
                .clicked()
                {
                    decision = Some(Decision::Submit);
                }
                if controls::action(ui, "Cancel", ButtonKind::Quiet).clicked() {
                    decision = Some(Decision::Cancel);
                }
            }
        });
    });
    if escape || response.should_close() {
        Some(Decision::Cancel)
    } else {
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, FullOutput, Pos2, Rect, Shape};

    fn context() -> Context {
        let context = Context::default();
        context.enable_accesskit();
        context.set_theme(egui::Theme::Light);
        context
    }
    fn frame(
        ctx: &Context,
        kind: Kind<'_>,
        input: &mut Input,
        height: f32,
        events: Vec<Event>,
    ) -> (FullOutput, Option<Decision>) {
        let mut decision = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900., height))),
                events,
                ..Default::default()
            },
            |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    decision = draw(ui.ctx(), kind, input);
                });
            },
        );
        // This harness has no renderer. Acknowledge every atlas upload,
        // including glyphs introduced when diagnostics change.
        output.textures_delta.clear();
        (output, decision)
    }
    fn key(key: egui::Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }
    fn button_rect(output: &FullOutput, label: &str) -> Rect {
        let node = output
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accessibility tree")
            .nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| {
                node.label() == Some(label) && node.role() == egui::accesskit::Role::Button
            })
            .unwrap_or_else(|| panic!("Missing modal action {label}"));
        let bounds = node.bounds().expect("button bounds");
        Rect::from_min_max(
            egui::pos2(bounds.x0 as f32, bounds.y0 as f32),
            egui::pos2(bounds.x1 as f32, bounds.y1 as f32),
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
    fn click(ctx: &Context, kind: Kind<'_>, input: &mut Input, position: Pos2) -> Option<Decision> {
        frame(ctx, kind, input, 800., pointer(position, true));
        frame(ctx, kind, input, 800., pointer(position, false)).1
    }
    fn modal_rect(output: &FullOutput, content_width: f32) -> Rect {
        fn rectangles(shape: &Shape, output: &mut Vec<Rect>) {
            match shape {
                Shape::Rect(rect) => output.push(rect.rect),
                Shape::Vec(shapes) => shapes.iter().for_each(|shape| rectangles(shape, output)),
                _ => {}
            }
        }
        let mut rects = Vec::new();
        for shape in &output.shapes {
            rectangles(&shape.shape, &mut rects);
        }
        rects.iter().copied().filter(|rect| rect.width() >= content_width + 48. && rect.width() <= content_width + 52.)
            .max_by(|a, b| a.height().total_cmp(&b.height()))
            .unwrap_or_else(|| panic!("Missing modal frame for {content_width}pt content; painted rectangles: {rects:?}"))
    }
    fn diagnostic_text(output: &FullOutput, message: &str) -> (Pos2, Rect) {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Text(text) if text.galley.job.text == message => {
                    Some((text.pos, shape.clip_rect))
                }
                _ => None,
            })
            .expect("complete diagnostic message")
    }

    #[test]
    fn opening_url_dialog_focuses_real_field_for_paste_and_enter() {
        for kind in [Kind::SaveLink, Kind::AddFeed] {
            let ctx = context();
            let mut input = Input {
                focus: true,
                ..Default::default()
            };
            assert_eq!(frame(&ctx, kind, &mut input, 800., Vec::new()).1, None);
            assert!(!input.focus);
            assert_eq!(
                ctx.memory(|memory| memory.focused()),
                Some(egui::Id::new("dialog-input"))
            );
            let url = "https://example.com/pasted-on-opening";
            assert_eq!(
                frame(&ctx, kind, &mut input, 800., vec![Event::Paste(url.into())]).1,
                None
            );
            assert_eq!(input.value, url);
            assert_eq!(
                frame(&ctx, kind, &mut input, 800., vec![key(egui::Key::Enter)]).1,
                Some(Decision::Submit)
            );
            assert_eq!(input.value, url);
        }
    }

    #[test]
    fn escape_cancel_and_backdrop_close_without_submission() {
        let ctx = context();
        let mut input = Input {
            value: "a link being edited".into(),
            focus: true,
            ..Default::default()
        };
        frame(&ctx, Kind::SaveLink, &mut input, 800., Vec::new());
        assert_eq!(
            frame(
                &ctx,
                Kind::SaveLink,
                &mut input,
                800.,
                vec![key(egui::Key::Escape)]
            )
            .1,
            Some(Decision::Cancel)
        );
        assert_eq!(input.value, "a link being edited");
        for backdrop in [false, true] {
            let ctx = context();
            let mut input = Input::default();
            let kind = Kind::Delete {
                description: "Delete three articles from your library?",
            };
            frame(&ctx, kind, &mut input, 800., Vec::new());
            let (output, _) = frame(&ctx, kind, &mut input, 800., Vec::new());
            let position = if backdrop {
                egui::pos2(10., 10.)
            } else {
                button_rect(&output, "Cancel").center()
            };
            assert_eq!(
                click(&ctx, kind, &mut input, position),
                Some(Decision::Cancel)
            );
        }
    }

    #[test]
    fn url_modal_and_footer_stay_compact_across_window_heights_and_validation() {
        for height in [600., 800., 1200.] {
            let ctx = context();
            let mut input = Input {
                error: "Use an HTTP or HTTPS address without credentials.".into(),
                ..Default::default()
            };
            frame(&ctx, Kind::SaveLink, &mut input, height, Vec::new());
            let (output, _) = frame(&ctx, Kind::SaveLink, &mut input, height, Vec::new());
            let modal = modal_rect(&output, 440.);
            let submit = button_rect(&output, "Save link");
            let cancel = button_rect(&output, "Cancel");
            assert!(
                modal.height() < 300.,
                "modal grew into unused window space: {modal:?}"
            );
            assert!((submit.height() - controls::ACTION_HEIGHT).abs() <= 1.);
            assert!((submit.center().y - cancel.center().y).abs() <= 1.);
            assert!(modal.contains_rect(submit) && modal.contains_rect(cancel));
            assert!(submit.right() > cancel.right());
            assert!(modal.bottom() - submit.bottom() < 30.);
        }
    }

    #[test]
    fn changing_diagnostics_stays_scrollable_with_close_in_view() {
        for height in [420., 900.] {
            let ctx = context();
            let mut input = Input::default();
            frame(
                &ctx,
                Kind::Details {
                    title: "Fetch details",
                    message: "Short message",
                },
                &mut input,
                height,
                Vec::new(),
            );
            let message = (0..100)
                .map(|n| format!("Diagnostic line {n:03}: complete retained evidence\n"))
                .collect::<String>();
            let kind = Kind::Details {
                title: "Fetch details",
                message: &message,
            };
            frame(&ctx, kind, &mut input, height, Vec::new());
            let (output, _) = frame(&ctx, kind, &mut input, height, Vec::new());
            let modal = modal_rect(&output, 520.);
            let close = button_rect(&output, "Close");
            let (before, clip) = diagnostic_text(&output, &message);
            assert!(
                modal.height() < height - 12.,
                "diagnostics escaped the viewport: {modal:?}"
            );
            assert!(close.bottom() <= height && modal.contains_rect(close));
            assert!(clip.height() <= height * 0.5 + 2.);
            frame(
                &ctx,
                kind,
                &mut input,
                height,
                vec![
                    Event::PointerMoved(clip.center()),
                    Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: vec2(0., -300.),
                        phase: egui::TouchPhase::Move,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            let (scrolled, _) = frame(&ctx, kind, &mut input, height, Vec::new());
            let (after, _) = diagnostic_text(&scrolled, &message);
            assert!(after.y < before.y, "long diagnostic content did not scroll");
            let position = button_rect(&scrolled, "Close").center();
            frame(&ctx, kind, &mut input, height, pointer(position, true));
            assert_eq!(
                frame(&ctx, kind, &mut input, height, pointer(position, false)).1,
                Some(Decision::Cancel)
            );
        }
    }
}
