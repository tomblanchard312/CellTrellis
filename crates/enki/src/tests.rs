use super::*;
use std::io::Cursor;

#[test]
fn chat_request_matches_the_documented_frame() {
    let msgs = [Message::new("system", "Be brief."), Message::new("user", "Hi")];
    let p = Params { max_tokens: Some(99_999), temperature: Some(f32::NAN), seed: Some(7) };
    let v = chat_request("m", &msgs, &p).unwrap();
    assert_eq!(
        v,
        json!({"type":"chat","model":"m","messages":[{"role":"system","content":"Be brief."},{"role":"user","content":"Hi"}],"params":{"max_tokens":4096,"seed":7}})
    );
    assert!(matches!(chat_request("", &msgs, &p), Err(Error::Invalid(_))));
    assert!(matches!(chat_request("m", &[], &p), Err(Error::Invalid(_))));
}

#[test]
fn encode_enforces_the_frame_limit() {
    let big = json!({"x": "a".repeat(100)});
    assert!(matches!(encode(&big, 50), Err(Error::Invalid(_))));
    assert_eq!(encode(&json!({"a":1}), 50).unwrap(), b"{\"a\":1}\n");
}

#[test]
fn frames_are_split_bounded_and_truncation_is_an_error() {
    let mut r = FrameReader::new(Cursor::new(b"{\"a\":1}\n{\"b\":2}\n".to_vec()), 64);
    assert_eq!(r.next_frame().unwrap().unwrap(), b"{\"a\":1}");
    assert_eq!(r.next_frame().unwrap().unwrap(), b"{\"b\":2}");
    assert!(r.next_frame().unwrap().is_none());
    let mut r = FrameReader::new(Cursor::new(b"{\"a\":".to_vec()), 64);
    assert!(matches!(r.next_frame(), Err(Error::Protocol(_))));
    let mut r = FrameReader::new(Cursor::new(vec![b'x'; 100_000]), 64);
    assert!(matches!(r.next_frame(), Err(Error::Protocol(_))));
}

#[test]
fn terminal_frames_map_to_errors() {
    assert!(
        matches!(parse_event(br#"{"type":"busy","retry_after_ms":1500}"#), Err(Error::Busy { retry_after }) if retry_after == Duration::from_millis(1500))
    );
    assert!(
        matches!(parse_event(br#"{"type":"inference_error","kind":"model_not_found","message":"no"}"#), Err(Error::Inference { kind, .. }) if kind == "model_not_found")
    );
    assert!(matches!(parse_event(b"not json"), Err(Error::Protocol(_))));
    assert!(matches!(parse_event(br#"{"type":"surprise"}"#), Err(Error::Protocol(_))));
}

fn stream(frames: &str) -> FrameReader<Cursor<Vec<u8>>> {
    FrameReader::new(Cursor::new(frames.as_bytes().to_vec()), MAX_FRAME_V2)
}

#[test]
fn drive_streams_chunks_in_order_to_the_summary() {
    let mut r = stream(
        "{\"type\":\"chunk\",\"seq\":0,\"text\":\"Hel\"}\n{\"type\":\"chunk\",\"seq\":1,\"text\":\"lo\"}\n{\"type\":\"done\",\"reason\":\"stop\",\"prompt_tokens\":9,\"completion_tokens\":2}\n",
    );
    let mut out = String::new();
    let s = drive(&mut r, &AtomicBool::new(false), Timeouts::default(), |_| Ok(()), |t| out.push_str(t)).unwrap();
    assert_eq!(out, "Hello");
    assert_eq!(s, Summary { reason: "stop".into(), prompt_tokens: 9, completion_tokens: 2 });
}

#[test]
fn drive_rejects_gaps_and_streams_without_a_result() {
    let mut r = stream("{\"type\":\"chunk\",\"seq\":1,\"text\":\"x\"}\n");
    assert!(matches!(drive(&mut r, &AtomicBool::new(false), Timeouts::default(), |_| Ok(()), |_| {}), Err(Error::Protocol(_))));
    let mut r = stream("{\"type\":\"chunk\",\"seq\":0,\"text\":\"x\"}\n");
    assert!(matches!(drive(&mut r, &AtomicBool::new(false), Timeouts::default(), |_| Ok(()), |_| {}), Err(Error::Protocol(_))));
}

#[test]
fn drive_sends_cancel_when_asked() {
    let mut r = stream("{\"type\":\"chunk\",\"seq\":0,\"text\":\"x\"}\n");
    let mut sent = Vec::new();
    let res = drive(
        &mut r,
        &AtomicBool::new(true),
        Timeouts::default(),
        |b| {
            sent.extend_from_slice(b);
            Ok(())
        },
        |_| {},
    );
    assert!(matches!(res, Err(Error::Cancelled)));
    assert_eq!(sent, b"{\"type\":\"cancel\"}\n");
}

#[cfg(not(windows))]
#[test]
fn a_configured_endpoint_wins() {
    assert_eq!(endpoint(" /run/x.sock ").unwrap(), PathBuf::from("/run/x.sock"));
}

/// A fake daemon on a real socket: negotiation, the model list and a streamed chat.
#[cfg(unix)]
#[test]
fn talks_to_a_daemon_over_a_unix_socket() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    let dir = std::env::temp_dir().join(format!("nisaba-sharedai-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let sock = dir.join("ipc.sock");
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).unwrap();
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..2 {
            let (s, _) = listener.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            let mut lines = BufReader::new(s).lines();
            let hello: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
            let version = hello["versions"][0].as_u64().unwrap();
            writeln!(w, "{}", json!({"type":"negotiated","version":version,"process_id":1})).unwrap();
            let req: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
            match req["type"].as_str().unwrap() {
                "list_models" => writeln!(w, "{}", json!({"type":"models","ids":["tiny","big"]})).unwrap(),
                "chat" => {
                    for (i, t) in ["Plain", " words."].iter().enumerate() {
                        writeln!(w, "{}", json!({"type":"chunk","seq":i,"text":t})).unwrap();
                    }
                    writeln!(w, "{}", json!({"type":"done","reason":"stop","prompt_tokens":5,"completion_tokens":2})).unwrap();
                }
                other => panic!("unexpected {other}"),
            }
            seen.push((version, req));
        }
        seen
    });
    assert_eq!(list_models(&sock, "nisaba").unwrap(), vec!["tiny".to_string(), "big".to_string()]);
    let mut out = String::new();
    let s =
        chat(&sock, "nisaba", "tiny", &[Message::new("user", "Rewrite")], &Params::default(), Timeouts::default(), &AtomicBool::new(false), |t| {
            out.push_str(t)
        })
        .unwrap();
    assert_eq!((out.as_str(), s.completion_tokens), ("Plain words.", 2));
    let seen = server.join().unwrap();
    assert_eq!(seen[0].0, 1);
    assert_eq!(seen[1].0, 2);
    assert_eq!(seen[1].1["messages"][0]["content"], "Rewrite");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_missing_daemon_says_so() {
    let e = list_models(std::path::Path::new("/nonexistent/nisaba/ipc.sock"), "nisaba").unwrap_err();
    assert!(e.to_string().contains("isn't running"), "{e}");
}

#[cfg(unix)]
#[test]
fn the_daemon_is_looked_for_in_the_private_helper_folder_first() {
    let c = daemon_candidates(Some(std::path::Path::new("/usr/bin/nisaba")), Some(std::ffi::OsStr::new("/opt/x:/usr/local/bin")));
    let c: Vec<String> = c.iter().map(|p| p.display().to_string()).collect();
    assert_eq!(
        c,
        [
            "/usr/bin/enki/sharedai-daemon",
            "/usr/lib/celltrellis/enki/sharedai-daemon",
            "/usr/lib/nisaba/sharedai-daemon",
            "/usr/bin/sharedai-daemon",
            "/opt/x/sharedai-daemon",
            "/usr/local/bin/sharedai-daemon"
        ]
    );
}

#[test]
fn model_ids_follow_the_registry_rules() {
    use std::path::Path;
    assert_eq!(model_id(Path::new("/m/Qwen2.5-1.5B-Instruct-Q4_K_M.gguf")), "Qwen2.5-1.5B-Instruct-Q4_K_M");
    assert_eq!(model_id(Path::new("/m/my model (v2).gguf")), "my-model--v2");
    assert_eq!(model_id(Path::new("/m/..gguf")), "model");
    assert!(model_id(Path::new(&format!("/m/{}.gguf", "a".repeat(300)))).len() <= 128);
}

#[test]
fn files_hash_to_their_sha256() {
    let p = std::env::temp_dir().join(format!("nisaba-hash-{}", std::process::id()));
    std::fs::write(&p, b"abc").unwrap();
    let (size, hex) = hash_file(&p).unwrap();
    let _ = std::fs::remove_file(&p);
    assert_eq!(size, 3);
    assert_eq!(hex, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn oversized_newline_terminated_frames_are_rejected() {
    let mut reader = FrameReader::new(Cursor::new(b"123456789\n"), 4);
    assert!(matches!(reader.next_frame(), Err(Error::Protocol(_))));
}

/// Opt-in real daemon/worker check; the caller supplies an isolated running Enki and GGUF.
#[test]
#[ignore = "requires local Enki and NANSHE_ENKI_TEST_MODEL"]
fn live_enki_registers_streams_and_cancels() {
    let path = std::env::var_os("NANSHE_ENKI_TEST_MODEL").expect("set NANSHE_ENKI_TEST_MODEL");
    let endpoint = endpoint("").unwrap();
    register_file(&endpoint, "nanshe-live-test", "nanshe-smoke", std::path::Path::new(&path)).unwrap();
    assert!(list_models(&endpoint, "nanshe-live-test").unwrap().contains(&"nanshe-smoke".into()));
    let messages = [Message::new("system", "Be brief."), Message::new("user", "Say hello in one short sentence.")];
    let params = Params { max_tokens: Some(64), temperature: Some(0.0), seed: Some(7) };
    let mut reply = String::new();
    let result =
        chat(&endpoint, "nanshe-live-test", "nanshe-smoke", &messages, &params, Timeouts::default(), &AtomicBool::new(false), |s| reply.push_str(s))
            .unwrap();
    assert!(!reply.trim().is_empty());
    assert!(result.prompt_tokens > 0 && result.completion_tokens > 0);
    assert_eq!(result.reason, "stop", "{reply}");
    eprintln!("live Enki: {result:?}; reply: {reply}");
    let cancel = AtomicBool::new(false);
    let result = chat(&endpoint, "nanshe-live-test", "nanshe-smoke", &messages, &params, Timeouts::default(), &cancel, |_| {
        cancel.store(true, Ordering::Relaxed);
    });
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    let result = chat(&endpoint, "nanshe-live-test", "missing-model", &messages, &params, Timeouts::default(), &AtomicBool::new(false), |_| {});
    assert!(matches!(result, Err(Error::Inference { kind, .. }) if kind == "model_not_found"));
}
