//! Trace is always armed in the desktop editor; the panel is only a view of it.
use crate::WordApp;
use egui::{Color32, RichText};
use serde_json::json;

mod labels;
use labels::tr;

pub struct TraceUi {
    pub open: bool,
    pub directory: String,
    pub message: String,
    pub selected: Option<usize>,
    pub show_code: bool,
    pub code: String,
    pub checkpoint_at: f64,
    pub checkpoints: std::collections::BTreeMap<u64, (usize, std::path::PathBuf)>,
    #[cfg(not(target_arch = "wasm32"))]
    pub inspection: Option<std::sync::mpsc::Receiver<(u64, Result<wordcraft_engine::officecli::OfficeResult, String>)>>,
    pub report: String,
    pub show_report: bool,
    pub batch: String,
}

impl Default for TraceUi {
    fn default() -> Self {
        let parent = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("."));
        Self {
            open: true,
            directory: parent.join("Documents/WordCraft-Trace-Exports").to_string_lossy().into_owned(),
            message: String::new(),
            selected: None,
            show_code: false,
            code: String::new(),
            checkpoint_at: 0.0,
            checkpoints: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            inspection: None,
            report: String::new(),
            show_report: false,
            batch: "[\n  {\"command\": \"set\", \"path\": \"/body/p[1]\",\n   \"props\": {\"bold\": \"true\"}}\n]".into(),
        }
    }
}

pub fn show(app: &mut WordApp, ui: &mut egui::Ui) {
    app.session.trace_sync("document.change", "Document update");
    #[cfg(not(target_arch = "wasm32"))]
    tick(app, ui.ctx());
    egui::Panel::top("trace_strip").exact_size(30.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("● TRACE")).color(Color32::from_rgb(29, 157, 110)).strong());
            let count = app.session.trace_current().map_or(0, |j| j.steps.len());
            ui.label(format!("Automatic recording · {count} committed edits"));
            if ui.selectable_label(app.trace_ui.open, tr("History & Python")).clicked() {
                app.trace_ui.open = !app.trace_ui.open;
            }
        });
    });
    if !app.trace_ui.open {
        return;
    }
    egui::Panel::right("trace_panel").default_size(330.0).min_size(270.0).max_size(600.0).resizable(true).show(ui, |ui| {
        ui.add_space(8.0);
        ui.heading(tr("Document → Code"));
        ui.label(tr("Edit normally. Every committed change is captured automatically."));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(tr("Verify replay")).clicked() {
                let r = app.run("trace.verify", json!({}));
                app.trace_ui.message = match r {
                    Ok(v) => format!("Verified: {} edits. Model and DOCX round trip match.", v.get("steps").and_then(|v| v.as_u64()).unwrap_or(0)),
                    Err(e) => format!("Verification failed: {e}"),
                };
            }
            if ui.button(tr("Python code")).clicked() {
                app.trace_ui.code = app
                    .session
                    .trace_current()
                    .ok_or("No trace".to_string())
                    .and_then(|j| j.compact())
                    .and_then(|v| v.get("code").and_then(|s| s.as_str()).map(str::to_string).ok_or("No generated code".into()))
                    .unwrap_or_else(|e| e);
                app.trace_ui.show_code = true;
            }
        });
        if ui.button("简洁 OP / Compact operations").clicked() {
            app.trace_ui.code = app
                .session
                .trace_current()
                .ok_or("No trace".to_string())
                .and_then(|j| j.compact())
                .and_then(|v| serde_json::to_string_pretty(&v["ops"]).map_err(|e| e.to_string()))
                .unwrap_or_else(|e| e);
            app.trace_ui.show_code = true;
        }
        if ui.button(tr("Replay in a new document")).clicked() {
            let steps = app.trace_ui.selected.map(|i| i + 1);
            let params = steps.map_or_else(|| json!({}), |n| json!({"steps":n}));
            app.trace_ui.message = match app.run("trace.replay", params) {
                Ok(_) => "Replayed document opened. The original stays open.".into(),
                Err(e) => format!("Replay failed: {e}"),
            };
            app.trace_ui.selected = None;
        }
        ui.add_space(8.0);
        #[cfg(not(target_arch = "wasm32"))]
        ui.horizontal(|ui| {
            if ui.button(tr("Resume project…")).clicked()
                && let Some(path) = rfd::FileDialog::new().set_directory(history_root()).pick_folder()
            {
                app.trace_ui.message = match app.run("trace.resume", json!({"directory":path})) {
                    Ok(_) => "Project restored. New edits continue its history.".into(),
                    Err(e) => format!("Cannot restore project: {e}"),
                };
            }
            if ui.add_enabled(app.trace_ui.inspection.is_none(), egui::Button::new(tr("OfficeCLI check"))).clicked() {
                inspect(app, "validate");
            }
        });
        #[cfg(not(target_arch = "wasm32"))]
        if app.trace_ui.inspection.is_some() {
            ui.label(tr("OfficeCLI is checking the current document…"));
        }
        #[cfg(not(target_arch = "wasm32"))]
        ui.collapsing(tr("OfficeCLI tools"), |ui| {
            if ui.add_enabled(app.trace_ui.inspection.is_none(), egui::Button::new(tr("Read document structure"))).clicked() {
                inspect(app, "outline");
            }
            ui.label(tr("Advanced batch edits (JSON)"));
            ui.add(egui::TextEdit::multiline(&mut app.trace_ui.batch).code_editor().desired_width(f32::INFINITY).desired_rows(5));
            if ui.add_enabled(app.trace_ui.inspection.is_none(), egui::Button::new(tr("Apply batch & record"))).clicked() {
                inspect(app, "batch");
            }
            ui.small(tr("Runs on a copy. A successful batch becomes one undoable, replayable edit."));
        });
        if app.trace_ui.checkpoints.contains_key(&app.session.document_id()) {
            ui.small(tr("History automatically saved · Resume project to restore"));
        }
        ui.add_space(8.0);
        ui.label(tr("Export folder"));
        ui.add(egui::TextEdit::singleline(&mut app.trace_ui.directory).desired_width(f32::INFINITY));
        if ui.add_sized([ui.available_width(), 32.0], egui::Button::new(tr("Export Python + original + edited DOCX"))).clicked() {
            let stamp = wordcraft_engine::cmd::now_iso().replace([':', '-'], "");
            let target = std::path::PathBuf::from(&app.trace_ui.directory).join(format!("trace-{stamp}"));
            let r = app.run("trace.export", json!({"directory": target}));
            app.trace_ui.message = match r {
                Ok(_) => format!("Exported and verified:\n{}\nRun replay.py with the original file.", target.display()),
                Err(e) => format!("Export failed: {e}"),
            };
        }
        ui.collapsing("完整原生记录归档 / Native archive", |ui| {
            ui.small("独立 Python 不支持的操作仍可归档；原生归档回放需要 WordCraft。");
            if ui.button("导出完整归档").clicked() {
                let stamp = wordcraft_engine::cmd::now_iso().replace([':', '-'], "");
                let path = std::path::PathBuf::from(&app.trace_ui.directory).join(format!("archive-{stamp}"));
                app.trace_ui.message = match app.run("trace.exportArchive", json!({"directory":path})) {
                    Ok(_) => format!("Archived: {}", path.display()),
                    Err(e) => e,
                };
            }
        });
        if let Some(e) = &app.session.trace.error {
            ui.colored_label(Color32::RED, format!("Recording error: {e}"));
        }
        if !app.trace_ui.message.is_empty() {
            ui.add_space(6.0);
            ui.label(&app.trace_ui.message);
        }
        ui.separator();
        let journal = app.session.trace_current();
        if let Some(journal) = journal {
            ui.small(format!("{} recorded steps · images included", journal.steps.len()));
            egui::ScrollArea::vertical().id_salt("trace_history").show(ui, |ui| {
                for (i, step) in journal.steps.iter().enumerate().rev() {
                    let selected = app.trace_ui.selected == Some(i);
                    if ui.selectable_label(selected, format!("{:03}  {}", i + 1, crate::i18n::t(&step.label))).clicked() {
                        app.trace_ui.selected = if selected { None } else { Some(i) };
                    }
                    ui.small(format!("{} · {} changes", step.command, step.changes.len()));
                    if app.trace_ui.selected == Some(i) {
                        let details = serde_json::to_string_pretty(&step.changes).unwrap_or_default();
                        let preview: String = details.chars().take(5000).collect();
                        ui.add(egui::Label::new(RichText::new(preview).monospace().size(10.0)).wrap());
                        if details.chars().count() > 5000 {
                            ui.small(tr("Preview shortened; the export contains the full data."));
                        }
                    }
                    ui.separator();
                }
                if journal.steps.is_empty() {
                    ui.label(tr("Start typing, format text, insert a table or a picture. Your history will appear here."));
                }
            });
        }
    });
    if app.trace_ui.show_code {
        egui::Window::new(tr("Generated Python replay")).open(&mut app.trace_ui.show_code).default_size([800.0, 600.0]).show(ui.ctx(), |ui| {
            ui.label(tr("transform(input_path, output_path=None) → output path"));
            if ui.button(tr("Copy complete code")).clicked() {
                ui.ctx().copy_text(app.trace_ui.code.clone());
            }
            egui::ScrollArea::both().show(ui, |ui| {
                let mut preview: String = app.trace_ui.code.chars().take(100_000).collect();
                ui.add(egui::TextEdit::multiline(&mut preview).code_editor().desired_width(f32::INFINITY).interactive(false));
            });
        });
    }
    if app.trace_ui.show_report {
        egui::Window::new(tr("OfficeCLI results")).open(&mut app.trace_ui.show_report).default_size([720.0, 500.0]).show(ui.ctx(), |ui| {
            ui.label(tr("OfficeCLI results for a snapshot of the document."));
            egui::ScrollArea::both().show(ui, |ui| {
                ui.monospace(&app.trace_ui.report);
            });
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn history_root() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("WORDCRAFT_TRACE_HOME") {
        return path.into();
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(|| ".".into());
    if cfg!(target_os = "macos") { home.join("Library/Application Support/WordCraftTrace/History") } else { home.join(".wordcraft-trace/history") }
}

impl WordApp {
    /// Flush every recorded document, including documents closed since the last timer tick.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn flush_trace_history(&mut self) -> Result<(), String> {
        self.session.trace_sync("document.change", "Document update");
        for (id, journal) in &self.session.trace.journals {
            if journal.steps.is_empty() || self.trace_ui.checkpoints.get(id).is_some_and(|(n, _)| *n == journal.steps.len()) {
                continue;
            }
            let folder = self.trace_ui.checkpoints.get(id).map(|(_, p)| p.clone()).unwrap_or_else(|| {
                let stamp = wordcraft_engine::cmd::now_iso().replace([':', '-'], "");
                history_root().join(format!("{stamp}-{}-{id}", std::process::id()))
            });
            journal.checkpoint(&folder)?;
            self.trace_ui.checkpoints.insert(*id, (journal.steps.len(), folder));
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn tick(app: &mut WordApp, ctx: &egui::Context) {
    if let Some(receiver) = &app.trace_ui.inspection {
        match receiver.try_recv() {
            Ok((document, result)) => {
                app.trace_ui.inspection = None;
                app.trace_ui.report = match result {
                    Ok(result) => {
                        if let Some(doc) = result.document {
                            if document != app.session.document_id() {
                                "The active document changed. Return to it and run the batch again.".into()
                            } else {
                                match app.run("officecli.apply", json!({"expected":result.expected,"document":doc})) {
                                    Ok(_) => format!("Applied and recorded. Undo is available.\n\n{}", result.report),
                                    Err(e) => format!("Batch result was not applied: {e}\n\n{}", result.report),
                                }
                            }
                        } else {
                            result.report
                        }
                    }
                    Err(e) => e,
                };
                app.trace_ui.show_report = true;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                app.trace_ui.inspection = None;
                app.trace_ui.message = "OfficeCLI check stopped unexpectedly".into();
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
    }
    let now = crate::now_ms();
    ctx.request_repaint_after(std::time::Duration::from_secs(5));
    if now - app.trace_ui.checkpoint_at < 5000.0 {
        return;
    }
    app.trace_ui.checkpoint_at = now;
    let id = app.session.document_id();
    if let Some(journal) = app.session.trace_current() {
        if journal.steps.is_empty() {
            return;
        }
        if app.trace_ui.checkpoints.get(&id).is_some_and(|(n, _)| *n == journal.steps.len()) {
            return;
        }
        let folder = app.trace_ui.checkpoints.get(&id).map(|(_, p)| p.clone()).unwrap_or_else(|| {
            let stamp = wordcraft_engine::cmd::now_iso().replace([':', '-'], "");
            history_root().join(format!("{stamp}-{}-{id}", std::process::id()))
        });
        match journal.checkpoint(&folder) {
            Ok(()) => {
                app.trace_ui.checkpoints.insert(id, (journal.steps.len(), folder));
            }
            Err(e) => {
                app.trace_ui.message = format!("History autosave failed: {e}. Use Export to save it.");
            }
        }
    }
    ctx.request_repaint_after(std::time::Duration::from_secs(5));
}

#[cfg(not(target_arch = "wasm32"))]
fn inspect(app: &mut WordApp, mode: &str) {
    if let Err(e) = crate::panes::flush_comment_drafts(app) {
        app.trace_ui.message = e.to_string();
        return;
    }
    let doc = app.session.doc.clone();
    let document = app.session.document_id();
    let mode = mode.to_string();
    let batch = app.trace_ui.batch.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    app.trace_ui.inspection = Some(rx);
    let task = move || {
        let result = wordcraft_engine::officecli::execute(&doc, &mode, &batch);
        let _ = tx.send((document, result));
    };
    if let Err(e) = std::thread::Builder::new().name("officecli-inspection".into()).spawn(task) {
        app.trace_ui.inspection = None;
        app.trace_ui.message = format!("Cannot start OfficeCLI inspection: {e}");
    }
}
