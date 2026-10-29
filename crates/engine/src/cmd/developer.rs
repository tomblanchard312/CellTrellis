//! Nanshe's source editor and bounded, transactional macro/Scribe edits.
use std::sync::Arc;

use serde_json::{Value as Json, json};

use super::{CommandSpec, Result, Session, bad, cmd, has_doc, stash, str_param, target_range};

pub const MAX_SOURCE: usize = 128 * 1024;
pub const MAX_STEPS: usize = 1_000;
pub const MAX_CELLS: u64 = 10_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "developer.source", "Read Script Source", ["Developer"], None, "{language?: vba|python|r} → {source}; no code runs on open", has_doc, source),
        cmd!(
            "developer.saveSource",
            "Save Script Source",
            ["Developer"],
            None,
            "{source: script text (128 KiB max), language?: vba|python|r}",
            has_doc,
            save_source
        ),
        cmd!(
            "developer.apply",
            "Apply Reviewed Changes",
            ["Developer"],
            None,
            "{doc, revision, commands: [{command, params}]}; bounded workbook commands, one undo step",
            has_doc,
            apply
        ),
    ]
}

fn source_key(p: &Json) -> Result<&'static str> {
    match str_param(p, "language").unwrap_or("vba") {
        "vba" => Ok("vba_source"),
        "python" => Ok("python_source"),
        "r" => Ok("r_source"),
        _ => Err(bad("developer.source", "language must be vba, python or r")),
    }
}

fn source(s: &mut Session, p: &Json) -> Result<Json> {
    Ok(json!({"source": stash::get(&s.doc()?.wb, source_key(p)?).unwrap_or_default()}))
}

fn save_source(s: &mut Session, p: &Json) -> Result<Json> {
    let text = str_param(p, "source").ok_or_else(|| bad("developer.saveSource", "source is required"))?;
    if text.len() > MAX_SOURCE {
        return Err(bad("developer.saveSource", "source exceeds 128 KiB"));
    }
    stash::set(Arc::make_mut(&mut s.doc_mut()?.wb), source_key(p)?, text)?;
    Ok(Json::Null)
}

/// Shared boundary for the isolated preview session and final replay. No file, shell, network,
/// other workbook, hidden-name storage, or nested automation commands are exposed to macros.
pub fn validate(s: &Session, command: &str, p: &Json) -> Result<()> {
    let keys: &[&str] = match command {
        "document.inspect" => &[],
        "cell.get" => &["cell", "sheet"],
        "sheet.read" => &["sheet", "range", "formulas"],
        "sheet.activate" => &["sheet"],
        "selection.set" => &["sheet", "range", "cell"],
        "range.setValues" => &["sheet", "range", "values", "parse"],
        "edit.clearContents" => &["sheet", "range"],
        "home.bold" | "home.italic" => &["sheet", "range", "on"],
        "home.fillColor" | "home.fontColor" => &["sheet", "range", "color"],
        "home.fontSize" => &["sheet", "range", "size"],
        "home.fontName" => &["sheet", "range", "name"],
        "home.numberFormat" => &["sheet", "range", "code"],
        _ => return Err(bad("developer.apply", format!("macro command {command} is not allowed"))),
    };
    let object = p.as_object().ok_or_else(|| bad("developer.apply", "params must be an object"))?;
    if object.keys().any(|key| !keys.contains(&key.as_str())) || p.to_string().len() > MAX_SOURCE {
        return Err(bad("developer.apply", "unknown parameters or request exceeds 128 KiB"));
    }
    if !matches!(command, "document.inspect" | "sheet.activate") {
        if str_param(p, "cell").is_none() && str_param(p, "range").is_none() {
            return Err(bad("developer.apply", "an explicit range or cell is required"));
        }
        let range = if let Some(cell) = str_param(p, "cell") {
            super::target_range(s, &json!({"range":cell, "sheet": p.get("sheet")}))?
        } else {
            target_range(s, p)?
        };
        if range.count() > MAX_CELLS {
            return Err(bad("developer.apply", "macro ranges are limited to 10,000 cells"));
        }
    }
    if command == "range.setValues" {
        let rows = p.get("values").and_then(Json::as_array).ok_or_else(|| bad(command, "values must be rows"))?;
        let range = target_range(s, p)?;
        if rows.len() != range.height() as usize || rows.iter().any(|row| row.as_array().is_none_or(|cols| cols.len() != range.width() as usize)) {
            return Err(bad(command, "value dimensions must match the target range"));
        }
        if rows.iter().filter_map(Json::as_array).flatten().any(|v| v.is_array() || v.is_object()) {
            return Err(bad(command, "cells must be strings, numbers, booleans or null"));
        }
    }
    Ok(())
}

pub fn execute(s: &mut Session, command: &str, p: Json) -> Result<Json> {
    validate(s, command, &p)?;
    s.execute(command, p)
}

fn apply(s: &mut Session, p: &Json) -> Result<Json> {
    let d = s.doc()?;
    if p.get("doc").and_then(Json::as_u64) != Some(d.uid) || p.get("revision").and_then(Json::as_u64) != Some(d.revision) {
        return Err(bad("developer.apply", "workbook changed since preview; run the preview again"));
    }
    let commands = p.get("commands").and_then(Json::as_array).ok_or_else(|| bad("developer.apply", "commands are required"))?;
    if commands.len() > MAX_STEPS || p.to_string().len() > 4 * 1024 * 1024 {
        return Err(bad("developer.apply", "too many commands or more than 4 MiB of changes"));
    }
    s.grouped("Nanshe reviewed changes", |s| {
        for step in commands {
            let id = str_param(step, "command").ok_or_else(|| bad("developer.apply", "command is required"))?;
            execute(s, id, step.get("params").cloned().unwrap_or_else(|| json!({})))?;
        }
        Ok(json!({"steps": commands.len()}))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(s: &Session, commands: Json) -> Json {
        let d = s.doc().unwrap();
        json!({"doc":d.uid,"revision":d.revision,"commands":commands})
    }

    #[test]
    fn reviewed_changes_are_atomic_and_one_undo_step() {
        let mut s = Session::new();
        s.new_workbook();
        let p = params(
            &s,
            json!([
                {"command":"range.setValues","params":{"range":"A1:A2","values":[[2],[3]]}},
                {"command":"range.setValues","params":{"range":"B1","values":[["=SUM(A1:A2)"]]}}
            ]),
        );
        s.execute("developer.apply", p).unwrap();
        assert_eq!(s.execute("cell.get", json!({"cell":"B1"})).unwrap()["value"], 5.0);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.execute("cell.get", json!({"cell":"A1"})).unwrap()["value"], Json::Null);
        let p = params(
            &s,
            json!([
                {"command":"range.setValues","params":{"range":"A1","values":[[9]]}},
                {"command":"file.saveAs","params":{"path":"/tmp/no.xlsx"}}
            ]),
        );
        assert!(s.execute("developer.apply", p).is_err());
        assert_eq!(s.execute("cell.get", json!({"cell":"A1"})).unwrap()["value"], Json::Null);
    }

    #[test]
    fn refuses_stale_previews_host_access_and_oversize_ranges() {
        let mut s = Session::new();
        s.new_workbook();
        let stale = params(&s, json!([]));
        s.execute("cell.set", json!({"cell":"A1","input":"7"})).unwrap();
        assert!(s.execute("developer.apply", stale).is_err());
        assert!(validate(&s, "sheet.read", &json!({"range":"A1:XFD1048576"})).is_err());
        assert!(validate(&s, "cell.get", &json!({"cell":"A1","doc":2})).is_err());
        assert!(validate(&s, "range.setValues", &json!({"range":"A1","values":[[1,2]]})).is_err());
        assert!(validate(&s, "developer.apply", &json!({})).is_err());
    }

    #[test]
    fn source_roundtrips_without_executing() {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("developer.saveSource", json!({"source":"Sub Main()\nEnd Sub"})).unwrap();
        assert_eq!(s.execute("developer.source", json!({})).unwrap()["source"], "Sub Main()\nEnd Sub");
        assert!(s.execute("developer.saveSource", json!({"source":"a".repeat(MAX_SOURCE + 1)})).is_err());
    }
    #[test]
    fn all_language_sources_survive_xlsx_roundtrip() {
        let mut s = Session::new();
        s.new_workbook();
        for (language, source) in [("vba", "Sub Main()\nEnd Sub"), ("python", "print('你好')"), ("r", "print(42)")] {
            s.execute("developer.saveSource", json!({"language":language,"source":source})).unwrap();
        }
        let bytes = nanshe_xlsx::write_xlsx(&s.doc().unwrap().wb).unwrap();
        let loaded = crate::io::open_bytes("scripts.xlsx", &bytes).unwrap().0;
        for (language, source) in [("vba", "Sub Main()\nEnd Sub"), ("python", "print('你好')"), ("r", "print(42)")] {
            assert_eq!(stash::get(&loaded, &format!("{language}_source")).unwrap(), source);
        }
    }
}
