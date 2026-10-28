//! Navigation symbols drawn in native egui, matching the reference's outline icons.
use egui::{Color32, Pos2, Rect, Stroke, Ui, vec2};
#[derive(Clone, Copy)]
pub enum Icon {
    Book,
    Plus,
    Inbox,
    Archive,
    Rss,
    Newspaper,
    Message,
    Download,
    Search,
    Close,
    Check,
    CheckSquare,
    Square,
    ArrowLeft,
    External,
    Refresh,
    More,
    Trash,
    ChevronDown,
    ChevronUp,
    ChevronRight,
    Warning,
    Grip,
    Upload,
    Play,
    Eye,
    Sun,
    Moon,
    Filter,
}
pub fn draw(ui: &Ui, rect: Rect, icon: Icon, color: Color32) {
    let size = rect.width().min(rect.height());
    let origin = rect.center() - vec2(size / 2., size / 2.);
    let p = |x: f32, y: f32| origin + vec2(x, y) * (size / 24.);
    let painter = ui.painter();
    let stroke = Stroke::new(1.6 * size / 24., color);
    let line = |points: &[(f32, f32)]| {
        painter.add(egui::Shape::line(
            points.iter().map(|&(x, y)| p(x, y)).collect::<Vec<Pos2>>(),
            stroke,
        ));
    };
    match icon {
        Icon::Filter => {
            line(&[(4., 7.), (20., 7.)]);
            line(&[(7., 12.), (17., 12.)]);
            line(&[(10., 17.), (14., 17.)]);
        }
        Icon::Sun => {
            painter.circle_stroke(p(12., 12.), size * 4. / 24., stroke);
            for (a, b) in [
                ((12., 1.), (12., 3.)),
                ((12., 21.), (12., 23.)),
                ((1., 12.), (3., 12.)),
                ((21., 12.), (23., 12.)),
                ((4., 4.), (5.5, 5.5)),
                ((18.5, 18.5), (20., 20.)),
                ((4., 20.), (5.5, 18.5)),
                ((18.5, 5.5), (20., 4.)),
            ] {
                line(&[a, b]);
            }
        }
        Icon::Moon => {
            // The inner arc gives a true outlined crescent without covering the background.
            let points: Vec<Pos2> = (0..=32)
                .map(|i| {
                    let a =
                        -std::f32::consts::FRAC_PI_2 - i as f32 * std::f32::consts::PI * 1.5 / 32.;
                    p(12. + 9. * a.cos(), 12. + 9. * a.sin())
                })
                .chain((0..=24).map(|i| {
                    let a =
                        std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::FRAC_PI_2 / 24.;
                    p(21. + 9. * a.cos(), 3. + 9. * a.sin())
                }))
                .collect();
            painter.add(egui::Shape::line(points, stroke));
        }
        Icon::Book => {
            line(&[
                (12., 5.),
                (8., 3.),
                (3., 3.),
                (2., 4.),
                (2., 20.),
                (8., 20.),
                (12., 22.),
                (16., 20.),
                (22., 20.),
                (22., 4.),
                (21., 3.),
                (16., 3.),
                (12., 5.),
                (12., 22.),
            ]);
        }
        Icon::Plus => {
            line(&[(12., 5.), (12., 19.)]);
            line(&[(5., 12.), (19., 12.)]);
        }
        Icon::Inbox => {
            line(&[
                (3., 13.),
                (5., 5.),
                (19., 5.),
                (21., 13.),
                (21., 20.),
                (3., 20.),
                (3., 13.),
                (8., 13.),
                (9., 16.),
                (15., 16.),
                (16., 13.),
                (21., 13.),
            ]);
        }
        Icon::Archive => {
            painter.rect_stroke(
                Rect::from_min_max(p(3., 3.), p(21., 7.)),
                1.,
                stroke,
                egui::StrokeKind::Inside,
            );
            line(&[(5., 7.), (5., 21.), (19., 21.), (19., 7.)]);
            line(&[(10., 11.), (14., 11.)]);
        }
        Icon::Rss => {
            painter.circle_filled(p(5., 19.), 1.4 * size / 24., color);
            for radius in [8., 15.] {
                let points = (0..=24)
                    .map(|i| {
                        let a = (i as f32 / 24.) * std::f32::consts::FRAC_PI_2;
                        p(5. + radius * a.sin(), 19. - radius * a.cos())
                    })
                    .collect();
                painter.add(egui::Shape::line(points, stroke));
            }
        }
        Icon::Newspaper => {
            line(&[
                (4., 7.),
                (2., 7.),
                (2., 20.),
                (4., 22.),
                (20., 22.),
                (22., 20.),
                (22., 3.),
                (6., 3.),
                (6., 20.),
                (4., 22.),
            ]);
            painter.rect_stroke(
                Rect::from_min_max(p(10., 7.), p(18., 11.)),
                0.,
                stroke,
                egui::StrokeKind::Inside,
            );
            line(&[(10., 15.), (18., 15.)]);
            line(&[(10., 18.), (18., 18.)]);
        }
        Icon::Message => {
            let points = (0..=40)
                .map(|i| {
                    let a = i as f32 / 40. * std::f32::consts::TAU;
                    p(12. + 9. * a.cos(), 11. + 9. * a.sin())
                })
                .collect();
            painter.add(egui::Shape::line(points, stroke));
            line(&[(5., 18.), (3., 22.), (9., 20.)]);
        }
        Icon::Download => {
            line(&[(12., 3.), (12., 16.)]);
            line(&[(7., 11.), (12., 16.), (17., 11.)]);
            line(&[(3., 16.), (3., 21.), (21., 21.), (21., 16.)]);
        }
        Icon::Search => {
            painter.circle_stroke(p(10., 10.), size * 6.5 / 24., stroke);
            line(&[(15., 15.), (21., 21.)]);
        }
        Icon::Close => {
            line(&[(6., 6.), (18., 18.)]);
            line(&[(18., 6.), (6., 18.)]);
        }
        Icon::Check => line(&[(5., 12.), (10., 17.), (20., 6.)]),
        Icon::Square | Icon::CheckSquare => {
            painter.rect_stroke(
                Rect::from_min_max(p(3., 3.), p(21., 21.)),
                3.,
                stroke,
                egui::StrokeKind::Inside,
            );
            if matches!(icon, Icon::CheckSquare) {
                line(&[(7., 12.), (11., 16.), (18., 8.)]);
            }
        }
        Icon::ArrowLeft => {
            line(&[(11., 5.), (4., 12.), (11., 19.)]);
            line(&[(4., 12.), (21., 12.)]);
        }
        Icon::External => {
            line(&[(14., 3.), (21., 3.), (21., 10.)]);
            line(&[(21., 3.), (10., 14.)]);
            line(&[(10., 4.), (4., 4.), (4., 20.), (20., 20.), (20., 14.)]);
        }
        Icon::Refresh => {
            let points = (0..=32)
                .map(|i| {
                    let a = -0.5 + i as f32 / 32. * std::f32::consts::PI * 1.65;
                    p(12. + 8. * a.cos(), 12. + 8. * a.sin())
                })
                .collect();
            painter.add(egui::Shape::line(points, stroke));
            line(&[(20., 3.), (20., 9.), (14., 9.)]);
        }
        Icon::More => {
            for x in [5., 12., 19.] {
                painter.circle_filled(p(x, 12.), size * 1.3 / 24., color);
            }
        }
        Icon::Trash => {
            line(&[(3., 6.), (21., 6.)]);
            line(&[(9., 6.), (9., 3.), (15., 3.), (15., 6.)]);
            line(&[(5., 6.), (6., 21.), (18., 21.), (19., 6.)]);
            line(&[(10., 10.), (10., 17.)]);
            line(&[(14., 10.), (14., 17.)]);
        }
        Icon::ChevronDown => line(&[(6., 9.), (12., 15.), (18., 9.)]),
        Icon::ChevronUp => line(&[(6., 15.), (12., 9.), (18., 15.)]),
        Icon::ChevronRight => line(&[(9., 6.), (15., 12.), (9., 18.)]),
        Icon::Warning => {
            line(&[(12., 3.), (22., 21.), (2., 21.), (12., 3.)]);
            line(&[(12., 9.), (12., 14.)]);
            painter.circle_filled(p(12., 17.5), size / 24., color);
        }
        Icon::Grip => {
            for x in [9., 15.] {
                for y in [6., 12., 18.] {
                    painter.circle_filled(p(x, y), size / 24., color);
                }
            }
        }
        Icon::Upload => {
            line(&[(12., 17.), (12., 3.)]);
            line(&[(7., 8.), (12., 3.), (17., 8.)]);
            line(&[(3., 17.), (3., 21.), (21., 21.), (21., 17.)]);
        }
        Icon::Play => line(&[(7., 4.), (21., 12.), (7., 20.), (7., 4.)]),
        Icon::Eye => {
            line(&[
                (2., 12.),
                (6., 7.),
                (12., 5.),
                (18., 7.),
                (22., 12.),
                (18., 17.),
                (12., 19.),
                (6., 17.),
                (2., 12.),
            ]);
            painter.circle_stroke(p(12., 12.), size * 3. / 24., stroke);
        }
    }
}
