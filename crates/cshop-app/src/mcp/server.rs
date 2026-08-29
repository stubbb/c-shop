//! Binding the socket, and deciding who may talk to it.
//!
//! The script language can read and write files, so this server is a
//! filesystem primitive with a port in front of it. Three things stand between
//! those two facts, and all three are on by default:
//!
//! * **A workspace.** Every path a script names resolves inside one directory
//!   and cannot leave it. There is no way to turn this off.
//! * **Loopback by default.** Serving anywhere else has to be asked for, and
//!   asking for it requires a token — a server reachable from the network
//!   without one will refuse to start rather than come up unprotected.
//! * **An origin check.** A page in a browser can post to localhost; without
//!   this, any site the operator visits could drive their editor. Requests
//!   carrying a foreign `Origin` are refused.

use std::net::{IpAddr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::editor::{Editor, Outcome, Work};
use super::http::{self, Request, Response};
use super::json::Json;
use super::protocol::{self, Incoming};
use super::tools;
use crate::script::Sandbox;

pub struct Config {
    pub addr: SocketAddr,
    pub workspace: PathBuf,
    pub token: Option<String>,
    /// Extra `Origin` values to accept beyond the loopback ones.
    pub allow_origins: Vec<String>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            addr: SocketAddr::from(([127, 0, 0, 1], 7333)),
            workspace: PathBuf::from("."),
            token: None,
            allow_origins: Vec::new(),
        }
    }
}

struct Server {
    editor: Editor,
    workspace: Sandbox,
    token: Option<String>,
    allow_origins: Vec<String>,
}

/// Bring the server up and serve until killed.
pub fn serve(config: Config) -> Result<(), String> {
    // The rule that matters most, checked before anything is bound: an editor
    // that can be reached from the network must be able to say no to someone.
    if !is_loopback(&config.addr) && config.token.is_none() {
        return Err(format!(
            "refusing to serve on {} without --token.\n\
             This server can read and write files in its workspace, so exposing it \
             beyond localhost without one would hand that to anyone who can reach \
             the port. Either pass --token SECRET, or bind to 127.0.0.1.",
            config.addr
        ));
    }

    let workspace = Sandbox::new(&config.workspace)?;
    let editor = Editor::start(workspace.clone())?;
    let listener = TcpListener::bind(config.addr)
        .map_err(|e| format!("could not bind {}: {e}", config.addr))?;
    let bound = listener.local_addr().unwrap_or(config.addr);

    log::info!("serving MCP on http://{bound}/mcp");
    log::info!("workspace: {}", workspace.root().display());
    if config.token.is_some() {
        log::info!("a bearer token is required");
    } else {
        log::info!("no token; reachable only from this machine");
    }

    let server = Server {
        editor,
        workspace,
        token: config.token,
        allow_origins: config.allow_origins,
    };
    http::serve(listener, move |request| server.route(request));
    Ok(())
}

fn is_loopback(addr: &SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_loopback(),
    }
}

impl Server {
    fn route(&self, request: &Request) -> Response {
        match (request.method.as_str(), request.route()) {
            ("POST", "/mcp") => self.guarded(request, |s, r| s.rpc(r)),
            // Server-initiated streams are not offered. The protocol allows
            // saying so, and it keeps this to one request, one answer.
            ("GET", "/mcp") => Response::text(405, "this server does not open SSE streams")
                .with_header("Allow", "POST, DELETE"),
            ("DELETE", "/mcp") => self.guarded(request, |s, r| s.end_session(r)),
            ("GET", "/health") => self.health(),
            ("GET", "/") => Response::text(200, &self.greeting()),
            ("OPTIONS", _) => self.preflight(),
            _ => Response::text(404, "not found; the endpoint is /mcp"),
        }
    }

    /// Apply the checks that every real endpoint shares.
    fn guarded(&self, request: &Request, then: impl Fn(&Server, &Request) -> Response) -> Response {
        if let Some(refusal) = self.check_origin(request) {
            return refusal;
        }
        if let Some(refusal) = self.check_token(request) {
            return refusal;
        }
        then(self, request)
    }

    /// Refuse a request carrying an `Origin` we do not know.
    ///
    /// Without this, any web page the operator happens to visit could post to
    /// their loopback port and drive the editor. A request with no `Origin` at
    /// all is not from a browser and is left alone.
    fn check_origin(&self, request: &Request) -> Option<Response> {
        let origin = request.header("origin")?;
        let known = origin_is_local(origin)
            || self.allow_origins.iter().any(|allowed| allowed == origin || allowed == "*");
        if known {
            return None;
        }
        log::warn!("refused a request from origin {origin:?}");
        Some(Response::text(
            403,
            "this origin is not allowed; pass --allow-origin to permit it",
        ))
    }

    fn check_token(&self, request: &Request) -> Option<Response> {
        let expected = self.token.as_deref()?;
        let given = request
            .header("authorization")
            .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
            .unwrap_or("");
        if constant_time_eq(given.as_bytes(), expected.as_bytes()) {
            return None;
        }
        Some(
            Response::text(401, "a bearer token is required")
                .with_header("WWW-Authenticate", "Bearer"),
        )
    }

    fn rpc(&self, request: &Request) -> Response {
        let session = self.session_of(request);
        let body = request.body_str();

        match protocol::parse(&body) {
            Incoming::Notification { method } => {
                log::debug!("notification: {method}");
                // Nothing is owed for a message with no id.
                Response::accepted()
            }
            Incoming::Invalid { id, code, message } => {
                Response::json(200, protocol::error(id, code, &message))
            }
            Incoming::Request { id, method, params } => {
                let answer = match method.as_str() {
                    "initialize" => Ok(protocol::initialize(&params)),
                    "ping" => Ok(Json::Object(Vec::new())),
                    "tools/list" => Ok(tools::list()),
                    "tools/call" => self.call_tool(&params, &session),
                    other => Err(format!("no method called {other:?}")),
                };
                let payload = match answer {
                    Ok(value) => protocol::result(id, value),
                    Err(why) => protocol::error(id, -32601, &why),
                };
                let response = Response::json(200, payload);
                // A new session's id has to reach the client that started it.
                if method == "initialize" {
                    response.with_header("Mcp-Session-Id", &session)
                } else {
                    response
                }
            }
        }
    }

    fn call_tool(&self, params: &Json, session: &str) -> Result<Json, String> {
        let Some(name) = params.str_field("name") else {
            return Err("tools/call needs a `name`".into());
        };
        let empty = Json::Object(Vec::new());
        let arguments = params.get("arguments").unwrap_or(&empty);
        let result = tools::call(&self.editor, &self.workspace, name, arguments, session);
        Ok(result.to_content())
    }

    fn end_session(&self, request: &Request) -> Response {
        let session = self.session_of(request);
        match self.editor.submit(&session, Work::Reset, None) {
            Outcome::Done(note) => Response::text(200, &note),
            _ => Response::text(200, "session ended"),
        }
    }

    fn health(&self) -> Response {
        let sessions = match self.editor.submit("", Work::Sessions, None) {
            Outcome::Sessions(list) => list,
            _ => Vec::new(),
        };
        let body = Json::object(vec![
            ("ok", Json::from(true)),
            ("server", Json::from(protocol::SERVER_NAME)),
            ("version", Json::from(protocol::SERVER_VERSION)),
            (
                "protocolVersions",
                Json::Array(
                    protocol::SUPPORTED_VERSIONS.iter().map(|v| Json::from(*v)).collect(),
                ),
            ),
            ("tools", Json::from(tools::TOOLS.len())),
            ("workspace", Json::from(self.workspace.root().display().to_string())),
            ("requiresToken", Json::from(self.token.is_some())),
            (
                "sessions",
                Json::Array(
                    sessions
                        .iter()
                        .map(|s| {
                            Json::object(vec![
                                ("id", Json::from(s.id.clone())),
                                (
                                    "document",
                                    match &s.document {
                                        None => Json::Null,
                                        Some((name, w, h)) => Json::object(vec![
                                            ("name", Json::from(name.clone())),
                                            ("width", Json::from(*w)),
                                            ("height", Json::from(*h)),
                                        ]),
                                    },
                                ),
                                ("layers", Json::from(s.layers)),
                                ("idleSeconds", Json::Number(s.idle_seconds as f64)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]);
        Response::json(200, body.write())
    }

    fn greeting(&self) -> String {
        format!(
            "C-Shop {version} — a layered image editor, over MCP.\n\n\
             Endpoint   POST /mcp   (JSON-RPC 2.0, protocol {protocol})\n\
             Health     GET  /health\n\
             Tools      {tools}\n\
             Workspace  {workspace}\n\
             Auth       {auth}\n\n\
             Try it:\n\
             \x20 curl -s localhost:PORT/mcp -H 'Content-Type: application/json' \\\n\
             \x20   -d '{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}}'\n",
            version = protocol::SERVER_VERSION,
            protocol = protocol::SUPPORTED_VERSIONS[0],
            tools = tools::TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", "),
            workspace = self.workspace.root().display(),
            auth = if self.token.is_some() {
                "Authorization: Bearer <token>"
            } else {
                "none (loopback only)"
            },
        )
    }

    /// Answer a browser's preflight, so a permitted origin can actually call.
    fn preflight(&self) -> Response {
        Response::new(204, "", Vec::new())
            .with_header("Access-Control-Allow-Methods", "POST, GET, DELETE, OPTIONS")
            .with_header(
                "Access-Control-Allow-Headers",
                "Content-Type, Authorization, Mcp-Session-Id, MCP-Protocol-Version",
            )
            .with_header("Access-Control-Expose-Headers", "Mcp-Session-Id")
            .with_header("Access-Control-Max-Age", "600")
    }

    /// Which document this request is about.
    ///
    /// A client that manages sessions sends the header it was given at
    /// `initialize`. One that does not gets a single shared document, which is
    /// the right default for the common case of one operator and one editor.
    fn session_of(&self, request: &Request) -> String {
        match request.header("mcp-session-id") {
            Some(id) if is_sane_session_id(id) => id.to_string(),
            _ => new_session_id(),
        }
    }
}

/// Session ids are only identifiers, never credentials — the token is what
/// grants access. They are still made hard to guess so that two clients
/// sharing a server cannot stumble into each other's documents.
fn new_session_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = process_seed()
        ^ COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
    format!("{:016x}{:016x}", mix(seed), mix(seed ^ 0xD1B5_4A32_D192_ED03))
}

/// Sixty-four bits from the operating system, once per process.
///
/// Falls back to the clock if there is no such device, which is weaker but
/// still better than a constant — and the token, not this, is the security
/// boundary.
fn process_seed() -> u64 {
    use std::sync::OnceLock;
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        let mut bytes = [0u8; 8];
        if std::io::Read::read_exact(
            &mut std::fs::File::open("/dev/urandom").ok().unwrap_or_else(|| {
                // Unreachable on the platforms this runs on; the fallback
                // below covers it rather than panicking here.
                std::fs::File::open("/dev/null").expect("a null device")
            }),
            &mut bytes,
        )
        .is_ok()
        {
            return u64::from_le_bytes(bytes);
        }
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x5DEE_CE66)
    })
}

/// splitmix64, which is enough to turn a counter into something unguessable.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A session id we are willing to keep in a map.
///
/// Bounded and printable, so a hostile header cannot become an unbounded key
/// or smuggle control characters into the log.
fn is_sane_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn origin_is_local(origin: &str) -> bool {
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .unwrap_or(origin);
    let host = rest.split(['/', ':']).next().unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

/// Compare without leaking, through timing, how much of a token was right.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut differences = 0u8;
    for (x, y) in a.iter().zip(b) {
        differences |= x ^ y;
    }
    differences == 0
}
