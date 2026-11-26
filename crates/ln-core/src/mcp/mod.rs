//! MCP server for Claude Code, Claude Desktop and other MCP clients (`docs/mcp.md`).
//!
//! Streamable HTTP at `/mcp` on the clip endpoint ([`crate::clip`]): same 127.0.0.1 binding,
//! bearer token and origin/host checks. Every POST carries one JSON-RPC message and gets a plain
//! JSON answer (no SSE stream, no sessions); notifications get `202`. The tools only call into
//! [`Core`], so they run on the same index, notes folder and models as the open app.

mod tools;

#[cfg(test)]
mod tests;

use crate::Core;
use serde_json::{Value, json};
use std::sync::Arc;

/// Protocol revisions this server speaks, newest first.
pub(crate) const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

const INSTRUCTIONS: &str = "Live Notes keeps the user's Markdown notes (meeting transcripts, summaries, \
clipped web pages and videos) in a local folder. Use search_notes to find notes, read_note to read one \
in full, create_note to write a new note, and clip_url to save a web page or YouTube video as a \
summarized note. Paths are relative to the notes folder.";

/// `false` for a `MCP-Protocol-Version` header naming a revision this server doesn't speak.
pub(crate) fn protocol_header_ok(v: Option<&str>) -> bool {
    v.is_none_or(|v| PROTOCOL_VERSIONS.contains(&v.trim()))
}

pub(crate) fn parse_error(msg: &str) -> Value {
    rpc_error(Value::Null, PARSE_ERROR, msg)
}

fn rpc_error(id: Value, code: i64, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Answers one JSON-RPC message. `None` for notifications and responses (HTTP `202`).
pub(crate) async fn handle(core: &Arc<Core>, msg: Value) -> Option<Value> {
    let Some(obj) = msg.as_object() else {
        // Batches were dropped from the protocol in 2025-06-18.
        return Some(rpc_error(Value::Null, INVALID_REQUEST, "expected one JSON-RPC message object"));
    };
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(rpc_error(obj.get("id").cloned().unwrap_or(Value::Null), INVALID_REQUEST, "jsonrpc must be \"2.0\""));
    }
    let method = obj.get("method").and_then(Value::as_str);
    let id = obj.get("id").cloned();
    let (Some(method), Some(id)) = (method, id) else {
        // A notification (`notifications/initialized`, `notifications/cancelled`) or a response.
        return None;
    };
    let params = obj.get("params").cloned().unwrap_or_else(|| json!({}));
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
            let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOL_VERSIONS[0]);
            rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "live-notes", "title": "Live Notes", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, json!({ "tools": tools::list() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(rpc_error(id, INVALID_PARAMS, "tools/call needs a tool name"));
            };
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            match tools::call(core, name, args).await {
                Some(result) => rpc_result(id, result),
                None => rpc_error(id, INVALID_PARAMS, &format!("unknown tool: {name}")),
            }
        }
        _ => rpc_error(id, METHOD_NOT_FOUND, &format!("method not found: {method}")),
    })
}
