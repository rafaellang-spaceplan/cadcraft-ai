//! HTTPS POST to the Claude API (native builds).

use std::time::Duration;

use crate::Error;

/// Largest response read.
const MAX_RESPONSE_BYTES: u64 = 8 << 20;

/// POST `body`; returns the status and the response text. HTTP error statuses are answers, not
/// transport errors.
pub fn post(url: &str, headers: &[(&'static str, String)], body: &str) -> Result<(u16, String), Error> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        // Long briefings with thinking can take minutes.
        .timeout_global(Some(Duration::from_secs(600)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.post(url);
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    log::info!("Claude API request: {} bytes", body.len());
    let mut resp = req.send(body).map_err(|e| Error::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().with_config().limit(MAX_RESPONSE_BYTES).read_to_string().map_err(|e| Error::Network(e.to_string()))?;
    log::info!("Claude API response: {status}, {} bytes", text.len());
    Ok((status, text))
}
