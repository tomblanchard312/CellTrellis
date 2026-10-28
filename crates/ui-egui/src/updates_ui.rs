//! Nanshe releases are separate from the upstream Nanshe update feed.
use crate::SheetApp;

pub(crate) fn start(app: &mut SheetApp, manual: bool) {
    if manual {
        app.message = Some(("Nanshe updates".into(), "Get Nanshe releases at https://github.com/tomblanchard312/Nanshe/releases".into()));
    }
}
pub(crate) fn tick(_: &mut SheetApp, _: &egui::Context) {}
pub(crate) fn show(_: &mut SheetApp, _: &egui::Context) {}
