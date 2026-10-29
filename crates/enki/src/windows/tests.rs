use super::*;
use crate::{AtomicBool, Message, Params, Timeouts, chat, json, list_models};
use interprocess::os::windows::named_pipe::PipeListenerOptions;
use std::io::{BufRead, BufReader};

fn pipe(name: &str) -> PathBuf {
    format!(r"\\.\pipe\SharedAI-{}-test-{}-{name}", current_sid().unwrap(), std::process::id()).into()
}

#[test]
fn rejects_remote_other_user_and_malformed_pipes() {
    let sid = current_sid().unwrap();
    assert!(validate(&pipe("valid"), &sid).is_ok());
    for path in [r"\\remote\pipe\SharedAI-x-default", r"\\.\pipe\SharedAI-S-1-5-18-default", r"\\.\pipe\other"] {
        assert!(validate(Path::new(path), &sid).is_err());
    }
    assert!(validate(Path::new(&format!(r"\\.\pipe\SharedAI-{sid}-")), &sid).is_err());
    assert!(validate(&pipe("bad\\path"), &sid).is_err());
    assert!(validate(&pipe(&"a".repeat(65)), &sid).is_err());
    assert_eq!(endpoint(pipe("configured").to_str().unwrap()).unwrap(), pipe("configured"));
    assert!(crate::daemon_candidates(Some(Path::new(r"C:\CellTrellis\nanshe.exe")), None).iter().all(|p| p.ends_with("sharedai-daemon.exe")));
}

#[test]
fn negotiates_lists_and_streams_over_a_real_windows_pipe() {
    let path = pipe("chat");
    let listener = PipeListenerOptions::new().path(path.as_path()).create_duplex::<pipe_mode::Bytes>().unwrap();
    let server = std::thread::spawn(move || {
        for version in [1, 2] {
            let mut stream = BufReader::new(listener.accept().unwrap());
            let mut line = String::new();
            stream.read_line(&mut line).unwrap();
            let hello: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(hello["versions"][0], version);
            writeln!(stream.get_mut(), "{}", json!({"type":"negotiated","version":version})).unwrap();
            line.clear();
            stream.read_line(&mut line).unwrap();
            if version == 1 {
                assert_eq!(serde_json::from_str::<serde_json::Value>(&line).unwrap()["type"], "list_models");
                writeln!(stream.get_mut(), "{}", json!({"type":"models","ids":["local"]})).unwrap();
            } else {
                writeln!(stream.get_mut(), "{}", json!({"type":"chunk","seq":0,"text":"=SUM(A1:A3)"})).unwrap();
                writeln!(stream.get_mut(), "{}", json!({"type":"done","reason":"stop","prompt_tokens":5,"completion_tokens":1})).unwrap();
            }
            stream.get_mut().flush().unwrap();
        }
    });
    assert_eq!(list_models(&path, "nanshe").unwrap(), ["local"]);
    let mut text = String::new();
    let result =
        chat(&path, "nanshe", "local", &[Message::new("user", "Sum")], &Params::default(), Timeouts::default(), &AtomicBool::new(false), |s| {
            text.push_str(s)
        })
        .unwrap();
    assert_eq!(text, "=SUM(A1:A3)");
    assert_eq!(result.reason, "stop");
    server.join().unwrap();
}

#[test]
fn idle_pipe_times_out_and_stop_sends_cancel() {
    let path = pipe("stop");
    let listener = PipeListenerOptions::new().path(path.as_path()).create_duplex::<pipe_mode::Bytes>().unwrap();
    let server = std::thread::spawn(move || {
        for cancel in [false, true] {
            let mut stream = BufReader::new(listener.accept().unwrap());
            let mut line = String::new();
            stream.read_line(&mut line).unwrap();
            writeln!(stream.get_mut(), "{}", json!({"type":"negotiated","version":2})).unwrap();
            line.clear();
            stream.read_line(&mut line).unwrap();
            line.clear();
            if cancel {
                stream.read_line(&mut line).unwrap();
                assert_eq!(serde_json::from_str::<serde_json::Value>(&line).unwrap()["type"], "cancel");
            } else {
                std::thread::sleep(Duration::from_millis(400));
            }
            stream.get_mut().assume_flushed();
        }
    });
    let timeouts = Timeouts { first: Duration::from_millis(100), next: Duration::from_millis(100) };
    let start = Instant::now();
    let result = chat(&path, "nanshe", "local", &[Message::new("user", "Wait")], &Params::default(), timeouts, &AtomicBool::new(false), |_| {});
    assert!(matches!(result, Err(Error::Timeout)), "{result:?}");
    assert!(start.elapsed() < Duration::from_secs(3));
    let result = chat(&path, "nanshe", "local", &[Message::new("user", "Stop")], &Params::default(), timeouts, &AtomicBool::new(true), |_| {});
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    server.join().unwrap();
}
