//! Developer and Scribe panes; edits stay in a preview until explicitly applied.
use std::collections::{HashMap, HashSet};

use nanshe_engine::cmd::developer::{MAX_SOURCE, MAX_STEPS};
use serde_json::{Value as Json, json};

use crate::{SheetApp, code_editor::Lang};

#[cfg(not(target_arch = "wasm32"))]
mod runtime;
mod scribe;

const SAMPLE: &str = "Sub Main()\n    Range(\"A1\").Value = \"Hello from CellTrellis\"\n    Range(\"A1\").Font.Bold = True\nEnd Sub\n";

#[derive(Default)]
pub struct State {
    drafts: HashMap<u64, Draft>,
    memory: Option<nanshe_enki::memory::Memory>,
    memory_notes: String,
    memory_status: String,
    #[cfg(not(target_arch = "wasm32"))]
    job: Option<Job>,
}

struct Draft {
    source: String,
    language: String,
    sources: HashMap<String, String>,
    dirty_sources: HashSet<String>,
    macro_name: String,
    reviewed: bool,
    status: String,
    preview: Option<Json>,
    prompt: String,
    mode: String,
    reply: String,
    suggestion: Option<Json>,
    analysis_setup: Option<(String, Json)>,
    models: Vec<String>,
    model: String,
}

impl Draft {
    fn new(source: String) -> Self {
        Self {
            source: if source.is_empty() { SAMPLE.into() } else { source },
            language: "VBA".into(),
            sources: HashMap::new(),
            dirty_sources: HashSet::new(),
            macro_name: "Main".into(),
            reviewed: false,
            status: String::new(),
            preview: None,
            prompt: String::new(),
            mode: "Chat and edit".into(),
            reply: String::new(),
            suggestion: None,
            analysis_setup: None,
            models: vec![],
            model: String::new(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct Job {
    doc: u64,
    rx: std::sync::mpsc::Receiver<Event>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(not(target_arch = "wasm32"))]
enum Event {
    Chunk(String),
    Reply(String),
    Memory(nanshe_enki::memory::Memory),
    MemoryStatus(String),
    Models(Vec<String>),
    Macro(Json),
    Suggestion(Json),
    Done(Result<(), String>),
}

/// UI commands share the same entry points as ribbon buttons and agent/control calls.
pub(crate) fn command(app: &mut SheetApp, id: &str, p: &Json) -> Result<Json, String> {
    let uid = app.session.doc().map_err(|e| e.to_string())?.uid;
    if id == "ui.developerStatus" {
        let draft = app.developer.drafts.get(&uid);
        #[cfg(not(target_arch = "wasm32"))]
        let busy = app.developer.job.as_ref().is_some_and(|job| job.doc == uid);
        #[cfg(target_arch = "wasm32")]
        let busy = false;
        return Ok(
            json!({"busy":busy,"status":draft.map(|d| &d.status),"preview":draft.and_then(|d| d.preview.as_ref()),"reply":draft.map(|d| answer(&d.reply)),"suggestion":draft.and_then(|d| d.suggestion.as_ref()),"memory":app.developer.memory,"memoryStatus":app.developer.memory_status}),
        );
    }
    if id == "ui.developerStop" {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(job) = &app.developer.job {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        return Ok(Json::Null);
    }
    if id == "ui.scribeReturnAnalysis" {
        let uid = app.session.doc().map_err(|e| e.to_string())?.uid;
        let (dialog, setup) = app.developer.drafts.get(&uid).and_then(|draft| draft.analysis_setup.clone()).ok_or("No analysis tool to return to")?;
        restore_analysis(app, &dialog, &setup);
        return Ok(Json::Null);
    }
    if id == "ui.scribeAnalysis" {
        let tool = p["tool"].as_str().ok_or("Analysis tool is required")?;
        let instruction = match tool {
            "whatIf" => {
                "Help me use What-If Analysis: choose between Goal Seek, Scenario Manager and Data Table, identify the input and result cells, and explain the assumptions."
            }
            "forecast" => {
                "Help me configure Forecast Sheet: identify the timeline and values columns, suggest a sensible forecast horizon, and explain seasonality, missing points and confidence intervals."
            }
            "analysis" => {
                "Help me choose a Data Analysis tool for this selection and explain its input range, labels, assumptions and output options."
            }
            _ => return Err("Unknown analysis tool".into()),
        };
        let setup = p.get("setup").cloned().unwrap_or(Json::Null);
        if setup.to_string().len() > 8192 {
            return Err("Analysis settings exceed 8 KiB".into());
        }
        let uid = app.session.doc().map_err(|e| e.to_string())?.uid;
        let draft = app.developer.drafts.entry(uid).or_insert_with(|| Draft::new(String::new()));
        if !draft.prompt.trim().is_empty() {
            return Err("Send or clear the current Scribe request before starting analysis help".into());
        }
        draft.prompt =
            format!("{instruction} Use the selected cells. Ask me for missing information and explain how to configure the built-in tool.");
        let dialog = match tool {
            "forecast" => "forecastSheet",
            "analysis" => "analysisToolPak",
            _ => "goalSeek",
        };
        let dialog = p["dialog"]
            .as_str()
            .filter(|name| matches!(*name, "forecastSheet" | "analysisToolPak" | "goalSeek" | "dataTable" | "scenarioManager"))
            .unwrap_or(dialog);
        draft.analysis_setup = Some((dialog.into(), setup));
        draft.mode = "Explain selection".into();
        app.grid.pane = Some("scribe".into());
        return Ok(Json::Null);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if id.starts_with("ui.scribeMemory") {
            return start_memory(app, id, p).map(|_| Json::Null);
        }
        let mut request = Draft::new(p.get("source").and_then(Json::as_str).unwrap_or("").into());
        request.analysis_setup = app.developer.drafts.get(&uid).and_then(|draft| draft.analysis_setup.clone());
        if let Some(setup) = p.get("analysisSetup") {
            if setup.to_string().len() > 8192 {
                return Err("Analysis settings exceed 8 KiB".into());
            }
            request.analysis_setup = serde_json::from_value(setup.clone()).map_err(|_| "Invalid analysis settings")?;
        }
        match id {
            "ui.developerPreview" => {
                let source = p.get("source").and_then(Json::as_str).ok_or("Script source is required")?;
                if source.trim().is_empty() || source.len() > MAX_SOURCE {
                    return Err("Source must contain code and fit 128 KiB".into());
                }
                request.source = source.into();
                if p.get("trusted").and_then(Json::as_bool) != Some(true) {
                    return Err("Review the script and explicitly trust it before running".into());
                }
                request.language = p.get("language").and_then(Json::as_str).unwrap_or("VBA").into();
                if !["VBA", "Python", "R"].contains(&request.language.as_str()) {
                    return Err("Choose VBA, Python or R".into());
                }
                request.macro_name = p.get("macro").and_then(Json::as_str).unwrap_or("Main").into();
                start_macro(app, &request)?;
                let draft = app.developer.drafts.entry(uid).or_insert_with(|| Draft::new(String::new()));
                draft.preview = None;
            }
            "ui.scribeModels" | "ui.scribeAsk" => {
                request.prompt = p.get("prompt").and_then(Json::as_str).unwrap_or("").into();
                request.model = p.get("model").and_then(Json::as_str).unwrap_or("").into();
                request.mode = p.get("mode").and_then(Json::as_str).unwrap_or("Chat and edit").into();
                if request.prompt.len() > 32 * 1024
                    || !["Chat and edit", "Formula", "Explain selection", "Write VBA", "Write Python", "Write R"].contains(&request.mode.as_str())
                {
                    return Err("Invalid Scribe request".into());
                }
                if id == "ui.scribeAsk" && request.prompt.trim().is_empty() {
                    return Err("Enter a request".into());
                }
                start_scribe(app, &request, id == "ui.scribeModels")?;
                let draft = app.developer.drafts.entry(uid).or_insert_with(|| Draft::new(String::new()));
                if id == "ui.scribeAsk" {
                    draft.reply.clear();
                    draft.suggestion = None;
                }
            }
            _ => return Err("Unknown Developer command".into()),
        }
        Ok(Json::Null)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = p;
        Err("Local runtimes require the desktop app".into())
    }
}

/// Join script drafts to the existing Save/Close path, so closing a workbook cannot silently
/// discard code the editor has not yet explicitly saved.
pub(crate) fn commit_sources(app: &mut SheetApp) -> Result<(), String> {
    let Some(uid) = app.session.active().map(|d| d.uid) else { return Ok(()) };
    let Some(draft) = app.developer.drafts.get_mut(&uid) else { return Ok(()) };
    let pending: Vec<String> = draft.dirty_sources.iter().cloned().collect();
    for language in pending {
        let source = if language == draft.language { &draft.source } else { draft.sources.get(&language).ok_or("Missing script draft")? };
        app.session.execute("developer.saveSource", json!({"language":language.to_ascii_lowercase(),"source":source})).map_err(|e| e.to_string())?;
        draft.dirty_sources.remove(&language);
    }
    Ok(())
}

pub(crate) fn commit_all_sources(app: &mut SheetApp) -> Result<(), String> {
    let active = app.session.active_window();
    let pending: Vec<u64> = app.developer.drafts.iter().filter(|(_, d)| !d.dirty_sources.is_empty()).map(|(uid, _)| *uid).collect();
    let result = (|| {
        for uid in pending {
            if let Some(index) = app.session.document_index(uid) {
                app.session.set_active(index);
                commit_sources(app)?;
            }
        }
        Ok(())
    })();
    if let Some(window) = active {
        app.session.activate_window(window);
    }
    result
}

pub fn tick(app: &mut SheetApp, ctx: &egui::Context) {
    let open: Vec<u64> = app.session.documents().iter().map(|d| d.uid).collect();
    app.developer.drafts.retain(|uid, _| open.contains(uid));
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut finished = false;
        if let Some(job) = &app.developer.job {
            if !open.contains(&job.doc) {
                finished = true;
            }
            for _ in 0..64 {
                match job.rx.try_recv() {
                    Ok(event) => {
                        if let Some(draft) = app.developer.drafts.get_mut(&job.doc) {
                            match event {
                                Event::Chunk(text) => draft.reply.push_str(&text),
                                Event::Reply(text) => draft.reply = text,
                                Event::Memory(memory) => {
                                    app.developer.memory_notes = memory.preferences.clone();
                                    app.developer.memory = Some(memory);
                                }
                                Event::MemoryStatus(status) => app.developer.memory_status = status,
                                Event::Models(models) => {
                                    if !models.contains(&draft.model) {
                                        draft.model = models
                                            .iter()
                                            .find(|model| {
                                                let name = model.to_ascii_lowercase();
                                                ["instruct", "chat"].iter().any(|word| name.contains(word))
                                                    && !["embed", "rerank", "vision", "mmproj"].iter().any(|word| name.contains(word))
                                            })
                                            .or_else(|| models.first())
                                            .cloned()
                                            .unwrap_or_default();
                                    }
                                    draft.models = models;
                                }
                                Event::Macro(preview) => {
                                    if !job.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                                        draft.preview = Some(preview);
                                    }
                                }
                                Event::Suggestion(p) => {
                                    if !job.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                                        draft.suggestion = Some(p);
                                    }
                                }
                                Event::Done(result) => {
                                    draft.status = if job.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                                        "Stopped; no changes prepared".into()
                                    } else {
                                        result.err().unwrap_or_else(|| "Ready for review".into())
                                    };
                                    finished = true;
                                }
                            }
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if finished {
            app.developer.job = None;
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = ctx;
}

pub fn show(app: &mut SheetApp, ui: &mut egui::Ui, scribe: bool) {
    let Some(uid) = app.session.active().map(|d| d.uid) else { return };
    let mut draft = match app.developer.drafts.remove(&uid) {
        Some(draft) => draft,
        None => {
            let source = app
                .session
                .run("developer.source", json!({}))
                .ok()
                .and_then(|v| v.get("source").and_then(Json::as_str).map(str::to_string))
                .unwrap_or_default();
            Draft::new(source)
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let busy = app.developer.job.is_some();
    #[cfg(target_arch = "wasm32")]
    let busy = false;
    if scribe {
        scribe_ui(app, ui, &mut draft, busy);
    } else {
        macro_ui(app, ui, &mut draft, busy);
    }
    if busy {
        ui.spinner();
        if ui.button("Stop").clicked() {
            let _ = app.run("ui.developerStop", json!({}));
            draft.status = "Stopping…".into();
        }
    }
    ui.separator();
    ui.label(&draft.status);
    app.developer.drafts.insert(uid, draft);
}

fn macro_ui(app: &mut SheetApp, ui: &mut egui::Ui, draft: &mut Draft, busy: bool) {
    let previous_language = draft.language.clone();
    ui.add_enabled_ui(!busy, |ui| {
        craft_ui::ComboBox::from_id_salt("developer_language")
            .selected_text(&draft.language)
            .show_ui(ui, |ui| {
                for language in ["VBA", "Python", "R"] {
                    ui.selectable_value(&mut draft.language, language.into(), language);
                }
            })
            .response
            .accessible_name("Script language");
    });
    if previous_language != draft.language {
        draft.sources.insert(previous_language, draft.source.clone());
        draft.source = match draft.sources.get(&draft.language) {
            Some(source) => source.clone(),
            None => {
                let saved = app
                    .session
                    .run("developer.source", json!({"language":draft.language.to_ascii_lowercase()}))
                    .ok()
                    .and_then(|v| v.get("source").and_then(Json::as_str).map(str::to_string))
                    .unwrap_or_default();
                if saved.is_empty() { sample(&draft.language).into() } else { saved }
            }
        };
        draft.reviewed = false;
        draft.preview = None;
    }
    ui.label(format!("{} · active workbook", draft.language));
    ui.weak("Edit a script, preview its changes, then apply them together.");
    ui.weak("Scripts run as your user. File and network access are outside the workbook preview.");
    let changed = ui
        .add_enabled_ui(!busy, |ui| {
            crate::widgets::code_editor(
                ui,
                egui::Id::unique("nanshe-vba"),
                &mut draft.source,
                if draft.language == "Python" { Lang::Python } else { Lang::Plain },
                16,
            )
            .changed()
        })
        .inner;
    if changed {
        draft.dirty_sources.insert(draft.language.clone());
        draft.reviewed = false;
        draft.preview = None;
    }
    if draft.language == "VBA" {
        ui.horizontal(|ui| {
            ui.label("Macro");
            ui.add_enabled(!busy, egui::TextEdit::singleline(&mut draft.macro_name).desired_width(130.0).char_limit(128))
                .accessible_name("Macro name");
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    let missing_runtime = runtime::missing_runtime(&draft.language);
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(message) = &missing_runtime {
        ui.colored_label(ui.visuals().warn_fg_color, message);
    }
    ui.add_enabled(!busy, egui::Checkbox::new(&mut draft.reviewed, "I reviewed this code and trust it to run as my user"));
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(!busy && draft.source.len() <= MAX_SOURCE, egui::Button::new("Save in workbook")).clicked() {
            draft.status = match app.run("developer.saveSource", json!({"source":draft.source,"language":draft.language.to_ascii_lowercase()})) {
                Ok(_) => {
                    draft.dirty_sources.remove(&draft.language);
                    "Source saved; save the workbook to keep it on disk".into()
                }
                Err(e) => e,
            };
        }
        #[cfg(not(target_arch = "wasm32"))]
        if ui
            .add_enabled(!busy && missing_runtime.is_none() && draft.reviewed && draft.source.len() <= MAX_SOURCE, egui::Button::new("Run preview"))
            .clicked()
        {
            draft.preview = None;
            match app.run(
                "ui.developerPreview",
                json!({"source":draft.source,"language":draft.language,"macro":draft.macro_name,"trusted":draft.reviewed}),
            ) {
                Ok(_) => draft.status = "Running preview…".into(),
                Err(e) => draft.status = e,
            }
        }
    });
    #[cfg(target_arch = "wasm32")]
    ui.weak("Script execution requires the desktop app and the selected language runtime.");
    if let Some(preview) = draft.preview.clone() {
        let commands = preview.get("commands").and_then(Json::as_array).cloned().unwrap_or_default();
        ui.label(format!("{} engine calls in preview", commands.len()));
        egui::CollapsingHeader::new("Review changes and reads").show(ui, |ui| {
            for step in commands.iter().take(MAX_STEPS) {
                ui.monospace(step.to_string());
            }
        });
        if let Some(output) = preview.get("output").and_then(Json::as_str) {
            ui.monospace(output);
        }
        if ui.add_enabled(!busy, egui::Button::new("Apply script changes")).clicked() {
            draft.status = match app.run("developer.apply", preview) {
                Ok(_) => "Applied · Undo reverses the macro".into(),
                Err(e) => e,
            };
            draft.preview = None;
        }
    }
}

fn restore_analysis(app: &mut SheetApp, dialog: &str, setup: &Json) {
    app.open_dialog(dialog, Json::Null);
    if let Some(d) = &mut app.dialog
        && let Some(values) = setup.as_object()
    {
        d.values.extend(values.clone());
    }
}

fn scribe_ui(app: &mut SheetApp, ui: &mut egui::Ui, draft: &mut Draft, busy: bool) {
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(32.0, 40.0), egui::Sense::hover());
        crate::icons::paint(ui.painter(), rect, crate::icons::Icon::Scribe, ui.visuals().text_color());
        ui.vertical(|ui| {
            ui.label("Scribe · CellTrellis");
            ui.weak("Ask a question, highlight cells, create formulas, or draft a macro. Review changes before applying them.");
        });
    });
    #[cfg(not(any(unix, windows)))]
    ui.weak("Local Enki requires the desktop app.");
    if let Some((dialog, setup)) = &draft.analysis_setup
        && ui.button("Return to analysis tool…").clicked()
    {
        restore_analysis(app, dialog, setup);
    }
    ui.add_enabled_ui(!busy, |ui| {
        craft_ui::ComboBox::from_id_salt("scribe_mode")
            .selected_text(&draft.mode)
            .show_ui(ui, |ui| {
                for mode in ["Chat and edit", "Formula", "Explain selection", "Write VBA", "Write Python", "Write R"] {
                    ui.selectable_value(&mut draft.mode, mode.into(), mode);
                }
            })
            .response
            .accessible_name("Scribe task");
        ui.add(
            egui::TextEdit::multiline(&mut draft.prompt)
                .hint_text("Describe what you need…")
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .char_limit(8000),
        )
        .accessible_name("Scribe request");
    });
    #[cfg(not(target_arch = "wasm32"))]
    ui.horizontal(|ui| {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if ui.add_enabled(!busy && cfg!(any(unix, windows)) && !draft.prompt.trim().is_empty(), egui::Button::new("Send")).clicked() {
                draft.reply.clear();
                draft.suggestion = None;
                match app.run("ui.scribeAsk", json!({"prompt":draft.prompt,"mode":draft.mode,"analysisSetup":draft.analysis_setup})) {
                    Ok(_) => draft.status = "Scribe is writing…".into(),
                    Err(e) => draft.status = e,
                }
            }
        }
    });
    let text = answer(&draft.reply);
    ui.add(egui::TextEdit::multiline(&mut text.as_str()).interactive(false).desired_width(f32::INFINITY).desired_rows(8))
        .accessible_name("Scribe response");
    if !busy && !text.is_empty() && ui.button("Copy response").clicked() {
        ui.ctx().copy_text(text.clone());
    }
    if let Some(proposal) = draft.suggestion.clone() {
        if let Some(commands) = proposal.get("commands").and_then(Json::as_array) {
            ui.label("Proposed worksheet changes");
            for command in commands {
                ui.label(scribe::action_label(command));
            }
        }
        if proposal.get("commands").is_some() && ui.add_enabled(!busy, egui::Button::new("Apply worksheet changes")).clicked() {
            draft.status = match app.run("developer.apply", proposal) {
                Ok(_) => "Changes applied · Undo is available".into(),
                Err(e) => e,
            };
            draft.suggestion = None;
        } else if proposal.get("source").is_some() && ui.add_enabled(!busy, egui::Button::new("Open draft in Developer")).clicked() {
            draft.sources.insert(draft.language.clone(), draft.source.clone());
            draft.language = proposal.get("language").and_then(Json::as_str).unwrap_or("VBA").into();
            draft.source = proposal.get("source").and_then(Json::as_str).unwrap_or("").into();
            draft.dirty_sources.insert(draft.language.clone());
            draft.reviewed = false;
            draft.preview = None;
            app.grid.pane = Some("developer".into());
            app.ui.ribbon_tab = "Developer".into();
        }
    }
    memory_ui(app, ui, draft, busy);
}

fn memory_ui(app: &mut SheetApp, ui: &mut egui::Ui, draft: &mut Draft, busy: bool) {
    egui::CollapsingHeader::new("Conversation and shared memory").show(ui, |ui| {
        ui.weak("Saved locally by Enki and shared with Scribe in your other apps. Compact keeps a summary; pinned preferences are kept separately.");
        if let Some(memory) = &app.developer.memory {
            if !memory.summary.is_empty() {
                ui.label("Conversation summary");
                ui.label(&memory.summary);
            }
            egui::ScrollArea::vertical().id_salt("scribe_history").max_height(160.0).show(ui, |ui| {
                for turn in &memory.turns {
                    ui.strong(format!("You · {}", turn.app));
                    ui.label(&turn.user);
                    ui.strong("Scribe");
                    ui.label(answer(&turn.assistant));
                    ui.separator();
                }
            });
        }
        ui.label("Preferences and style to remember");
        ui.add_enabled(
            !busy,
            egui::TextEdit::multiline(&mut app.developer.memory_notes).desired_rows(3).desired_width(f32::INFINITY).char_limit(2000),
        )
        .accessible_name("Shared Scribe preferences");
        #[cfg(not(target_arch = "wasm32"))]
        ui.horizontal_wrapped(|ui| {
            for (label, id) in [
                ("Refresh memory", "ui.scribeMemoryRefresh"),
                ("Save preferences", "ui.scribeMemorySave"),
                ("Compact conversation", "ui.scribeMemoryCompact"),
                ("Clear conversation across apps", "ui.scribeMemoryClear"),
            ] {
                let has_memory = app.developer.memory.is_some();
                if ui.add_enabled(!busy && (has_memory || id == "ui.scribeMemoryRefresh"), egui::Button::new(label)).clicked() {
                    let p = json!({"preferences":app.developer.memory_notes});
                    match app.run(id, p) {
                        Ok(_) => app.developer.memory_status = "Updating shared memory…".into(),
                        Err(e) => app.developer.memory_status = e,
                    }
                    if id == "ui.scribeMemoryClear" {
                        draft.reply.clear();
                        draft.suggestion = None;
                    }
                }
            }
        });
        ui.label(&app.developer.memory_status);
    });
}

fn visible(raw: &str) -> (String, bool) {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    let mut rest = raw;
    // Thinking opened by the template: only a closing tag, with no opening tag before it.
    if let Some(c) = rest.find(CLOSE)
        && rest.get(..c).is_some_and(|before| !before.contains(OPEN))
    {
        rest = rest.get(c + CLOSE.len()..).unwrap_or("");
    }
    let mut out = String::new();
    let mut thinking = false;
    loop {
        match rest.find(OPEN) {
            None => {
                out.push_str(rest);
                break;
            }
            Some(o) => {
                out.push_str(rest.get(..o).unwrap_or(""));
                let after = rest.get(o + OPEN.len()..).unwrap_or("");
                match after.find(CLOSE) {
                    Some(c) => rest = after.get(c + CLOSE.len()..).unwrap_or(""),
                    None => {
                        thinking = true;
                        break;
                    }
                }
            }
        }
    }
    // A tag still arriving ("<thi") isn't shown.
    if !thinking
        && let Some(lt) = out.rfind('<')
        && out.get(lt..).is_some_and(|tail| tail.len() > 1 && (OPEN.starts_with(tail) || CLOSE.starts_with(tail)))
    {
        out.truncate(lt);
    }
    (out.trim_start().to_string(), thinking)
}

/// Remove reasoning and a single Markdown fence before displaying or applying a suggestion.
fn answer(raw: &str) -> String {
    let (out, _) = visible(raw);
    let out = out.lines().map(|line| if line.trim().eq_ignore_ascii_case("```excel") { "```formula" } else { line }).collect::<Vec<_>>().join("\n");
    let out = out.trim();
    if out.starts_with("```")
        && let Some((_, content)) = out.split_once('\n')
        && let Some(content) = content.trim_end().strip_suffix("```")
        && !content.contains("```")
    {
        return content.trim().into();
    }
    out.into()
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn(
    app: &mut SheetApp,
    work: impl FnOnce(std::sync::mpsc::SyncSender<Event>, std::sync::Arc<std::sync::atomic::AtomicBool>) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    if app.developer.job.is_some() {
        return Err("Wait for the current job or stop it".into());
    }
    let doc = app.session.doc().map_err(|e| e.to_string())?.uid;
    let (tx, rx) = std::sync::mpsc::sync_channel(64);
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = cancel.clone();
    std::thread::Builder::new()
        .name("nanshe-developer".into())
        .spawn(move || {
            let result = work(tx.clone(), stopped);
            let _ = tx.send(Event::Done(result));
        })
        .map_err(|e| e.to_string())?;
    app.developer.job = Some(Job { doc, rx, cancel });
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn start_macro(app: &mut SheetApp, draft: &Draft) -> Result<(), String> {
    let runner = runtime::program("NANSHE_VBA_RUNNER", "vba/Nanshe.Vba.exe", if cfg!(windows) { "Nanshe.Vba.exe" } else { "Nanshe.Vba" });
    let doc = app.session.doc().map_err(|e| e.to_string())?.clone();
    let (source, name, language) = (draft.source.clone(), draft.macro_name.clone(), draft.language.clone());
    spawn(app, move |tx, cancel| {
        let preview = if language == "VBA" {
            runtime::macro_preview(&runner, &source, &name, doc, cancel)?
        } else {
            runtime::script_preview(&language, &source, doc, cancel)?
        };
        tx.send(Event::Macro(preview)).map_err(|e| e.to_string())
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn start_memory(app: &mut SheetApp, id: &str, p: &Json) -> Result<(), String> {
    use nanshe_enki as enki;
    let endpoint = enki::endpoint("").map_err(|e| e.to_string())?;
    let id = id.to_string();
    let preferences = p["preferences"].as_str().unwrap_or("").to_string();
    if preferences.len() > 8192 {
        return Err("Preferences exceed 8 KiB".into());
    }
    let cached = app.developer.memory.clone();
    spawn(app, move |tx, cancel| {
        enki::ensure_running(&endpoint, None, &[]).map_err(|e| e.to_string())?;
        let result = match id.as_str() {
            "ui.scribeMemoryRefresh" => enki::memory::get(&endpoint, "nanshe"),
            "ui.scribeMemorySave" => {
                let mut memory = cached.ok_or("Refresh shared memory before saving preferences")?;
                memory.preferences = preferences;
                enki::memory::update(&endpoint, "nanshe", &memory)
            }
            "ui.scribeMemoryCompact" => {
                let model = enki::select_text_model(&enki::list_models(&endpoint, "nanshe").map_err(|e| e.to_string())?, "writing")
                    .ok_or("The local assistant is not ready. Configure text generation in Enki and retry.")?;
                let memory = enki::memory::get(&endpoint, "nanshe").map_err(|e| e.to_string())?;
                enki::memory::compact(&endpoint, "nanshe", &model, &memory, &cancel)
            }
            "ui.scribeMemoryClear" => {
                let mut memory = enki::memory::get(&endpoint, "nanshe").map_err(|e| e.to_string())?;
                memory.summary.clear();
                memory.turns.clear();
                enki::memory::update(&endpoint, "nanshe", &memory)
            }
            _ => return Err("Unknown shared memory action".into()),
        }
        .map_err(|e| format!("Shared memory: {e}. Enki protocol v3 is required."))?;
        tx.send(Event::Memory(result)).map_err(|e| e.to_string())?;
        tx.send(Event::MemoryStatus("Shared memory updated".into())).map_err(|e| e.to_string())
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn start_scribe(app: &mut SheetApp, draft: &Draft, list: bool) -> Result<(), String> {
    use nanshe_enki as enki;
    let endpoint = enki::endpoint("").map_err(|e| e.to_string())?;
    let doc = app.session.doc().map_err(|e| e.to_string())?.clone();
    let (uid, revision, sheet, cell) = (doc.uid, doc.revision, doc.wb.active_sheet(), doc.selection.active.a1());
    let context = if list { Json::Null } else { scribe::context(&doc)? };
    let (model, prompt, mode) = (draft.model.clone(), draft.prompt.clone(), draft.mode.clone());
    let analysis_setup = draft.analysis_setup.clone();
    spawn(app, move |tx, cancel| {
        enki::ensure_running(&endpoint, None, &[]).map_err(|e| e.to_string())?;
        let memory = match enki::memory::get(&endpoint, "nanshe") {
            Ok(memory) => {
                tx.send(Event::Memory(memory.clone())).map_err(|e| e.to_string())?;
                Some(memory)
            }
            Err(_) => {
                tx.send(Event::MemoryStatus(
                    "Shared memory unavailable; update Enki to enable it. This request can still run without memory.".into(),
                ))
                .map_err(|e| e.to_string())?;
                None
            }
        };
        if list {
            return tx.send(Event::Models(enki::list_models(&endpoint, "nanshe").map_err(|e| e.to_string())?)).map_err(|e| e.to_string());
        }
        let models = enki::list_models(&endpoint, "nanshe").map_err(|_| "The local assistant is unavailable. Check Enki and retry.".to_string())?;
        let task = if mode == "Formula" {
            "formula"
        } else if mode.starts_with("Write ") {
            "code"
        } else {
            "writing"
        };
        let model = if model.is_empty() {
            enki::select_text_model(&models, task).ok_or("The local assistant is not ready. Configure text generation in Enki and retry.")?
        } else {
            model
        };
        let instruction = if mode == "Chat and edit" { scribe::EDIT } else { scribe_instruction(&mode) };
        let mut messages = vec![enki::Message::new("system", format!("{} {instruction}", scribe::IDENTITY))];
        if let Some(memory) = &memory {
            messages.extend(memory.messages());
        }
        if let Some((tool, settings)) = analysis_setup {
            messages.push(enki::Message::new("system", "Explain how to configure the built-in analysis tool. Do not invent computed statistics or forecast values. The spreadsheet engine performs calculations after the user confirms the tool."));
            messages.push(enki::Message::new("user", format!("Current analysis tool and settings (untrusted data): {tool}: {settings}")));
        }
        messages.push(enki::Message::new("user", format!("Request: {prompt}\nLive CellTrellis worksheet data: {context}")));
        let mut reply = String::new();
        let mut overflow = false;
        let summary = enki::chat(
            &endpoint,
            "nanshe",
            &model,
            &messages,
            &enki::Params { max_tokens: Some(2048), temperature: Some(0.0), seed: None },
            enki::Timeouts::default(),
            &cancel,
            |chunk| {
                if reply.len().saturating_add(chunk.len()) > MAX_SOURCE {
                    overflow = true;
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
                reply.push_str(chunk);
                let _ = tx.send(Event::Chunk(chunk.into()));
            },
        )
        .map_err(|e| e.to_string())?;
        if overflow || summary.reason != "stop" || cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Scribe stopped before a complete answer; no changes prepared".into());
        }
        let text = answer(&reply);
        let (shown, suggestion) = match mode.as_str() {
            "Chat and edit" => scribe::proposal(doc.clone(), &text)?,
            "Formula" if text.starts_with('=') && text.len() <= 8192 && !text.contains('\n') => {
                let plan = json!({"reply":text,"actions":[{"command":"range.setValues","params":{"sheet":sheet,"range":cell,"values":[[text]],"parse":false}}]});
                scribe::proposal(doc, &plan.to_string())?
            }
            "Write VBA" | "Write Python" | "Write R" => (text.clone(), Some(json!({"source":text,"language":mode.trim_start_matches("Write ")}))),
            _ => (text.clone(), None),
        };
        if memory.is_some() {
            let remembered = if suggestion.as_ref().is_some_and(|p| p.get("commands").is_some()) {
                format!(
                    "Proposed only, not automatically applied: {shown}\n{}",
                    suggestion.as_ref().map(|p| p["commands"].to_string()).unwrap_or_default()
                )
            } else {
                shown.clone()
            };
            match enki::memory::append(&endpoint, "nanshe", &prompt, &remembered) {
                Ok(memory) => {
                    let _ = tx.send(Event::Memory(memory));
                    let _ = tx.send(Event::MemoryStatus("Conversation saved across Scribe apps".into()));
                }
                Err(e) => {
                    let _ = tx.send(Event::MemoryStatus(format!("Reply is ready but history was not saved: {e}")));
                }
            }
        }
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Stopped; no changes prepared".into());
        }
        tx.send(Event::Reply(shown)).map_err(|e| e.to_string())?;
        tx.send(Event::Suggestion(suggestion.unwrap_or(Json::Null))).map_err(|e| e.to_string())?;
        let _ = (uid, revision);
        Ok(())
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn scribe_instruction(mode: &str) -> &'static str {
    match mode {
        "Formula" => "Return only one spreadsheet formula starting with =, in canonical English syntax. No explanation.",
        "Write VBA" => {
            "Return only one VBA module with Sub Main(). Use Application, ActiveWorkbook, Worksheets, Range, Cells, Value, Formula, Font. No files, shell, network, or auto-run events."
        }
        "Write Python" => {
            "Return only Python code. Use nanshe.read(address, sheet=0) for row lists, nanshe.write(address, rows, sheet=0, formulas=False) for literal values, formulas=True for formulas. No files or network."
        }
        "Write R" => {
            "Return only R code. Use sheet_read(address, sheet=0) for row lists and sheet_write(address, matrix_or_dataframe, sheet=0, formulas=FALSE). No files or network."
        }
        _ => "Explain the selected spreadsheet data clearly and concisely.",
    }
}

fn sample(language: &str) -> &'static str {
    match language {
        "Python" => "rows = nanshe.read(\"A1:A3\")\nprint(rows)\nnanshe.write(\"B1\", [[sum(row[0] or 0 for row in rows)]])\n",
        "R" => "rows <- sheet_read(\"A1:A3\")\nprint(rows)\nsheet_write(\"B1\", matrix(sum(unlist(rows)), nrow = 1))\n",
        _ => SAMPLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn developer_and_scribe_controls_have_accessible_names() {
        for scribe in [false, true] {
            let mut session = nanshe_engine::Session::new();
            session.new_workbook();
            let app = SheetApp::new(session, Default::default());
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(520.0, 900.0))
                .with_context_setup(|ctx| {
                    ctx.set_fonts(crate::theme::pinned_font_definitions());
                    crate::theme::apply(ctx, true);
                })
                .build_ui_state(move |ui, app| show(app, ui, scribe), app);
            harness.run_steps(2);
        }
    }

    #[test]
    fn scribe_keeps_model_selection_out_of_the_pane() {
        use egui_kittest::kittest::Queryable;
        let mut session = nanshe_engine::Session::new();
        session.new_workbook();
        let mut app = SheetApp::new(session, Default::default());
        let uid = app.session.doc().unwrap().uid;
        let mut draft = Draft::new(String::new());
        draft.model = "private-model-name".into();
        draft.models = vec![draft.model.clone()];
        app.developer.drafts.insert(uid, draft);
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(520.0, 900.0)).build_ui_state(|ui, app| show(app, ui, true), app);
        h.run_steps(2);
        assert!(h.query_by_label("Send").is_some());
        for label in ["Model settings", "Scribe model", "private-model-name", "Refresh models"] {
            assert!(h.query_by_label(label).is_none(), "model control visible: {label}");
        }
    }

    #[test]
    fn analysis_help_keeps_workbook_and_tool_settings_until_confirmation() {
        for (tool, dialog, phrase) in
            [("whatIf", "goalSeek", "Goal Seek"), ("forecast", "forecastSheet", "Forecast Sheet"), ("analysis", "analysisToolPak", "Data Analysis")]
        {
            let mut session = nanshe_engine::Session::new();
            session.new_workbook();
            let mut app = SheetApp::new(session, Default::default());
            let uid = app.session.doc().unwrap().uid;
            let revision = app.session.doc().unwrap().revision;
            let setup = json!({"timeline":"A2:A12","values":"B2:B12","confidence":"90"});
            command(&mut app, "ui.scribeAnalysis", &json!({"tool":tool,"dialog":dialog,"setup":setup})).unwrap();
            assert_eq!(app.grid.pane.as_deref(), Some("scribe"));
            let draft = &app.developer.drafts[&uid];
            assert!(draft.prompt.contains(phrase));
            assert!(!draft.prompt.contains("A2:A12"), "raw tool settings belong in context, not the editable request");
            assert_eq!(draft.mode, "Explain selection");
            assert_eq!(app.session.doc().unwrap().revision, revision);
            assert!(command(&mut app, "ui.scribeAnalysis", &json!({"tool":tool})).is_err(), "must not discard an unfinished request");
            command(&mut app, "ui.scribeReturnAnalysis", &json!({})).unwrap();
            let restored = app.dialog.as_ref().unwrap();
            assert_eq!(restored.name, dialog);
            assert_eq!(restored.values.get("confidence"), Some(&json!("90")));
            assert_eq!(app.session.doc().unwrap().revision, revision);
        }
    }

    #[test]
    fn analysis_dialog_help_and_return_buttons_preserve_the_form() {
        use egui_kittest::kittest::Queryable;
        for dialog in ["forecastSheet", "analysisToolPak", "goalSeek"] {
            let mut session = nanshe_engine::Session::new();
            session.new_workbook();
            let mut app = SheetApp::new(session, Default::default());
            app.open_dialog(dialog, Json::Null);
            app.dialog.as_mut().unwrap().values.insert("confidence".into(), json!("90"));
            let mut h = egui_kittest::Harness::builder()
                .with_size(egui::vec2(1000.0, 900.0))
                .with_context_setup(|ctx| {
                    ctx.set_fonts(crate::theme::pinned_font_definitions());
                    crate::theme::apply(ctx, true);
                })
                .build_ui_state(
                    |ui, app| {
                        crate::dialogs::show(app, ui.ctx());
                        if app.grid.pane.as_deref() == Some("scribe") {
                            show(app, ui, true);
                        }
                    },
                    app,
                );
            h.run_steps(2);
            h.get_by_label("Ask Scribe for help…").click();
            h.run_steps(2);
            assert!(h.state().dialog.is_none());
            h.get_by_label("Return to analysis tool…").click();
            h.run_steps(2);
            let restored = h.state().dialog.as_ref().unwrap();
            assert_eq!(restored.name, dialog);
            assert_eq!(restored.values.get("confidence"), Some(&json!("90")));
        }
    }

    #[test]
    fn control_preview_requires_explicit_trust() {
        let mut session = nanshe_engine::Session::new();
        session.new_workbook();
        let mut app = SheetApp::new(session, Default::default());
        assert!(app.run("ui.developerPreview", json!({"language":"Python","source":"print(42)"})).is_err());
        assert!(app.run("ui.developerStatus", json!({})).unwrap()["preview"].is_null());
    }

    #[test]
    fn script_drafts_join_save_and_stay_separate_by_language() {
        let mut session = nanshe_engine::Session::new();
        session.new_workbook();
        let mut app = SheetApp::new(session, Default::default());
        let uid = app.session.doc().unwrap().uid;
        let mut draft = Draft::new("Sub Main()\nEnd Sub".into());
        draft.sources.insert("Python".into(), "print(42)".into());
        draft.dirty_sources.extend(["VBA".into(), "Python".into()]);
        app.developer.drafts.insert(uid, draft);
        commit_sources(&mut app).unwrap();
        assert_eq!(app.session.run("developer.source", json!({"language":"python"})).unwrap()["source"], "print(42)");
        assert_eq!(app.session.run("developer.source", json!({"language":"vba"})).unwrap()["source"], "Sub Main()\nEnd Sub");
        assert!(app.session.doc().unwrap().is_dirty());
        assert!(app.developer.drafts[&uid].dirty_sources.is_empty());
    }

    #[test]
    fn reasoning_is_never_applied_and_fences_are_removed() {
        assert_eq!(answer("<think>private thoughts</think>\n```excel\n=SUM(A1:A3)\n```"), "=SUM(A1:A3)");
        assert_eq!(answer("<think>unfinished"), "");
        assert_eq!(answer("template thinking</think>=1+2"), "=1+2");
        assert_eq!(answer("Привет"), "Привет");
        assert_eq!(answer("Visible<thi"), "Visible");
        #[cfg(not(target_arch = "wasm32"))]
        {
            assert!(scribe_instruction("Write Python").contains("nanshe.read"));
            assert!(scribe_instruction("Write R").contains("sheet_read"));
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "requires local Enki with the registered nanshe-smoke model"]
    #[allow(clippy::disallowed_methods)]
    fn live_enki_scribe_streams_without_changing_the_workbook() {
        fn wait(app: &mut SheetApp, context: &egui::Context) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
            while app.developer.job.is_some() {
                tick(app, context);
                assert!(std::time::Instant::now() < deadline, "Scribe did not finish");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        let mut session = nanshe_engine::Session::new();
        session.new_workbook();
        let mut app = SheetApp::new(session, Default::default());
        let uid = app.session.doc().unwrap().uid;
        let revision = app.session.doc().unwrap().revision;
        let context = egui::Context::default();
        command(&mut app, "ui.scribeModels", &json!({})).unwrap();
        wait(&mut app, &context);
        assert!(app.developer.drafts[&uid].models.contains(&"nanshe-smoke".into()));
        command(&mut app, "ui.scribeAsk", &json!({"model":"nanshe-smoke","mode":"Explain selection","prompt":"Say hello in one short sentence."}))
            .unwrap();
        wait(&mut app, &context);
        let draft = &app.developer.drafts[&uid];
        assert_eq!(draft.status, "Ready for review", "{}", draft.reply);
        assert!(!answer(&draft.reply).is_empty());
        assert!(draft.suggestion.as_ref().is_some_and(Json::is_null));
        assert_eq!(app.session.doc().unwrap().revision, revision);
        eprintln!("live Scribe reply: {}", answer(&draft.reply));
        command(&mut app, "ui.scribeAsk", &json!({"model":"nanshe-smoke","mode":"Write Python","prompt":"Write a very long script."})).unwrap();
        command(&mut app, "ui.developerStop", &json!({})).unwrap();
        wait(&mut app, &context);
        assert_eq!(app.developer.drafts[&uid].status, "Stopped; no changes prepared");
        assert!(app.developer.drafts[&uid].suggestion.is_none());
        assert_eq!(app.session.doc().unwrap().revision, revision);
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn disabled_pane_fields_keep_pending_typing_out_of_cells() {
        use egui_kittest::kittest::Queryable as _;
        for (pane, label) in [("scribe", "Scribe request"), ("developer", "Code Editor")] {
            let mut session = nanshe_engine::Session::new();
            session.new_workbook();
            let mut app = SheetApp::new(session, Default::default());
            app.use_prepared_fonts();
            app.grid.pane = Some(pane.into());
            let uid = app.session.doc().unwrap().uid;
            let revision = app.session.doc().unwrap().revision;
            let mut h = egui_kittest::Harness::builder()
                .with_size(egui::vec2(1280.0, 800.0))
                .with_os(egui::os::OperatingSystem::Windows)
                .with_context_setup(|ctx| {
                    ctx.set_fonts(crate::theme::pinned_font_definitions());
                    crate::theme::apply(ctx, true);
                })
                .build_ui_state(
                    |ui, app: &mut SheetApp| {
                        app.logic(ui.ctx());
                        app.ui(ui);
                    },
                    app,
                );
            h.run_steps(4);
            h.get_by_label(label).click();
            h.run_steps(2);
            assert!(h.ctx.text_edit_focused(), "{pane} editor must own input");
            for c in "Return exactly =1+1".chars() {
                h.input_mut().events.push(egui::Event::Text(c.to_string()));
                h.run_steps(2);
                assert!(h.state().editor.is_none(), "{pane} typing entered a cell");
            }
            let (_tx, rx) = std::sync::mpsc::sync_channel(1);
            h.state_mut().developer.job = Some(Job { doc: uid, rx, cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)) });
            h.input_mut().events.push(egui::Event::Text("=late".into()));
            h.run_steps(2);
            assert!(h.state().editor.is_none(), "{pane} disabled field leaked typing into a cell");
            assert_eq!(h.state().session.doc().unwrap().revision, revision);
            let pos = h.state().grid.rect.unwrap().center();
            h.input_mut().events.extend([
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
            ]);
            h.step();
            h.input_mut().events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
            h.run_steps(2);
            h.input_mut().events.push(egui::Event::Text("cell".into()));
            h.run_steps(2);
            assert!(h.state().editor.is_some(), "clicking the sheet must restore normal typing");
        }
    }
}
