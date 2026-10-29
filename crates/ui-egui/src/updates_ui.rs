//! CellTrellis releases are separate from the upstream GridCraft update feed.
use crate::SheetApp;

pub(crate) fn start(app: &mut SheetApp, manual: bool) {
    if manual {
        app.message =
            Some(("CellTrellis updates".into(), "Get CellTrellis releases at https://github.com/tomblanchard312/CellTrellis/releases".into()));
    }
}
pub(crate) fn tick(_: &mut SheetApp, _: &egui::Context) {}
pub(crate) fn show(_: &mut SheetApp, _: &egui::Context) {}
