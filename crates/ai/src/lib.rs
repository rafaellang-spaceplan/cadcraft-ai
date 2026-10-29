//! CADCraft AI: Claude reads an office briefing (free text or a PDF) into the space program the
//! layout generator plans with, and reviews the options it made.
//!
//! The requests go to the Claude Messages API over HTTPS with structured outputs, so the answers
//! are JSON that `cadcraft_spaceplan` reads directly. Credentials come from `ANTHROPIC_API_KEY`
//! (or `ANTHROPIC_AUTH_TOKEN`); `CADCRAFT_AI_MODEL`, `CADCRAFT_AI_EFFORT` and `ANTHROPIC_BASE_URL`
//! override the defaults. Everything the API returns is checked like any other outside input.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod schema;
#[cfg(not(target_arch = "wasm32"))]
mod transport;

pub use cadcraft_spaceplan as spaceplan;
use cadcraft_spaceplan::{FloorInfo, Program};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The model used unless `CADCRAFT_AI_MODEL` names another.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// Thinking effort unless `CADCRAFT_AI_EFFORT` says otherwise.
pub const DEFAULT_EFFORT: &str = "medium";
const API_VERSION: &str = "2026-10-21";
/// On a safety decline the API re-runs the request on the model Anthropic recommends for it.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const OAUTH_BETA: &str = "oauth-2025-04-20";
/// Largest briefing sent: text characters and PDF bytes (the API takes 32 MB per request).
pub const MAX_BRIEF_CHARS: usize = 200_000;
pub const MAX_PDF_BYTES: usize = 24 << 20;

/// Why a request didn't produce an answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// No `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN`.
    NoCredentials,
    /// Not available in this build (the web build has no HTTP client yet).
    Unavailable,
    /// The briefing is empty or too large.
    Brief(String),
    /// The request didn't reach the API or its answer didn't arrive.
    Network(String),
    /// The API answered with an error.
    Api { status: u16, kind: String, message: String },
    /// The model declined the request.
    Refused(String),
    /// The answer was cut off at the output limit.
    Truncated,
    /// The answer isn't what was asked for.
    BadAnswer(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoCredentials => write!(f, "no Claude API key: set ANTHROPIC_API_KEY"),
            Error::Unavailable => write!(f, "AI features are not available in this build yet"),
            Error::Brief(m) => write!(f, "briefing: {m}"),
            Error::Network(m) => write!(f, "could not reach the Claude API: {m}"),
            Error::Api { status, kind, message } => write!(f, "Claude API error {status} ({kind}): {message}"),
            Error::Refused(m) => write!(f, "the model declined the request{}", if m.is_empty() { String::new() } else { format!(": {m}") }),
            Error::Truncated => write!(f, "the answer was cut off"),
            Error::BadAnswer(m) => write!(f, "unexpected answer: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// How requests authenticate.
#[derive(Clone, Debug, PartialEq)]
pub enum Credentials {
    ApiKey(String),
    /// An OAuth access token.
    Bearer(String),
}

/// Where and how to send requests.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub credentials: Credentials,
    pub base_url: String,
    pub model: String,
    pub effort: String,
}

impl Config {
    /// From the environment; [`Error::NoCredentials`] without a key or token.
    pub fn from_env() -> Result<Config, Error> {
        let var = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let credentials = match (var("ANTHROPIC_API_KEY"), var("ANTHROPIC_AUTH_TOKEN")) {
            (Some(k), _) => Credentials::ApiKey(k),
            (None, Some(t)) => Credentials::Bearer(t),
            (None, None) => return Err(Error::NoCredentials),
        };
        Ok(Config {
            credentials,
            base_url: var("ANTHROPIC_BASE_URL").unwrap_or_else(|| "https://api.anthropic.com".into()),
            model: var("CADCRAFT_AI_MODEL").unwrap_or_else(|| DEFAULT_MODEL.into()),
            effort: var("CADCRAFT_AI_EFFORT").unwrap_or_else(|| DEFAULT_EFFORT.into()),
        })
    }

    /// The request headers: API version, betas and credentials.
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        let mut betas = vec![FALLBACK_BETA];
        let mut h = vec![("content-type", "application/json".to_string()), ("anthropic-version", API_VERSION.to_string())];
        match &self.credentials {
            Credentials::ApiKey(k) => h.push(("x-api-key", k.clone())),
            Credentials::Bearer(t) => {
                h.push(("authorization", format!("Bearer {t}")));
                betas.push(OAUTH_BETA);
            }
        }
        h.push(("anthropic-beta", betas.join(",")));
        h
    }

    /// The Messages endpoint.
    pub fn url(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }
}

/// A briefing: text, a PDF document, or both (the text then adds to the document).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Brief {
    pub text: String,
    pub pdf: Option<Vec<u8>>,
}

impl Brief {
    pub fn text(t: impl Into<String>) -> Self {
        Brief { text: t.into(), pdf: None }
    }

    fn check(&self) -> Result<(), Error> {
        if self.text.trim().is_empty() && self.pdf.as_ref().is_none_or(Vec::is_empty) {
            return Err(Error::Brief("it is empty".into()));
        }
        if self.text.chars().count() > MAX_BRIEF_CHARS {
            return Err(Error::Brief(format!("longer than {MAX_BRIEF_CHARS} characters")));
        }
        if self.pdf.as_ref().is_some_and(|p| p.len() > MAX_PDF_BYTES) {
            return Err(Error::Brief(format!("the PDF is larger than {} MB", MAX_PDF_BYTES >> 20)));
        }
        if self.pdf.as_ref().is_some_and(|p| !p.starts_with(b"%PDF")) {
            return Err(Error::Brief("the document is not a PDF".into()));
        }
        Ok(())
    }

    /// The user message's content blocks: the document first, then the text.
    fn blocks(&self, extra: &str) -> Vec<Value> {
        let mut blocks = Vec::new();
        if let Some(pdf) = &self.pdf {
            blocks.push(json!({ "type": "document", "source": { "type": "base64", "media_type": "application/pdf", "data": base64_encode(pdf) } }));
        }
        let mut text = String::new();
        if self.pdf.is_some() {
            text.push_str("The briefing is the attached document.\n");
        }
        if !self.text.trim().is_empty() {
            text.push_str(&format!("<briefing>\n{}\n</briefing>\n", self.text.trim()));
        }
        text.push_str(extra);
        blocks.push(json!({ "type": "text", "text": text }));
        blocks
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn base64_encode(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

#[cfg(target_arch = "wasm32")]
fn base64_encode(_b: &[u8]) -> String {
    String::new()
}

/// A briefing read into a program.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Interpretation {
    pub program: Program,
    /// What the briefing didn't settle and was assumed, one sentence each.
    pub assumptions: Vec<String>,
    /// Whether the program plausibly fits the floor (true when no floor was given).
    pub fits: bool,
}

/// The review of one option.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OptionReview {
    pub option: usize,
    pub pros: Vec<String>,
    pub cons: Vec<String>,
}

/// A review of the options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub recommended: usize,
    pub summary: String,
    pub options: Vec<OptionReview>,
}

fn floor_text(floor: Option<&FloorInfo>) -> String {
    match floor {
        Some(f) => format!(
            "\nThe floor plate: usable area {:.0} m², about {:.1} × {:.1} m, {} core(s), {} column(s).\n",
            f.usable_area, f.length, f.width, f.cores, f.columns
        ),
        None => "\nThe floor plate is not known yet.\n".into(),
    }
}

/// The request body for reading a briefing.
pub fn interpret_request(cfg: &Config, brief: &Brief, floor: Option<&FloorInfo>) -> Result<Value, Error> {
    brief.check()?;
    Ok(request(cfg, schema::INTERPRET_SYSTEM, brief.blocks(&floor_text(floor)), schema::program()))
}

/// The request body for reviewing options; `options` is the `spaceplan` command's result.
pub fn review_request(cfg: &Config, brief: &Brief, options: &Value) -> Result<Value, Error> {
    brief.check()?;
    let figures = serde_json::to_string(&summarize_options(options)).map_err(|e| Error::BadAnswer(e.to_string()))?;
    let extra = format!("\nThe generated options, best score first:\n{figures}\n");
    Ok(request(cfg, schema::REVIEW_SYSTEM, brief.blocks(&extra), schema::review()))
}

/// What the review needs of each option: its strategy and figures, without per-room detail.
fn summarize_options(result: &Value) -> Value {
    let opts = result.get("options").and_then(Value::as_array).cloned().unwrap_or_default();
    let list: Vec<Value> = opts
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let m = o.get("metrics").cloned().unwrap_or(Value::Null);
            let missing: Vec<Value> = m
                .get("spaces")
                .and_then(Value::as_array)
                .map(|s| s.iter().filter(|c| c.get("placed").and_then(Value::as_u64) < c.get("target").and_then(Value::as_u64)).cloned().collect())
                .unwrap_or_default();
            json!({
                "option": i + 1,
                "strategy": o.get("description"),
                "score": o.get("score"),
                "workstations": [m.get("workstations"), m.get("workstationsTarget")],
                "rooms": [m.get("rooms"), m.get("roomsTarget")],
                "notPlaced": missing,
                "daylight": m.get("daylight"),
                "areaPerPerson": m.get("areaPerPerson"),
                "circulationArea": m.get("circulationArea"),
            })
        })
        .collect();
    json!({ "usableArea": result.get("usableArea"), "options": list })
}

fn request(cfg: &Config, system: &str, content: Vec<Value>, schema: Value) -> Value {
    json!({
        "model": cfg.model,
        "max_tokens": 16000,
        "fallbacks": "default",
        "output_config": { "effort": cfg.effort, "format": { "type": "json_schema", "schema": schema } },
        "system": system,
        "messages": [{ "role": "user", "content": content }],
    })
}

/// The JSON answer in a Messages API response (status and body), or why there is none.
pub fn answer(status: u16, body: &str) -> Result<Value, Error> {
    let v: Value = serde_json::from_str(body).map_err(|_| {
        if (200..300).contains(&status) {
            Error::BadAnswer("the response is not JSON".into())
        } else {
            Error::Api { status, kind: "http".into(), message: snippet(body) }
        }
    })?;
    if !(200..300).contains(&status) {
        let e = v.get("error");
        let s = |k: &str| e.and_then(|e| e.get(k)).and_then(Value::as_str).unwrap_or("").to_string();
        return Err(Error::Api { status, kind: s("type"), message: s("message") });
    }
    match v.get("stop_reason").and_then(Value::as_str) {
        Some("refusal") => {
            let d = v.get("stop_details");
            let why =
                d.and_then(|d| d.get("explanation").and_then(Value::as_str).or_else(|| d.get("category").and_then(Value::as_str))).unwrap_or("");
            return Err(Error::Refused(why.to_string()));
        }
        Some("max_tokens") => return Err(Error::Truncated),
        _ => {}
    }
    let text: String = v
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err(Error::BadAnswer("no text in the response".into()));
    }
    serde_json::from_str(&text).map_err(|e| Error::BadAnswer(format!("the answer is not the JSON asked for ({e})")))
}

fn snippet(s: &str) -> String {
    s.chars().take(300).collect()
}

/// Check and read an interpretation answer.
pub fn read_interpretation(v: Value) -> Result<Interpretation, Error> {
    let mut program: Program = v
        .get("program")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| Error::BadAnswer(format!("program: {e}")))?
        .ok_or_else(|| Error::BadAnswer("no program".into()))?;
    program.validate().map_err(|e| Error::BadAnswer(e.to_string()))?;
    let assumptions = v
        .get("assumptions")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).take(50).map(|s| s.chars().take(400).collect()).collect())
        .unwrap_or_default();
    let fits = v.get("fits").and_then(Value::as_bool).unwrap_or(true);
    Ok(Interpretation { program, assumptions, fits })
}

/// Check and read a review answer against the number of options.
pub fn read_review(v: Value, options: usize) -> Result<Review, Error> {
    let mut r: Review = serde_json::from_value(v).map_err(|e| Error::BadAnswer(format!("review: {e}")))?;
    if r.recommended == 0 || r.recommended > options {
        return Err(Error::BadAnswer(format!("recommended option {} of {options}", r.recommended)));
    }
    r.options.retain(|o| o.option >= 1 && o.option <= options);
    Ok(r)
}

/// Read a briefing into a program (one request; may take a minute).
pub fn interpret(cfg: &Config, brief: &Brief, floor: Option<&FloorInfo>) -> Result<Interpretation, Error> {
    let body = interpret_request(cfg, brief, floor)?;
    read_interpretation(send(cfg, &body)?)
}

/// Review the options the `spaceplan` command made (its JSON result).
pub fn review(cfg: &Config, brief: &Brief, options: &Value) -> Result<Review, Error> {
    let n = options.get("options").and_then(Value::as_array).map_or(0, Vec::len);
    if n == 0 {
        return Err(Error::BadAnswer("no options to review".into()));
    }
    let body = review_request(cfg, brief, options)?;
    read_review(send(cfg, &body)?, n)
}

#[cfg(not(target_arch = "wasm32"))]
fn send(cfg: &Config, body: &Value) -> Result<Value, Error> {
    let (status, text) = transport::post(&cfg.url(), &cfg.headers(), &body.to_string())?;
    answer(status, &text)
}

#[cfg(target_arch = "wasm32")]
fn send(_cfg: &Config, _body: &Value) -> Result<Value, Error> {
    Err(Error::Unavailable)
}

#[cfg(test)]
mod tests;
