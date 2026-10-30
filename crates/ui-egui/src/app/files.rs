use super::{Json, Notice, NoticeKind, OpenWindow, SheetApp, clean_error, json};

impl SheetApp {
    pub(crate) fn open_template(&mut self, p: &Json) -> Result<Json, String> {
        let sample = p["sample"].as_str().ok_or("Template name is required")?;
        if !nanshe_engine::sample::SAMPLES.iter().any(|(name, _)| *name == sample) {
            return Err("Unknown template".into());
        }
        self.settle_drafts()?;
        let doc = self.session.active();
        let needs_save = doc.is_some_and(|d| d.is_dirty() || (d.path.is_none() && d.wb.sheets.iter().any(|s| s.used_range().is_some())));
        if needs_save {
            let title = doc.map(|d| d.display_title()).unwrap_or_default();
            let uid = doc.map(|d| d.uid);
            self.open_dialog("saveChanges", json!({"title":title,"template":sample,"doc":uid}));
            return Ok(json!({"confirmationRequired":true}));
        }
        self.run("file.new", json!({"sample":sample,"replace":true}))
    }

    pub(crate) fn answer_template_save(&mut self, answer: crate::SaveAnswer, sample: &str, uid: u64) -> bool {
        if self.session.active().map(|d| d.uid) != Some(uid) {
            self.alert("The workbook changed. Open the template again.".into());
            return true;
        }
        match answer {
            crate::SaveAnswer::Cancel => true,
            crate::SaveAnswer::Save => {
                if let Err(e) = self.run("file.save", json!({})) {
                    self.alert(e);
                    return false;
                }
                if self.session.active().is_none_or(|d| d.uid != uid || d.is_dirty() || d.path.is_none()) {
                    return false;
                }
                match self.run("file.new", json!({"sample":sample,"replace":true})) {
                    Ok(_) => true,
                    Err(e) => {
                        self.alert(e);
                        false
                    }
                }
            }
            crate::SaveAnswer::DontSave => match self.run("file.new", json!({"sample":sample,"replace":true,"force":true})) {
                Ok(_) => true,
                Err(e) => {
                    self.alert(e);
                    false
                }
            },
        }
    }

    pub fn open_path(&mut self, path: &str) {
        match self.run("file.open", json!({"path": path})) {
            Ok(r) => self.after_open(Some(path), &r),
            // A password-protected workbook: ask for the password; the dialog opens it.
            Err(e) if e.contains(nanshe_engine::io::PASSWORD_REQUIRED) => self.open_dialog("openPassword", json!({"path": path})),
            Err(e) => self.message = Some(("CellTrellis".into(), clean_error(&e))),
        }
        self.after_engine();
    }

    /// What follows a successful `file.open`: the workbook shows (not the Backstage), the Recent
    /// list (for a file on disk) and the open warnings notice. Shared by File › Open, the
    /// password dialog and the control channel.
    pub fn after_open(&mut self, path: Option<&str>, r: &Json) {
        self.backstage = None;
        if let Some(path) = path {
            self.ui.recent.retain(|p| p != path);
            self.ui.recent.insert(0, path.to_string());
            self.ui.recent.truncate(20);
        }
        self.show_open_warnings(r);
    }

    /// Turns the `warnings` a `file.open` result carries into an info-bar notice, choosing the
    /// most important one (a preserved VBA project outranks, say, a skipped optional part).
    /// Kept deliberately generic: any open-time warning can reach the user this way.
    pub fn show_open_warnings(&mut self, r: &Json) {
        // The opened workbook's window shows them, however it was opened (a script may have run
        // `file.open` on the session directly).
        self.follow_active_window();
        let Some(list) = r.get("warnings").and_then(Json::as_array) else {
            self.notice = None;
            return;
        };
        let msgs: Vec<&str> = list.iter().filter_map(Json::as_str).collect();
        if msgs.is_empty() {
            // A clean open clears any notice left over from the previous document.
            self.notice = None;
            return;
        }
        let vba: Vec<&str> = msgs.iter().copied().filter(|m| m.to_lowercase().contains("macro") || m.contains("VBA")).collect();
        if !vba.is_empty() {
            // The reader says "dropped" when the VBA project was missing or unreadable; never promise to keep it then.
            let dropped = vba.iter().any(|m| m.contains("dropped"));
            let text = if dropped {
                "This workbook's VBA macros could not be loaded and were dropped, so they will not be in the saved file. Keep the original file."
            } else {
                "This workbook contains VBA macros. CellTrellis keeps them when you save as .xlsm, but does not run them."
            };
            self.notice = Some(Notice { text: text.into(), kind: NoticeKind::Warning, action: Some(("What this means".into(), "macros".into())) });
        } else {
            let text = if msgs.len() == 1 {
                msgs[0].to_string()
            } else {
                format!("{} items in this file were handled differently. First: {}", msgs.len(), msgs[0])
            };
            self.notice = Some(Notice { text, kind: NoticeKind::Info, action: None });
        }
    }

    /// The open windows in order, for the Switch Windows lists.
    pub fn open_windows(&self) -> Vec<OpenWindow> {
        let active = self.session.active_window();
        self.session
            .windows()
            .iter()
            .filter_map(|w| {
                let d = self.session.documents().iter().find(|d| d.uid == w.doc_uid)?;
                let title = self.session.window_title(w.id)?;
                Some(OpenWindow { window: w.id, doc: d.uid, title, dirty: d.is_dirty(), active: active == Some(w.id) })
            })
            .collect()
    }

    /// Opens files that arrived asynchronously (web file picker, dropped files).
    pub(super) fn poll_async_opens(&mut self) {
        let arrived: Vec<(String, Vec<u8>)> = self
            .services
            .inbox
            .as_ref()
            .map(|i| std::mem::take(&mut *i.lock().unwrap_or_else(std::sync::PoisonError::into_inner)))
            .unwrap_or_default();
        for (name, bytes) in arrived {
            let b64 = nanshe_engine::io::base64_encode(&bytes);
            match self.run("file.open", json!({"name": &name, "base64": &b64})) {
                // A file picked from File › Open (web) arrives here: show it, not the Backstage.
                Ok(r) => {
                    self.backstage = None;
                    self.show_open_warnings(&r);
                }
                Err(e) if e.contains(nanshe_engine::io::PASSWORD_REQUIRED) => {
                    self.backstage = None;
                    self.open_dialog("openPassword", json!({"name": name, "base64": b64}));
                }
                Err(e) => self.message = Some(("CellTrellis".into(), clean_error(&e))),
            }
            self.after_engine();
        }
    }

    /// AutoSave: save workbooks that have a file shortly after they change. This decision lives in
    /// `logic` so a hidden or logic-only frame still saves; the monotonic timestamp check keeps a
    /// forced hidden UI pass from saving the same change twice.
    pub(super) fn autosave(&mut self, ctx: &egui::Context) {
        let dirty_xlsx = self.session.active().is_some_and(|d| d.is_dirty() && d.path.as_deref().is_some_and(|p| p.ends_with(".xlsx")));
        let now = crate::monotonic_ms();
        let autosave_due =
            self.ui.autosave && self.editor.is_none() && self.text_box_editor.is_none() && now - self.grid.last_autosave > 2000.0 && dirty_xlsx;
        if autosave_due {
            self.grid.last_autosave = now;
            self.store_view_state();
            let _ = self.session.run("file.save", json!({}));
        } else if self.ui.autosave && dirty_xlsx {
            // Keep checking even when no input arrives while the window is hidden.
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
    }

    /// Flip AutoSave (title-bar switch / Options ▸ Save). Persisted with the other UI
    /// preferences, so it stays as the user left it across restarts (#1901).
    pub fn toggle_autosave(&mut self) {
        self.ui.autosave = !self.ui.autosave;
    }
}

#[cfg(test)]
mod template_tests {
    use super::*;
    use crate::{SaveAnswer, Services};

    fn app() -> SheetApp {
        let mut session = nanshe_engine::Session::new();
        session.new_workbook();
        SheetApp::new(session, Services::default())
    }

    #[test]
    fn template_tile_and_save_cancel_buttons_keep_the_current_window() {
        use egui_kittest::kittest::Queryable;
        let mut app = app();
        app.use_prepared_fonts();
        let window = app.session.active_window();
        app.open_backstage(crate::backstage::Page::New);
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1280.0, 900.0))
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
        h.run_steps(3);
        let label = nanshe_engine::sample::SAMPLES.iter().find(|(n, _)| *n == "budget").unwrap().1;
        h.get_by_label(label).click();
        h.run_steps(3);
        assert_eq!(h.state().session.active_window(), window);
        assert_eq!(h.state().session.windows().len(), 1);
        let uid = h.state().session.doc().unwrap().uid;
        h.state_mut().run("ui.openTemplate", json!({"sample":"sales"})).unwrap();
        h.run_steps(2);
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Cancel").click();
        h.run_steps(2);
        assert_eq!(h.state().session.doc().unwrap().uid, uid);
        h.state_mut().run("ui.openTemplate", json!({"sample":"sales"})).unwrap();
        h.run_steps(2);
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Don't Save").click();
        h.run_steps(2);
        assert_ne!(h.state().session.doc().unwrap().uid, uid);
        assert_eq!(h.state().session.active_window(), window);
        assert_eq!(h.state().session.windows().len(), 1);
    }

    #[test]
    fn template_reuses_blank_window_and_gets_a_new_document_identity() {
        let mut app = app();
        let window = app.session.active_window();
        let uid = app.session.doc().unwrap().uid;
        app.run("ui.openTemplate", json!({"sample":"budget"})).unwrap();
        assert_eq!(app.session.active_window(), window);
        assert_eq!(app.session.windows().len(), 1);
        assert_eq!(app.session.documents().len(), 1);
        assert_ne!(app.session.doc().unwrap().uid, uid);
        assert!(app.session.doc().unwrap().wb.active().unwrap().used_range().is_some());
    }

    #[test]
    fn template_cancel_discard_and_invalid_requests_protect_current_workbook() {
        let mut app = app();
        app.run("cell.set", json!({"cell":"A1","input":"Keep me"})).unwrap();
        let uid = app.session.doc().unwrap().uid;
        let window = app.session.active_window();
        app.run("ui.openTemplate", json!({"sample":"budget"})).unwrap();
        assert_eq!(app.dialog.as_ref().unwrap().name(), "saveChanges");
        assert_eq!(app.session.doc().unwrap().uid, uid);
        assert!(app.answer_template_save(SaveAnswer::Cancel, "budget", uid));
        assert_eq!(app.session.doc().unwrap().uid, uid);
        assert!(app.run("file.new", json!({"sample":"budget","replace":true})).is_err());
        assert!(app.run("file.new", json!({"sample":"missing","replace":true,"force":true})).is_err());
        assert_eq!(app.run("cell.get", json!({"cell":"A1"})).unwrap()["value"], "Keep me");
        assert!(app.answer_template_save(SaveAnswer::DontSave, "budget", uid));
        assert_ne!(app.session.doc().unwrap().uid, uid);
        assert_eq!(app.session.active_window(), window);
    }

    #[test]
    fn template_save_requires_success_and_cancelled_picker_keeps_original() {
        let mut app = app();
        app.run("cell.set", json!({"cell":"A1","input":"Saved original"})).unwrap();
        let uid = app.session.doc().unwrap().uid;
        app.services.pick_save = Some(Box::new(|_| None));
        assert!(!app.answer_template_save(SaveAnswer::Save, "budget", uid));
        assert_eq!(app.session.doc().unwrap().uid, uid);
        let dir = std::env::temp_dir().join(format!("celltrellis-template-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("original.xlsx");
        let path = target.to_string_lossy().into_owned();
        app.services.pick_save = Some(Box::new(move |_| Some(path.clone())));
        assert!(app.answer_template_save(SaveAnswer::Save, "budget", uid));
        assert_ne!(app.session.doc().unwrap().uid, uid);
        let mut original = nanshe_engine::Session::new();
        original.run("file.open", json!({"path":target})).unwrap();
        assert_eq!(original.run("cell.get", json!({"cell":"A1"})).unwrap()["value"], "Saved original");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
