//! Optional .NET helper, private stdio, and an engine-only workbook preview.
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[allow(clippy::disallowed_methods)] // This module is compiled only on native targets.
fn instant() -> Instant {
    Instant::now()
}

use gridcraft_engine::{DocState, Session, cmd::developer};
use serde_json::{Value as Json, json};

struct Reap(Child);
impl Drop for Reap {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ =
            Command::new("/bin/kill").args(["-s", "KILL", "--", &format!("-{}", self.0.id())]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        #[cfg(windows)]
        let _ = Command::new("taskkill").args(["/PID", &self.0.id().to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(super) fn macro_preview(program: &Path, source: &str, name: &str, doc: DocState, cancel: Arc<AtomicBool>) -> Result<Json, String> {
    if source.len() > developer::MAX_SOURCE {
        return Err("Source exceeds 128 KiB".into());
    }
    let mut command = Command::new(program);
    command.arg("bridge").env("DOTNET_EnableDiagnostics", "0").env("DOTNET_GCHeapHardLimit", "0x10000000");
    run(command, source, name, doc, cancel)
}

pub(super) fn script_preview(language: &str, source: &str, doc: DocState, cancel: Arc<AtomicBool>) -> Result<Json, String> {
    let command = match language {
        "Python" => {
            let mut cmd =
                Command::new(std::env::var_os("NANSHE_PYTHON").unwrap_or_else(|| if cfg!(windows) { "python".into() } else { "python3".into() }));
            cmd.args(["-I", "-u", "-X", "utf8", "-c", include_str!("runners/python.py")]);
            cmd
        }
        "R" => {
            let mut cmd = Command::new(std::env::var_os("NANSHE_RSCRIPT").unwrap_or_else(|| "Rscript".into()));
            cmd.args(["--vanilla", "-e", include_str!("runners/r.R")]);
            cmd
        }
        _ => return Err("Unknown scripting language".into()),
    };
    run(command, source, "", doc, cancel)
}

fn run(mut command: Command, source: &str, name: &str, doc: DocState, cancel: Arc<AtomicBool>) -> Result<Json, String> {
    if source.len() > developer::MAX_SOURCE {
        return Err("Source exceeds 128 KiB".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Cannot start the selected runtime: {e}. See docs/nanshe-developer.md for runtime setup."))?;
    let input = child.stdin.take();
    let output = child.stdout.take();
    let finished = Arc::new(AtomicBool::new(false));
    let mut child = Reap(child);
    let done = finished.clone();
    let stopped = cancel.clone();
    let watcher = std::thread::Builder::new()
        .name("nanshe-script-watch".into())
        .spawn(move || {
            let start = instant();
            loop {
                if done.load(Ordering::Relaxed) || stopped.load(Ordering::Relaxed) || start.elapsed() >= Duration::from_secs(15) {
                    break;
                }
                if child.0.try_wait().ok().flatten().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(child);
        })
        .map_err(|e| e.to_string())?;
    let result = match (input, output) {
        (Some(mut input), Some(output)) => {
            let request = json!({"source":source,"macro":name});
            match writeln!(input, "{request}").and_then(|_| input.flush()) {
                Ok(()) => bridge(input, BufReader::new(output), doc, &cancel),
                Err(e) => Err(e.to_string()),
            }
        }
        _ => Err("Script runner has no private stdio pipes".into()),
    };
    finished.store(true, Ordering::Relaxed);
    let _ = watcher.join();
    if cancel.load(Ordering::Relaxed) { Err("Stopped; no workbook changes were applied".into()) } else { result }
}

fn bridge(mut input: impl Write, mut output: impl BufRead, doc: DocState, cancel: &AtomicBool) -> Result<Json, String> {
    let (uid, revision) = (doc.uid, doc.revision);
    let mut preview = Session::new();
    preview.add_document(doc);
    let mut commands = Vec::new();
    let mut log = String::new();
    let mut total = 0usize;
    for _ in 0..=developer::MAX_STEPS * 2 {
        if cancel.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        let mut line = Vec::new();
        output.by_ref().take(256 * 1024 + 1).read_until(b'\n', &mut line).map_err(|e| e.to_string())?;
        total = total.saturating_add(line.len());
        if line.len() > 256 * 1024 || total > 4 * 1024 * 1024 || line.last() != Some(&b'\n') {
            return Err("Script runner disconnected, timed out, or exceeded its output limit".into());
        }
        let v: Json = serde_json::from_slice(&line).map_err(|e| e.to_string())?;
        match v.get("type").and_then(Json::as_str) {
            Some("command") => {
                if commands.len() >= developer::MAX_STEPS {
                    return Err("Script exceeded 1,000 engine calls".into());
                }
                let id = v.get("command").and_then(Json::as_str).ok_or("Missing command")?;
                let p = v.get("params").cloned().unwrap_or_else(|| json!({}));
                // A rejected engine command invalidates the entire preview, even if VBA catches it.
                let r = developer::execute(&mut preview, id, p.clone()).map_err(|e| e.to_string())?;
                let reply = json!({"result": r}).to_string();
                if reply.len() > 256 * 1024 {
                    return Err("Macro result exceeds 256 KiB; read a smaller range".into());
                }
                writeln!(input, "{reply}").and_then(|_| input.flush()).map_err(|e| e.to_string())?;
                commands.push(json!({"command":id,"params":p}));
            }
            Some("output") => {
                let text = v.get("text").and_then(Json::as_str).unwrap_or("");
                if log.len().saturating_add(text.len()) > 64 * 1024 {
                    return Err("Macro output exceeds 64 KiB".into());
                }
                log.push_str(text);
            }
            Some("error") => return Err(v.get("message").and_then(Json::as_str).unwrap_or("Macro failed").into()),
            Some("done") => return Ok(json!({"doc":uid,"revision":revision,"commands":commands,"output":log})),
            _ => return Err("Unexpected Script runner message".into()),
        }
    }
    Err("Script exceeded its message limit".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn doc() -> DocState {
        let mut s = Session::new();
        s.new_workbook();
        s.doc().unwrap().clone()
    }

    #[test]
    fn bridge_previews_recalculation_without_editing_the_original() {
        let d = doc();
        let frames = [
            json!({"type":"command","command":"range.setValues","params":{"range":"A1","values":[["=2+3"]]}}),
            json!({"type":"command","command":"cell.get","params":{"cell":"A1"}}),
            json!({"type":"done"}),
        ]
        .iter()
        .map(|v| format!("{v}\n"))
        .collect::<String>();
        let mut replies = Vec::new();
        let result = bridge(&mut replies, Cursor::new(frames), d.clone(), &AtomicBool::new(false)).unwrap();
        assert_eq!(result["commands"].as_array().unwrap().len(), 2);
        assert!(String::from_utf8(replies).unwrap().contains("\"value\":5.0"));
        assert!(d.wb.active().unwrap().cells.is_empty());
    }

    #[test]
    fn bridge_rejects_host_access_partial_and_unbounded_output() {
        for frames in [
            "{\"type\":\"command\",\"command\":\"file.save\",\"params\":{}}\n".to_string(),
            "{\"type\":\"done\"}".to_string(),
            "a".repeat(256 * 1024 + 1),
        ] {
            assert!(bridge(Vec::new(), Cursor::new(frames), doc(), &AtomicBool::new(false)).is_err());
        }
    }

    #[test]
    fn real_transpiler_roundtrip_when_runner_is_provided() {
        let Some(path) = std::env::var_os("NANSHE_VBA_TEST_RUNNER") else { return };
        let source = "Sub Main()\nRange(\"A1\").Value = 12\nRange(\"B1\").Formula = \"=A1*2\"\nDebug.Print Range(\"B1\").Value\nEnd Sub";
        let result = macro_preview(Path::new(&path), source, "Main", doc(), Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(result["output"], "24\n");
        assert!(!result["commands"].as_array().unwrap().is_empty());
        let malicious = "Sub Main()\nShell \"touch /tmp/unsafe\"\nEnd Sub";
        assert!(macro_preview(Path::new(&path), malicious, "Main", doc(), Arc::new(AtomicBool::new(false))).is_err());
    }
    #[test]
    fn python_preview_reads_recalculates_and_keeps_strings_literal() {
        let result = script_preview("Python", "nanshe.write('A1', [[12]])\nnanshe.write('B1', [['=A1*2']], formulas=True)\nprint(nanshe.read('B1')[0][0])\nnanshe.write('C1', [['=literal']])", doc(), Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(result["output"], "24.0\n");
        let commands = result["commands"].as_array().unwrap();
        assert_eq!(commands.last().unwrap()["params"]["values"][0][0], "'=literal");
        assert!(script_preview("Python", "nanshe.execute('file.save', path='unsafe.xlsx')", doc(), Arc::new(AtomicBool::new(false))).is_err());
    }

    #[test]
    fn r_preview_roundtrip_when_runtime_is_provided() {
        if std::env::var_os("NANSHE_RSCRIPT").is_none() {
            return;
        }
        let result = script_preview(
            "R",
            "sheet_write('A1', matrix(12, nrow=1))\nsheet_write('B1', matrix('=A1*2', nrow=1), formulas=TRUE)\nprint(sheet_read('B1'))",
            doc(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(result["output"].as_str().unwrap().contains("24"));
    }

    #[test]
    fn stop_terminates_a_busy_interpreter_without_applying_changes() {
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = cancel.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            stopped.store(true, Ordering::Relaxed);
        });
        let result = script_preview(
            "Python",
            "import subprocess, sys, time\nsubprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])\nwhile True: time.sleep(1)",
            doc(),
            cancel,
        );
        stopper.join().unwrap();
        assert!(result.unwrap_err().contains("Stopped"));
    }
}
