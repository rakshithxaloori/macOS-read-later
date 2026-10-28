use super::{
    Action,
    controls::{self, ButtonKind},
    icons::Icon,
};
use crate::{
    backend::{Command, TransferKind},
    imports::{Item, Progress},
};
use egui::{RichText, Ui};

pub fn draw(
    ui: &mut Ui,
    runs: &[Progress],
    activity: Option<TransferKind>,
    actions: &mut Vec<Action>,
) {
    let colors = crate::theme::palette(ui.ctx());
    egui::ScrollArea::vertical().id_salt("import-export").show(ui, |ui| {
        ui.set_max_width(ui.available_width().min(880.));
        super::secondary::header(ui, "Import / export", "Bring your reading list with you.");
        controls::panel(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, Icon::Upload, "Import your reading");
            ui.label("Choose a file, review its preview, then start importing.");
            ui.add_space(8.);
            ui.label(RichText::new("Matter CSV / ZIP · OPML · bookmarks HTML · URL CSV · portable JSON / ZIP").size(12.).color(colors.muted));
            ui.label(RichText::new("Up to 20 MB and 5,000 records. Successful rows survive interruption.").size(12.).color(colors.muted));
            ui.add_space(16.);
            if activity == Some(TransferKind::Import) {
                ui.horizontal(|ui| { controls::status(ui, controls::Status::Loading); ui.label("Preparing your preview…"); });
                ui.add_space(10.);
            }
            if ui.add_enabled_ui(activity.is_none(), |ui| controls::action(ui, "Choose import file", ButtonKind::Primary)).inner.clicked() {
                actions.push(Action::ChooseImport);
            }
        });
        ui.add_space(20.);
        controls::panel(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, Icon::Download, "Export your library");
            ui.label("Keep a portable copy of retained metadata, subscriptions, stored PDFs and current draft assets.");
            ui.label(RichText::new("Credentials and disposable article copies are excluded. Article bodies are fetched on demand.").size(12.).color(colors.muted));
            ui.add_space(14.);
            if activity == Some(TransferKind::Export) {
                ui.horizontal(|ui| { controls::status(ui, controls::Status::Loading); ui.label("Exporting your library…"); });
                ui.add_space(10.);
            }
            if ui.add_enabled_ui(activity.is_none(), |ui| controls::action(ui, "Export library ZIP", ButtonKind::Secondary)).inner.clicked() {
                actions.push(Action::ChooseExport);
            }
        });
        ui.add_space(28.);
        ui.label(RichText::new("Import activity").size(20.).color(colors.ink));
        ui.add_space(10.);
        if runs.is_empty() {
            controls::empty(ui, Icon::Upload, "Ready when you are", "Your file previews and import reports will appear here.");
        }
        for run in runs {
            ui.push_id(&run.id, |ui| report(ui, run, actions));
            ui.add_space(16.);
        }
    });
}

fn section_title(ui: &mut Ui, icon: Icon, title: &str) {
    let colors = crate::theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(20., 20.), egui::Sense::hover());
        super::icons::draw(ui, rect, icon, colors.green);
        ui.label(RichText::new(title).size(18.).color(colors.ink));
    });
    ui.add_space(10.);
}

fn report(ui: &mut Ui, run: &Progress, actions: &mut Vec<Action>) {
    let colors = crate::theme::palette(ui.ctx());
    controls::panel(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            RichText::new(&run.filename)
                .size(15.)
                .strong()
                .color(colors.ink),
        );
        ui.horizontal_wrapped(|ui| {
            match run.state.as_str() {
                "preview" => controls::status(ui, controls::Status::Waiting),
                "queued" => controls::status(ui, controls::Status::Queued),
                "running" => controls::status(ui, controls::Status::Loading),
                "completed" => controls::status(ui, controls::Status::Completed),
                "failed" => controls::status(ui, controls::Status::Failed),
                other => controls::badge(ui, other, colors.muted),
            };
            ui.label(
                RichText::new(format!("{} · {} records", run.format, run.total))
                    .size(12.)
                    .color(colors.muted),
            );
        });
        ui.add_space(12.);
        if matches!(run.state.as_str(), "queued" | "running") {
            let processed = run.added + run.duplicates + run.failed;
            ui.horizontal(|ui| {
                ui.label(format!("{processed} of {} records processed", run.total));
            });
            ui.add(
                egui::ProgressBar::new(if run.total == 0 {
                    0.
                } else {
                    processed as f32 / run.total as f32
                })
                .desired_width(ui.available_width()),
            );
            ui.add_space(10.);
        }
        ui.horizontal_wrapped(|ui| {
            for (count, label, color) in [
                (run.added, "Added", colors.green),
                (run.duplicates, "Already saved", colors.muted),
                (run.pending, "Pending", colors.muted),
                (
                    run.failed,
                    "Failed",
                    if run.failed > 0 {
                        colors.danger
                    } else {
                        colors.muted
                    },
                ),
                (
                    run.invalid,
                    "Invalid",
                    if run.invalid > 0 {
                        colors.danger
                    } else {
                        colors.muted
                    },
                ),
            ] {
                ui.vertical(|ui| {
                    ui.set_min_width(76.);
                    ui.label(RichText::new(count.to_string()).size(20.).color(color));
                    ui.label(RichText::new(label).size(11.).color(colors.muted));
                });
            }
        });
        if let Some(error) = &run.error {
            ui.add_space(12.);
            ui.label(
                RichText::new("The import needs attention. Successful records are already saved.")
                    .color(colors.danger),
            );
            egui::CollapsingHeader::new("Error details")
                .id_salt("error")
                .show(ui, |ui| {
                    ui.label(error);
                });
        }
        if !run.preview_rows.is_empty() {
            ui.add_space(12.);
            egui::CollapsingHeader::new("Preview first 20 records")
                .id_salt("preview")
                .default_open(run.state == "preview")
                .show(ui, |ui| {
                    for (index, item) in run.preview_rows.iter().enumerate() {
                        let (kind, title, url) = match item {
                            Item::Article(article) => ("Article", &article.title, &article.url),
                            Item::Feed(feed) => ("Feed", &feed.title, &feed.url),
                        };
                        ui.horizontal_top(|ui| {
                            ui.label(
                                RichText::new(format!("{:02}", index + 1))
                                    .size(11.)
                                    .color(colors.muted),
                            );
                            ui.vertical(|ui| {
                                ui.set_max_width((ui.available_width() - 68.).max(100.));
                                ui.label(RichText::new(title).size(12.).color(colors.ink));
                                ui.label(RichText::new(url).size(11.).color(colors.muted));
                            });
                            controls::badge(ui, kind, colors.muted);
                        });
                        ui.add_space(5.);
                        ui.separator();
                    }
                });
        }
        let warnings = run.warnings.as_array();
        let validation_errors = run.validation_errors.as_array();
        if warnings.is_some_and(|items| !items.is_empty())
            || validation_errors.is_some_and(|items| !items.is_empty())
            || !run.row_errors.is_empty()
        {
            ui.add_space(10.);
            egui::CollapsingHeader::new("Warnings and row details")
                .id_salt("details")
                .show(ui, |ui| {
                    if let Some(warnings) = warnings {
                        for warning in warnings {
                            if let Some(message) = warning.as_str() {
                                ui.label(message);
                            }
                        }
                    }
                    for problem in &run.row_errors {
                        ui.label(format!("Row {}: {}", problem.row, problem.message));
                    }
                    if let Some(problems) = validation_errors {
                        for problem in problems {
                            ui.label(format!(
                                "Row {}: {}",
                                problem["row"],
                                problem["message"].as_str().unwrap_or("Invalid record")
                            ));
                        }
                    }
                });
        }
        ui.add_space(14.);
        ui.horizontal_wrapped(|ui| {
            if run.state == "preview"
                && controls::action(ui, "Start import", ButtonKind::Primary).clicked()
            {
                actions.push(Action::Command(Command::StartImport {
                    id: run.id.clone(),
                    retry: false,
                }));
            }
            if run.state == "failed"
                && controls::action(ui, "Retry failed rows", ButtonKind::Primary).clicked()
            {
                actions.push(Action::Command(Command::StartImport {
                    id: run.id.clone(),
                    retry: true,
                }));
            }
            if run.state != "running"
                && controls::action(ui, "Remove report", ButtonKind::Quiet).clicked()
            {
                actions.push(Action::Command(Command::RemoveImport(run.id.clone())));
            }
        });
    });
}
