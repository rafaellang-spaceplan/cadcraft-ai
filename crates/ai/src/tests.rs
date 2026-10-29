use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

use serde_json::{Value, json};

use super::*;

fn cfg(base: &str) -> Config {
    Config { credentials: Credentials::ApiKey("test-key".into()), base_url: base.into(), model: DEFAULT_MODEL.into(), effort: "medium".into() }
}

fn floor() -> FloorInfo {
    FloorInfo { usable_area: 750.0, length: 40.0, width: 20.0, cores: 1, columns: 2, has_entrance: true }
}

/// Every object in a schema is closed and requires all its properties (structured outputs).
fn check_closed(v: &Value, path: &str) {
    if let Some(o) = v.as_object() {
        if o.get("type").and_then(Value::as_str) == Some("object") {
            assert_eq!(o.get("additionalProperties"), Some(&json!(false)), "{path}");
            let props: Vec<&String> = o["properties"].as_object().unwrap().keys().collect();
            let req: Vec<&str> = o["required"].as_array().unwrap().iter().map(|r| r.as_str().unwrap()).collect();
            assert_eq!(props.len(), req.len(), "{path}");
            assert!(props.iter().all(|p| req.contains(&p.as_str())), "{path}");
        }
        for (k, x) in o {
            check_closed(x, &format!("{path}/{k}"));
        }
    } else if let Some(a) = v.as_array() {
        for (i, x) in a.iter().enumerate() {
            check_closed(x, &format!("{path}/{i}"));
        }
    }
}

#[test]
fn schemas_suit_structured_outputs() {
    check_closed(&schema::program(), "program");
    check_closed(&schema::review(), "review");
}

#[test]
fn interpret_request_shape() {
    let r = interpret_request(&cfg("x"), &Brief::text("60 estações, 4 salas de reunião"), Some(&floor())).unwrap();
    assert_eq!(r["model"], "claude-opus-5-5");
    assert_eq!(r["fallbacks"], "default");
    assert_eq!(r["output_config"]["effort"], "medium");
    assert_eq!(r["output_config"]["format"]["type"], "json_schema");
    assert!(r.get("thinking").is_none(), "Opus 5.5 thinks adaptively by default");
    let text = r["messages"][0]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("<briefing>\n60 estações") && text.contains("usable area 750 m²"), "{text}");
    // A PDF goes first, as a document block.
    let pdf = Brief { text: String::new(), pdf: Some(b"%PDF-1.7 ...".to_vec()) };
    let r = interpret_request(&cfg("x"), &pdf, None).unwrap();
    assert_eq!(r["messages"][0]["content"][0]["type"], "document");
    assert_eq!(r["messages"][0]["content"][0]["source"]["data"], "JVBERi0xLjcgLi4u");
    assert!(r["messages"][0]["content"][1]["text"].as_str().unwrap().starts_with("The briefing is the attached document."));
}

#[test]
fn headers_carry_the_version_the_fallback_beta_and_the_credentials() {
    let h = cfg("https://api.anthropic.com/").headers();
    let get = |k: &str| h.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
    assert_eq!(get("x-api-key").as_deref(), Some("test-key"));
    assert_eq!(get("anthropic-version").as_deref(), Some("2026-10-21"));
    assert_eq!(get("anthropic-beta").as_deref(), Some("server-side-fallback-2026-07-01"));
    assert_eq!(cfg("https://api.anthropic.com/").url(), "https://api.anthropic.com/v1/messages");
    let bearer = Config { credentials: Credentials::Bearer("tok".into()), ..cfg("x") }.headers();
    assert!(bearer.iter().any(|(k, v)| *k == "authorization" && v == "Bearer tok"));
    assert!(bearer.iter().any(|(k, v)| *k == "anthropic-beta" && v == "server-side-fallback-2026-07-01,oauth-2025-04-20"));
    assert!(!bearer.iter().any(|(k, _)| *k == "x-api-key"));
}

#[test]
fn briefs_are_checked() {
    for b in [
        Brief::text("  "),
        Brief { text: String::new(), pdf: Some(Vec::new()) },
        Brief { text: "x".into(), pdf: Some(b"PK\x03\x04".to_vec()) },
        Brief::text("x".repeat(MAX_BRIEF_CHARS + 1)),
    ] {
        assert!(matches!(interpret_request(&cfg("x"), &b, None), Err(Error::Brief(_))));
    }
}

fn ok_body(answer: &Value) -> String {
    json!({
        "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "content": [{ "type": "thinking", "thinking": "", "signature": "x" }, { "type": "text", "text": answer.to_string() }],
        "stop_reason": "end_turn", "stop_details": null, "usage": { "input_tokens": 10, "output_tokens": 20 }
    })
    .to_string()
}

fn program_answer() -> Value {
    json!({
        "program": {
            "workstations": { "count": 60, "deskWidth": 1.2, "deskDepth": 0.7, "perSide": null },
            "spaces": [
                { "kind": "meeting", "name": "Sala de reunião", "count": 4, "seats": 6, "width": null, "depth": null, "area": null, "enclosed": null, "zone": null },
                { "kind": "office", "name": "Diretoria", "count": 1, "seats": null, "width": null, "depth": null, "area": null, "enclosed": null, "zone": "facade" }
            ],
            "corridor": 1.2, "mainCorridor": 1.5, "lang": "pt-BR"
        },
        "assumptions": ["Uma mesa por pessoa."],
        "fits": true
    })
}

#[test]
fn answers_are_read_and_checked() {
    let v = answer(200, &ok_body(&program_answer())).unwrap();
    let i = read_interpretation(v).unwrap();
    assert_eq!(i.program.workstations.count, 60);
    assert_eq!(i.program.spaces[1].zone, Some(cadcraft_spaceplan::Zone::Facade));
    assert_eq!(i.program.lang, cadcraft_spaceplan::Lang::PtBr);
    assert_eq!(i.assumptions, vec!["Uma mesa por pessoa."]);
    // A program the generator refuses is a bad answer, not a crash.
    let mut bad = program_answer();
    bad["program"]["workstations"]["count"] = json!(1_000_000);
    assert!(matches!(read_interpretation(bad), Err(Error::BadAnswer(_))));
    assert!(matches!(read_interpretation(json!({"program": 5})), Err(Error::BadAnswer(_))));
    let refusal = json!({ "content": [], "stop_reason": "refusal", "stop_details": { "type": "refusal", "category": "cyber", "explanation": null } });
    assert_eq!(answer(200, &refusal.to_string()), Err(Error::Refused("cyber".into())));
    assert_eq!(answer(200, &json!({ "content": [], "stop_reason": "max_tokens" }).to_string()), Err(Error::Truncated));
    let err = json!({ "type": "error", "error": { "type": "authentication_error", "message": "invalid x-api-key" } });
    assert_eq!(
        answer(401, &err.to_string()),
        Err(Error::Api { status: 401, kind: "authentication_error".into(), message: "invalid x-api-key".into() })
    );
    assert!(matches!(answer(502, "<html>bad gateway</html>"), Err(Error::Api { status: 502, .. })));
    assert!(matches!(answer(200, "{}"), Err(Error::BadAnswer(_))));
    assert!(matches!(
        answer(200, &json!({ "content": [{ "type": "text", "text": "not json" }], "stop_reason": "end_turn" }).to_string()),
        Err(Error::BadAnswer(_))
    ));
}

#[test]
fn reviews_are_checked_against_the_options() {
    let r = json!({ "recommended": 2, "summary": "A 2.", "options": [{ "option": 1, "pros": ["a"], "cons": [] }, { "option": 9, "pros": [], "cons": [] }] });
    let review = read_review(r.clone(), 3).unwrap();
    assert_eq!(review.recommended, 2);
    assert_eq!(review.options.len(), 1, "option 9 doesn't exist");
    assert!(read_review(r.clone(), 1).is_err());
    assert!(read_review(json!({ "recommended": 0, "summary": "", "options": [] }), 3).is_err());
}

#[test]
fn review_request_summarizes_the_options() {
    let result = json!({ "usableArea": 751.4, "options": [
        { "description": "salas no núcleo", "score": 99.0, "metrics": { "workstations": 60, "workstationsTarget": 60, "rooms": 3, "roomsTarget": 4, "daylight": 0.9,
          "areaPerPerson": 12.0, "circulationArea": 400.0, "spaces": [{ "name": "Sala", "placed": 3, "target": 4 }, { "name": "Copa", "placed": 1, "target": 1 }] } }
    ]});
    let r = review_request(&cfg("x"), &Brief::text("briefing"), &result).unwrap();
    let text = r["messages"][0]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"option\":1") && text.contains("\"notPlaced\":[{\"name\":\"Sala\",\"placed\":3,\"target\":4}]"), "{text}");
    assert!(review(&cfg("x"), &Brief::text("b"), &json!({ "options": [] })).is_err());
}

/// A one-shot HTTP server answering `status` with `body`; returns its base URL and a handle
/// yielding the request it received.
fn serve(status: u16, body: String) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(sock.try_clone().unwrap());
        let mut head = String::new();
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                len = v.trim().parse().unwrap();
            }
            head.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        let mut req = vec![0; len];
        reader.read_exact(&mut req).unwrap();
        write!(sock, "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
            .unwrap();
        format!("{head}{}", String::from_utf8_lossy(&req))
    });
    (base, h)
}

#[test]
fn interpret_over_http() {
    let (base, server) = serve(200, ok_body(&program_answer()));
    let i = interpret(&cfg(&base), &Brief::text("60 estações"), Some(&floor())).unwrap();
    assert_eq!(i.program.spaces.len(), 2);
    let req = server.join().unwrap();
    assert!(req.starts_with("POST /v1/messages "), "{req}");
    assert!(req.to_ascii_lowercase().contains("x-api-key: test-key"));
    assert!(req.contains("\"fallbacks\":\"default\""));
    let (base, server) = serve(529, json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } }).to_string());
    assert!(matches!(interpret(&cfg(&base), &Brief::text("x"), None), Err(Error::Api { status: 529, .. })));
    server.join().unwrap();
    // Nothing listening.
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert!(matches!(interpret(&cfg(&format!("http://127.0.0.1:{port}")), &Brief::text("x"), None), Err(Error::Network(_))));
}
