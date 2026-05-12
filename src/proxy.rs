use crate::{config::Config, error::ProxyError};
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::{error, info};

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub client: Client,
    pub config: Config,
}

// ---------------------------------------------------------------------------
// Request helpers
// ---------------------------------------------------------------------------

/// Force `stream=true` + `enable_thinking=false` upstream while forwarding
fn build_upstream_body(mut body: Value, model: &str) -> Value {
    const WHITELIST_KEYS: &[&str] = &[
        "temperature",
        "max_tokens",
        "top_p",
        "frequency_penalty",
        "presence_penalty",
        "stop",
    ];
    let obj = body.as_object_mut().expect("body must be a JSON object");

    let messages = obj.remove("messages").unwrap_or(json!([]));
    let mut upstream = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "enable_thinking": false,
    });

    for key in WHITELIST_KEYS {
        if let Some(val) = obj.remove(*key) {
            upstream[key] = val;
        }
    }

    upstream
}

/// Unix timestamp in seconds.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

async fn handle_chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, ProxyError> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string)
        .ok_or_else(|| ProxyError::ModelNotAllowed {
            model: "(missing)".to_string(),
            allowed: state.config.allowed_models.clone(),
        })?;

    if !state.config.allowed_models.contains(&model) {
        return Err(ProxyError::ModelNotAllowed {
            model,
            allowed: state.config.allowed_models.clone(),
        });
    }

    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !auth_header.starts_with("Bearer ") || auth_header.len() < 15 {
        return Err(ProxyError::Unauthorized);
    }

    let client_wants_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);

    let upstream_payload = build_upstream_body(body, &model);
    let msg_count = upstream_payload
        .get("messages")
        .and_then(|v| v.as_array())
        .map_or(0, Vec::len);

    info!(
        model = %model,
        msgs = msg_count,
        client_stream = client_wants_stream,
        "[-> upstream]"
    );

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    headers.insert(
        "Authorization",
        auth_header.parse().or(Err(ProxyError::Unauthorized))?,
    );

    let response = state
        .client
        .post(&state.config.upstream)
        .headers(headers)
        .json(&upstream_payload)
        .timeout(Duration::from_secs(state.config.timeout))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                ProxyError::UpstreamTimeout
            } else {
                ProxyError::UpstreamConnection(e.to_string())
            }
        })?;

    let status = response.status();
    if status != StatusCode::OK {
        let err_text = response.text().await.unwrap_or_default();
        error!("Upstream error {status}: {err_text}");
        return Err(ProxyError::UpstreamResponse {
            status,
            body: err_text,
        });
    }

    if client_wants_stream {
        Ok(passthrough_stream(response))
    } else {
        collect_and_return(response, &model).await
    }
}

/// Forward the upstream SSE byte stream directly to the client.
fn passthrough_stream(upstream: reqwest::Response) -> Response {
    let stream = upstream
        .bytes_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .header("X-Accel-Buffering", "no")
        .body(axum::body::Body::from_stream(stream))
        .expect("valid response with streaming body")
}

/// Consume the upstream SSE stream and assemble a single non-streaming JSON response.
async fn collect_and_return(
    upstream: reqwest::Response,
    model: &str,
) -> Result<Response, ProxyError> {
    #[derive(Deserialize)]
    struct Chunk {
        id: String,
        choices: Vec<Choice>,
    }
    #[derive(Deserialize)]
    struct Choice {
        delta: Delta,
        finish_reason: Option<String>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Delta {
        content: String,
        reasoning_content: String,
    }

    let mut stream = upstream.bytes_stream();
    let mut buf = Vec::new(); // buffer for partial lines across chunks
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut chunk_count = 0u32;
    let mut response_id = None;
    let mut finish_reason = None;

    while let Some(chunk_result) = stream.next().await {
        const PREFIX: &str = "data: ";
        let chunk = chunk_result.map_err(|e| ProxyError::StreamRead(e.to_string()))?;
        buf.extend_from_slice(&chunk);

        // Process complete lines from buffer
        while let Some(position) = buf.iter().position(|&b| b == b'\n') {
            let line_bytes = buf.drain(..=position);
            let line = String::from_utf8_lossy(&line_bytes.as_slice()[..position]);
            let line = line.trim();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            if !line.starts_with(PREFIX) {
                continue;
            }

            let payload = line[PREFIX.len()..].trim();
            if payload == "[DONE]" {
                break;
            }
            if let Ok(chunk) = serde_json::from_str::<Chunk>(payload) {
                chunk_count += 1;
                if response_id.is_none() {
                    response_id = Some(chunk.id);
                }
                if let Some(choice) = chunk.choices.into_iter().next() {
                    content.push_str(&choice.delta.content);
                    reasoning.push_str(&choice.delta.reasoning_content);
                    if finish_reason.is_none() {
                        finish_reason = choice.finish_reason;
                    }
                }
            }
        }
    }

    info!(
        chunk_count = chunk_count,
        content_len = content.len(),
        reasoning_len = reasoning.len(),
        "[<- collected]"
    );

    let timestamp = now();
    let mut assistant_msg = json!({
        "role": "assistant",
        "content": content,
    });
    if !reasoning.is_empty() {
        assistant_msg["reasoning_content"] = json!(reasoning);
    }

    let final_response = json!({
        "id": response_id.unwrap_or_else(|| format!("proxy-{timestamp}")),
        "object": "chat.completion",
        "created": timestamp,
        "model": model,
        "choices": [{
            "index": 0,
            "message": assistant_msg,
            "finish_reason": finish_reason.unwrap_or_else(|| "stop".to_string()),
        }],
    });

    Ok(Json(final_response).into_response())
}

async fn handle_models(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "object": "list",
        "data": state.config.allowed_models.iter().map(|id| {
            json!({
                "id": id,
                "object": "model",
                "created": now(),
                "owned_by": "llm-proxy",
            })
        }).collect::<Vec<_>>()
    }))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route("/v1/models", get(handle_models))
        .with_state(Arc::new(state))
}
