//! JSON-RPC 2.0, and the handful of methods the Model Context Protocol adds.
//!
//! The protocol is small: a client says hello, asks what tools there are, and
//! calls them. Everything interesting is in [`super::tools`]; this file only
//! moves envelopes.
//!
//! One distinction matters and is easy to get wrong: a request has an `id` and
//! is owed a response, a notification has none and must be answered with
//! nothing at all. Replying to a notification is a protocol error, not a
//! harmless extra.

use super::json::Json;

/// The protocol revisions this server knows how to speak, newest first.
///
/// A client asking for one of these gets it back; a client asking for anything
/// else is told what we do speak and may carry on or give up. Refusing outright
/// would be worse — the negotiation exists precisely so that a version gap need
/// not be fatal.
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

pub const SERVER_NAME: &str = "cshop";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a caller should know before its first tool call.
pub const INSTRUCTIONS: &str = "\
C-Shop is a layered image editor driven by a script. `run_script` is the whole \
editor; the other tools exist so you can find your way around before using it.

Work in this order. Call `workspace` to see what you may open, `list_styles` \
and `describe` to see what you can do, then `run_script` to do it. A document \
stays open between calls, so build the picture up in steps and look at the \
image that comes back rather than composing the whole thing blind.

Two habits worth keeping. Use `measure text` before placing type, so it is \
positioned from its real size rather than a guess. And judge fine texture on a \
full-size `export` rather than on the picture returned here, which is scaled \
down and will flatter hatching, grain and banding that do not survive at size.";

pub enum Incoming {
    Request { id: Json, method: String, params: Json },
    Notification { method: String },
    Invalid { id: Json, code: i64, message: String },
}

/// Read one JSON-RPC message.
pub fn parse(body: &str) -> Incoming {
    let value = match super::json::parse(body) {
        Ok(value) => value,
        Err(why) => {
            return Incoming::Invalid {
                id: Json::Null,
                code: -32700,
                message: format!("could not parse the request: {why}"),
            }
        }
    };

    // Batches were removed from the protocol; saying so is friendlier than
    // failing to find a `method` in an array.
    if value.as_array().is_some() {
        return Incoming::Invalid {
            id: Json::Null,
            code: -32600,
            message: "batched requests are not part of this protocol; send one at a time".into(),
        };
    }

    let id = value.get("id").cloned().unwrap_or(Json::Null);
    let Some(method) = value.str_field("method") else {
        return Incoming::Invalid { id, code: -32600, message: "no `method`".into() };
    };
    let method = method.to_string();
    let params = value.get("params").cloned().unwrap_or(Json::Object(Vec::new()));

    // No id at all means a notification. An id of null is a request with a
    // null id, which is legal and must still be answered.
    if value.get("id").is_none() {
        Incoming::Notification { method }
    } else {
        Incoming::Request { id, method, params }
    }
}

pub fn result(id: Json, value: Json) -> String {
    Json::object(vec![
        ("jsonrpc", Json::from("2.0")),
        ("id", id),
        ("result", value),
    ])
    .write()
}

pub fn error(id: Json, code: i64, message: &str) -> String {
    Json::object(vec![
        ("jsonrpc", Json::from("2.0")),
        ("id", id),
        (
            "error",
            Json::object(vec![
                ("code", Json::Number(code as f64)),
                ("message", Json::from(message)),
            ]),
        ),
    ])
    .write()
}

/// The `initialize` result.
pub fn initialize(params: &Json) -> Json {
    let asked = params.str_field("protocolVersion").unwrap_or("");
    let version = if SUPPORTED_VERSIONS.contains(&asked) {
        asked
    } else {
        SUPPORTED_VERSIONS[0]
    };
    Json::object(vec![
        ("protocolVersion", Json::from(version)),
        (
            "capabilities",
            Json::object(vec![(
                "tools",
                // The tool list is fixed for the life of the process, so there
                // is nothing to notify anyone about.
                Json::object(vec![("listChanged", Json::from(false))]),
            )]),
        ),
        (
            "serverInfo",
            Json::object(vec![
                ("name", Json::from(SERVER_NAME)),
                ("title", Json::from("C-Shop")),
                ("version", Json::from(SERVER_VERSION)),
            ]),
        ),
        ("instructions", Json::from(INSTRUCTIONS)),
    ])
}
