//! The server, exercised the way a client reaches it: over a socket.
//!
//! These start a real listener on a port the operating system picks and speak
//! HTTP to it. That is deliberate — most of what could go wrong here is in the
//! transport and in the guards around it, and neither is visible to a test
//! that calls the handler directly.

// A test binary uses a subset of the module it includes; the rest being
// unused here says nothing about whether it is used in the binary.
#[allow(dead_code)]
#[path = "../src/mcp/mod.rs"]
mod mcp;
// A test binary uses a subset of the module it includes; the rest being
// unused here says nothing about whether it is used in the binary.
#[allow(dead_code)]
#[path = "../src/script.rs"]
mod script;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;

use mcp::json::{self, Json};

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

#[test]
fn the_json_round_trip_survives_the_awkward_values() {
    let source = r#"{"a":[1,-2.5,1e3],"b":"quote \" backslash \\ newline \n","c":{"d":null},"e":[true,false]}"#;
    let parsed = json::parse(source).expect("should parse");
    let again = json::parse(&parsed.write()).expect("what we wrote should parse back");
    assert_eq!(parsed, again, "writing then reading must not change the value");
    assert_eq!(parsed.get("b").and_then(Json::as_str).unwrap(), "quote \" backslash \\ newline \n");
    assert_eq!(parsed.get("a").unwrap().as_array().unwrap()[2].as_f64(), Some(1000.0));
}

#[test]
fn a_surrogate_pair_becomes_one_character() {
    // The emoji arrives as two \u escapes and is one character afterwards.
    let parsed = json::parse(r#""🎨""#).expect("should parse");
    assert_eq!(parsed.as_str(), Some("🎨"));
    // A high surrogate on its own is refused rather than silently mangled.
    assert!(json::parse(r#""\ud83c""#).is_err());
}

#[test]
fn malformed_json_is_refused_rather_than_guessed_at() {
    for bad in [
        "{",
        "{\"a\":}",
        "[1,]",
        "{'a':1}",
        "tru",
        "\"unterminated",
        "{} trailing",
        "{\"a\":1,}",
    ] {
        assert!(json::parse(bad).is_err(), "{bad:?} should not parse");
    }
}

/// A parser that recurses has to have a floor, or a few kilobytes from the
/// network is a stack overflow.
#[test]
fn deep_nesting_is_refused_rather_than_overflowing_the_stack() {
    let deep = "[".repeat(5000) + &"]".repeat(5000);
    let err = json::parse(&deep).expect_err("should refuse");
    assert!(err.contains("deep"), "{err}");
}

#[test]
fn numbers_that_json_cannot_express_are_written_as_null() {
    // Rather than `inf`, which no reader on the other end would accept.
    assert_eq!(Json::Number(f64::INFINITY).write(), "null");
    assert_eq!(Json::Number(f64::NAN).write(), "null");
    assert_eq!(Json::Number(3.0).write(), "3", "whole numbers keep their shape");
}

// ---------------------------------------------------------------------------
// The protocol
// ---------------------------------------------------------------------------

#[test]
fn a_notification_is_told_apart_from_a_request() {
    use mcp::protocol::{parse, Incoming};
    // No id: nothing is owed.
    assert!(matches!(
        parse(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
        Incoming::Notification { .. }
    ));
    // A null id is still an id, and still owed an answer.
    assert!(matches!(
        parse(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#),
        Incoming::Request { .. }
    ));
}

#[test]
fn the_protocol_version_is_negotiated_rather_than_imposed() {
    use mcp::protocol::{initialize, SUPPORTED_VERSIONS};
    let asked = json::parse(r#"{"protocolVersion":"2024-11-05"}"#).unwrap();
    assert_eq!(initialize(&asked).str_field("protocolVersion"), Some("2024-11-05"));
    // Something we do not know gets our newest, not a refusal.
    let strange = json::parse(r#"{"protocolVersion":"1999-01-01"}"#).unwrap();
    assert_eq!(initialize(&strange).str_field("protocolVersion"), Some(SUPPORTED_VERSIONS[0]));
}

#[test]
fn every_tool_declares_a_schema_an_object_shape() {
    let listed = mcp::tools::list();
    let tools = listed.get("tools").unwrap().as_array().unwrap();
    assert_eq!(tools.len(), mcp::tools::TOOLS.len());
    for tool in tools {
        let name = tool.str_field("name").expect("a name");
        assert!(!tool.str_field("description").unwrap_or("").is_empty(), "{name} needs a description");
        let schema = tool.get("inputSchema").expect("a schema");
        assert_eq!(schema.str_field("type"), Some("object"), "{name}");
        assert!(schema.get("properties").is_some(), "{name} needs properties");
    }
}

// ---------------------------------------------------------------------------
// Over a socket
// ---------------------------------------------------------------------------

struct Server {
    port: u16,
    workspace: PathBuf,
}

/// Start a server on a port the OS chooses, so tests never collide.
fn start(name: &str, token: Option<&str>) -> Option<Server> {
    let workspace = std::env::temp_dir().join(format!("cshop-mcp-{name}"));
    let _ = std::fs::remove_dir_all(&workspace);
    std::fs::create_dir_all(&workspace).expect("make the workspace");

    // Port 0 asks the OS for a free one; bind here to learn which.
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = probe.local_addr().expect("addr").port();
    drop(probe);

    let config = mcp::server::Config {
        addr: ([127, 0, 0, 1], port).into(),
        workspace: workspace.clone(),
        token: token.map(str::to_string),
        allow_origins: Vec::new(),
    };
    std::thread::spawn(move || {
        let _ = mcp::server::serve(config);
    });

    // Wait for it, but not forever: a machine with no GPU never comes up, and
    // that is a skip rather than a failure.
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Some(Server { port, workspace });
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    None
}

struct Reply {
    status: u16,
    headers: String,
    body: String,
}

fn request(port: u16, head: &str, body: &str) -> Reply {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let message = format!(
        "{head}\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(message.as_bytes()).expect("write");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read");
    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let (headers, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
    Reply { status, headers: headers.to_string(), body: body.to_string() }
}

fn rpc(port: u16, session: &str, body: &str) -> Json {
    let head = format!("POST /mcp HTTP/1.1\r\nMcp-Session-Id: {session}");
    let reply = request(port, &head, body);
    assert_eq!(reply.status, 200, "{}", reply.body);
    json::parse(&reply.body).expect("a JSON body")
}

/// Call a tool and hand back its text and whether it reported an error.
fn tool(port: u16, session: &str, name: &str, arguments: &str) -> (String, bool) {
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
    );
    let result = rpc(port, session, &body);
    let result = result.get("result").expect("a result");
    let text = result
        .get("content")
        .and_then(Json::as_array)
        .and_then(|blocks| blocks.iter().find(|b| b.str_field("type") == Some("text")))
        .and_then(|b| b.str_field("text"))
        .unwrap_or("")
        .to_string();
    (text, result.get("isError").and_then(Json::as_bool).unwrap_or(false))
}

#[test]
fn a_client_can_shake_hands_and_call_a_tool() {
    let Some(server) = start("handshake", None) else { return };

    let hello = rpc(
        server.port,
        "t1",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
    );
    let result = hello.get("result").expect("a result");
    assert_eq!(result.str_field("protocolVersion"), Some("2025-06-18"));
    assert!(result.get("capabilities").and_then(|c| c.get("tools")).is_some());

    let listed = rpc(server.port, "t1", r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let tools = listed.get("result").unwrap().get("tools").unwrap().as_array().unwrap();
    assert!(tools.iter().any(|t| t.str_field("name") == Some("run_script")));

    let (text, failed) = tool(server.port, "t1", "run_script", r#"{"script":"new 40 30","return_image":false}"#);
    assert!(!failed, "{text}");
    assert!(text.contains("40x30"), "{text}");
}

/// The reason for holding a runner between calls at all.
#[test]
fn a_document_stays_open_across_calls_in_one_session() {
    let Some(server) = start("session", None) else { return };
    tool(server.port, "keep", "run_script", r#"{"script":"new 80 60","return_image":false}"#);
    let (text, failed) = tool(server.port, "keep", "run_script", r#"{"script":"info","return_image":false}"#);
    assert!(!failed, "{text}");
    assert!(text.contains("80x60"), "the document should still be open: {text}");

    // A different session must not see it.
    let (other, _) = tool(server.port, "elsewhere", "run_script", r#"{"script":"info","return_image":false}"#);
    assert!(!other.contains("80x60"), "sessions must not share a document: {other}");

    // And reset closes it.
    tool(server.port, "keep", "reset", "{}");
    let (after, _) = tool(server.port, "keep", "run_script", r#"{"script":"info","return_image":false}"#);
    assert!(!after.contains("80x60"), "reset should have closed it: {after}");
}

#[test]
fn a_result_can_carry_the_picture_it_drew() {
    let Some(server) = start("image", None) else { return };
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"run_script","arguments":{"script":"new 64 64 background=#ff0000","image_fit":32}}}"#;
    let result = rpc(server.port, "pic", body);
    let blocks = result.get("result").unwrap().get("content").unwrap().as_array().unwrap();
    let image = blocks
        .iter()
        .find(|b| b.str_field("type") == Some("image"))
        .expect("an image block");
    assert_eq!(image.str_field("mimeType"), Some("image/png"));
    let data = image.str_field("data").expect("base64");
    assert!(!data.is_empty());
    // Decodable, and really a PNG.
    let bytes = decode_base64(data);
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "the payload should be a PNG");
}

/// Only used to check what the server encoded, so a small decoder is enough.
fn decode_base64(text: &str) -> Vec<u8> {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = Vec::new();
    for c in text.bytes().filter(|c| *c != b'=') {
        bits.push(A.iter().position(|a| *a == c).expect("a base64 character") as u32);
    }
    let mut out = Vec::new();
    for chunk in bits.chunks(4) {
        let mut packed = 0u32;
        for (i, v) in chunk.iter().enumerate() {
            packed |= v << (18 - 6 * i);
        }
        let bytes = [(packed >> 16) as u8, (packed >> 8) as u8, packed as u8];
        out.extend_from_slice(&bytes[..chunk.len() - 1]);
    }
    out
}

// ---------------------------------------------------------------------------
// The guards
// ---------------------------------------------------------------------------

/// The whole reason a served editor is not simply the CLI with a port.
#[test]
fn a_script_cannot_read_or_write_outside_the_workspace() {
    let Some(server) = start("sandbox", None) else { return };

    // Something worth stealing, next to the workspace but not in it.
    let outside = server.workspace.parent().unwrap().join("cshop-mcp-sandbox-secret.txt");
    std::fs::write(&outside, b"secret").expect("write");

    for attempt in [
        "open ../cshop-mcp-sandbox-secret.txt",
        "open /etc/passwd",
        "open ~/.bashrc",
        "open a/../../elsewhere.png",
    ] {
        let arguments = format!(r#"{{"script":"{attempt}","return_image":false}}"#);
        let (text, failed) = tool(server.port, "s", "run_script", &arguments);
        assert!(failed, "{attempt:?} should have been refused: {text}");
        assert!(
            text.contains("workspace") || text.contains(".."),
            "{attempt:?} should say why: {text}"
        );
    }

    // Writing out is refused too, and really does not appear.
    let target = server.workspace.parent().unwrap().join("cshop-mcp-sandbox-pwned.png");
    let _ = std::fs::remove_file(&target);
    let (_, failed) = tool(
        server.port,
        "s",
        "run_script",
        r#"{"script":"new 10 10\nexport ../cshop-mcp-sandbox-pwned.png","return_image":false}"#,
    );
    assert!(failed, "writing outside should be refused");
    assert!(!target.exists(), "and must not have happened anyway");

    // While a path inside it works, so the confinement is not simply refusing.
    let (text, failed) = tool(
        server.port,
        "s",
        "run_script",
        r#"{"script":"new 10 10\nexport inside.png","return_image":false}"#,
    );
    assert!(!failed, "{text}");
    assert!(server.workspace.join("inside.png").exists());
}

#[test]
fn a_bearer_token_is_required_when_one_is_set() {
    let Some(server) = start("token", Some("open-sesame")) else { return };
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;

    let none = request(server.port, "POST /mcp HTTP/1.1", body);
    assert_eq!(none.status, 401);
    let wrong = request(
        server.port,
        "POST /mcp HTTP/1.1\r\nAuthorization: Bearer wrong",
        body,
    );
    assert_eq!(wrong.status, 401);
    let right = request(
        server.port,
        "POST /mcp HTTP/1.1\r\nAuthorization: Bearer open-sesame",
        body,
    );
    assert_eq!(right.status, 200, "{}", right.body);

    // Health stays open, so a load balancer need not hold the secret.
    let health = request(server.port, "GET /health HTTP/1.1", "");
    assert_eq!(health.status, 200);
}

/// Without this, any page the operator visits could drive their editor.
#[test]
fn a_foreign_browser_origin_is_refused() {
    let Some(server) = start("origin", None) else { return };
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;

    let evil = request(
        server.port,
        "POST /mcp HTTP/1.1\r\nOrigin: https://evil.example",
        body,
    );
    assert_eq!(evil.status, 403);

    for allowed in ["http://localhost:5173", "http://127.0.0.1:8080"] {
        let reply = request(
            server.port,
            &format!("POST /mcp HTTP/1.1\r\nOrigin: {allowed}"),
            body,
        );
        assert_eq!(reply.status, 200, "{allowed} should be allowed");
    }

    // No Origin at all is not a browser, and is left alone.
    let plain = request(server.port, "POST /mcp HTTP/1.1", body);
    assert_eq!(plain.status, 200);
}

#[test]
fn serving_beyond_loopback_without_a_token_is_refused_at_startup() {
    let config = mcp::server::Config {
        addr: ([0, 0, 0, 0], 0).into(),
        workspace: std::env::temp_dir().join("cshop-mcp-refuse"),
        token: None,
        allow_origins: Vec::new(),
    };
    let err = mcp::server::serve(config).expect_err("should refuse to start");
    assert!(err.contains("--token"), "the message should say how to fix it: {err}");
}

#[test]
fn a_notification_is_answered_with_no_body() {
    let Some(server) = start("notify", None) else { return };
    let reply = request(
        server.port,
        "POST /mcp HTTP/1.1",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    assert_eq!(reply.status, 202);
    assert!(reply.body.is_empty(), "a notification is owed nothing: {:?}", reply.body);
}

#[test]
fn a_body_larger_than_the_limit_is_refused_rather_than_read() {
    let Some(server) = start("toobig", None) else { return };
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("connect");
    // Claim far more than the cap, and send nothing: the length alone decides.
    let head = format!(
        "POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        mcp::http::MAX_BODY_BYTES + 1
    );
    stream.write_all(head.as_bytes()).expect("write");
    let mut raw = String::new();
    let _ = stream.read_to_string(&mut raw);
    assert!(raw.starts_with("HTTP/1.1 413"), "{raw:?}");
}

#[test]
fn nonsense_gets_a_protocol_error_rather_than_a_dropped_connection() {
    let Some(server) = start("nonsense", None) else { return };
    let reply = request(server.port, "POST /mcp HTTP/1.1", "this is not json");
    assert_eq!(reply.status, 200);
    let parsed = json::parse(&reply.body).expect("still valid JSON-RPC");
    assert_eq!(parsed.get("error").and_then(|e| e.get("code")).and_then(Json::as_f64), Some(-32700.0));

    let unknown = rpc(server.port, "n", r#"{"jsonrpc":"2.0","id":9,"method":"no/such"}"#);
    assert_eq!(unknown.get("error").and_then(|e| e.get("code")).and_then(Json::as_f64), Some(-32601.0));
}

#[test]
fn the_session_id_travels_back_on_the_handshake() {
    let Some(server) = start("sessionid", None) else { return };
    let reply = request(
        server.port,
        "POST /mcp HTTP/1.1",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    );
    let header = reply
        .headers
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("mcp-session-id:"))
        .expect("the handshake should hand back a session id");
    let id = header.split_once(':').unwrap().1.trim();
    assert!(id.len() >= 16, "an id worth having: {id:?}");
    assert!(id.chars().all(|c| c.is_ascii_alphanumeric()), "{id:?}");
}

#[test]
fn the_reference_answers_for_every_topic_it_advertises() {
    for topic in ["", "commands", "syntax", "filters", "adjustments", "effects", "blends"] {
        let text = mcp::reference::describe(topic);
        assert!(text.len() > 200, "{topic:?} came back nearly empty");
        assert!(!text.starts_with("no topic"), "{topic:?} should be known");
    }
    assert!(mcp::reference::describe("wibble").starts_with("no topic"));
}

/// A connection costs a thread, and a thread that is merely waiting still
/// costs a stack. Past the ceiling the server says so and closes, rather than
/// making threads until it cannot make any more.
#[test]
fn too_many_connections_are_refused_rather_than_served() {
    use std::io::{Read, Write};

    let Some(server) = start("connection-cap", None) else { return };

    // Hold open more than it will serve at once, without sending anything: an
    // idle connection is the cheapest way to occupy one, and with a two minute
    // read timeout it stays occupied.
    let mut held = Vec::new();
    for _ in 0..80 {
        match TcpStream::connect(("127.0.0.1", server.port)) {
            Ok(s) => held.push(s),
            Err(_) => break,
        }
    }
    assert!(held.len() > 64, "the test needs to get past the ceiling to mean anything");
    std::thread::sleep(std::time::Duration::from_millis(500));

    // One more. It should be answered and closed, not queued forever.
    let mut extra = TcpStream::connect(("127.0.0.1", server.port)).expect("connect");
    extra.set_read_timeout(Some(std::time::Duration::from_secs(5))).expect("timeout");
    let _ = extra.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    let mut said = String::new();
    let _ = extra.read_to_string(&mut said);
    assert!(
        said.contains("503"),
        "a server at capacity should say so: {said:?}"
    );

    // And when the crowd leaves, it serves again.
    drop(held);
    std::thread::sleep(std::time::Duration::from_millis(600));
    let answer = request(server.port, "GET /health HTTP/1.1", "");
    assert_eq!(answer.status, 200, "it should recover once there is room");
}
