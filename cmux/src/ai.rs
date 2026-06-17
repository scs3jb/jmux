//! Optional AI workspace auto-naming via the Anthropic Messages API.
//!
//! Uses the caller's `ANTHROPIC_API_KEY` (and optional `ANTHROPIC_BASE_URL`).
//! HTTP is blocking (ureq) — only call this from a socket worker thread, never
//! the GTK main thread.

use serde_json::Value;

/// Fast, cheap model — titling doesn't need a frontier model.
const MODEL: &str = "claude-haiku-4-5-20251001";

/// Generate a concise 2–5 word workspace title from a terminal/agent transcript.
/// Returns a human-readable error string on any failure (no key, network, etc.).
pub fn generate_workspace_title(transcript: &str) -> Result<String, String> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "ANTHROPIC_API_KEY is not set".to_string())?;
    let base = std::env::var("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|_| "https://api.anthropic.com".to_string());

    // Keep the last ~6000 chars (most recent context matters most), on a char
    // boundary.
    let convo = transcript.trim();
    let convo = if convo.len() > 6000 {
        let target = convo.len() - 6000;
        let start = (target..convo.len())
            .find(|i| convo.is_char_boundary(*i))
            .unwrap_or(convo.len());
        &convo[start..]
    } else {
        convo
    };
    if convo.is_empty() {
        return Err("nothing to name (empty transcript)".to_string());
    }

    let body = serde_json::json!({
        "model": MODEL,
        "max_tokens": 24,
        "system": "You name a developer's terminal workspace. Given a transcript of an \
                   agent/terminal session, reply with ONLY a concise 2-5 word Title Case \
                   name for the task. No quotes, no trailing punctuation, no preamble.",
        "messages": [{ "role": "user", "content": convo }],
    });

    let resp = ureq::post(format!("{base}/v1/messages"))
        .header("x-api-key", api_key.as_str())
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .send_json(&body)
        .map_err(|e| format!("Anthropic request failed: {e}"))?;

    let json: Value = resp
        .into_body()
        .read_json()
        .map_err(|e| format!("Could not parse Anthropic response: {e}"))?;

    let title = json
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|a| a.iter().find_map(|b| b.get("text").and_then(|t| t.as_str())))
        .ok_or_else(|| {
            json.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|m| m.to_string())
                .unwrap_or_else(|| "No title in Anthropic response".to_string())
        })?;

    let title = title.trim().trim_matches('"').trim().to_string();
    if title.is_empty() {
        return Err("Anthropic returned an empty title".to_string());
    }
    Ok(title)
}
