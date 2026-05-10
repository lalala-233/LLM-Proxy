//! LLM Proxy — An OpenAI-compatible proxy that sits between your client
//! and an upstream LLM API (default: SiliconFlow).
//!
//! Key behaviours:
//!   - Always sends stream=true + enable_thinking=false upstream
//!   - If the client asks for stream=true, the upstream SSE is forwarded
//!     as-is.
//!   - If the client asks for stream=false (or omits stream), chunks are
//!     collected internally and returned as a single non-streaming JSON.
//!
//! Why force stream=true + enable_thinking=false?
//!   - enable_thinking=false saves ~6 seconds per request on average.
//!   - stream=true lets the proxy start forwarding content immediately
//!     while still being able to emulate non-streaming responses.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tracing::{error, info};

// =============================================
// Configuration — edit these to fit your setup
// =============================================
const UPSTREAM_URL: &str = "https://api.siliconflow.cn/v1/chat/completions";
const ALLOWED_MODELS: [&str; 2] = ["Qwen/Qwen3-8B", "THUDM/GLM-4-9B-0414"];
const TIMEOUT_SECS: u64 = 60;

#[derive(Clone)]
struct AppState {
    client: reqwest::Client,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_upstream_body(body: &Value) -> Value {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or(ALLOWED_MODELS[0]);

    let mut upstream = json!({
        "model": model,
        "messages": body.get("messages").unwrap_or(&json!([])),
        "stream": true,
        "enable_thinking": false,
    });

    // Forward optional parameters unchanged
    for key in [
        "temperature",
        "max_tokens",
        "top_p",
        "frequency_penalty",
        "presence_penalty",
        "stop",
    ] {
        if let Some(val) = body.get(key) {
            upstream[key] = val.clone();
        }
    }

    upstream
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

async fn handle_chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    // Validate model against allowlist
    let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("");
    if !ALLOWED_MODELS.contains(&model) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!(
                    "Model '{model}' not allowed. Allowed: {}",
                    ALLOWED_MODELS.join(", ")
                )
            })),
        )
            .into_response();
    }

    // Validate authorization
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !auth_header.starts_with("Bearer ") || auth_header.len() < 15 {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Missing or invalid Authorization header"})),
        )
            .into_response();
    }

    let client_wants_stream = body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let upstream_payload = build_upstream_body(&body);
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

    // Build upstream request headers
    let mut req_headers = HeaderMap::new();
    req_headers.insert("Content-Type", "application/json".parse().unwrap());
    req_headers.insert("Authorization", auth_header.parse().unwrap());

    let upstream_result = state
        .client
        .post(UPSTREAM_URL)
        .headers(req_headers)
        .json(&upstream_payload)
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .send()
        .await;

    let upstream_resp = match upstream_result {
        Ok(r) => r,
        Err(e) => {
            if e.is_timeout() {
                error!("Upstream request timed out");
                return (
                    StatusCode::GATEWAY_TIMEOUT,
                    Json(json!({"error": "Upstream request timed out"})),
                )
                    .into_response();
            }
            error!("Upstream connection error: {e}");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("Upstream connection error: {e}")})),
            )
                .into_response();
        }
    };

    let status = upstream_resp.status();
    if status != StatusCode::OK {
        let err_text = upstream_resp.text().await.unwrap_or_default();
        error!("Upstream error {status}: {err_text}");
        return (
            status,
            Json(json!({"error": format!("Upstream returned {status}")})),
        )
            .into_response();
    }

    if client_wants_stream {
        passthrough_stream(upstream_resp).await
    } else {
        collect_and_return(upstream_resp, model).await
    }
}

async fn passthrough_stream(upstream: reqwest::Response) -> Response {
    let stream = upstream
        .bytes_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));

    let body = axum::body::Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .header("X-Accel-Buffering", "no")
        .body(body)
        .unwrap()
}

async fn collect_and_return(upstream: reqwest::Response, default_model: &str) -> Response {
    let mut buf = Vec::<u8>::new();
    let mut collected_content = String::new();
    let mut collected_reasoning = String::new();
    let mut finish_reason: Option<String> = None;
    let mut response_id: Option<String> = None;
    let mut model_name: Option<String> = None;
    let mut chunk_count = 0u32;
    let mut done = false;

    let mut stream = upstream.bytes_stream();

    while let Some(chunk_result) = stream.next().await {
        let chunk = match chunk_result {
            Ok(c) => c,
            Err(e) => {
                error!("Error reading upstream stream: {e}");
                break;
            }
        };
        buf.extend_from_slice(&chunk);

        // Process as many complete lines as possible from the buffer
        'lines: loop {
            // Find newline position
            let nl_pos = match buf.iter().position(|&b| b == b'\n') {
                Some(p) => p,
                None => break 'lines,
            };

            // Extract line (excluding the newline byte)
            let raw: Vec<u8> = buf.drain(..=nl_pos).collect();
            let line = String::from_utf8_lossy(&raw[..nl_pos]).trim().to_string();

            if line.is_empty() || line.starts_with(':') {
                continue 'lines;
            }
            if !line.starts_with("data: ") {
                continue 'lines;
            }

            let data_str = line[6..].trim();
            if data_str == "[DONE]" {
                done = true;
                break 'lines;
            }

            if let Ok(chunk) = serde_json::from_str::<Value>(data_str) {
                chunk_count += 1;

                if response_id.is_none() {
                    response_id = chunk.get("id").and_then(|v| v.as_str()).map(String::from);
                }
                if model_name.is_none() {
                    model_name = chunk
                        .get("model")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                }

                if let Some(choices) = chunk.get("choices").and_then(|v| v.as_array())
                    && let Some(choice) = choices.first()
                {
                    if let Some(delta) = choice.get("delta") {
                        if let Some(c) = delta.get("content").and_then(|v| v.as_str()) {
                            collected_content.push_str(c);
                        }
                        if let Some(r) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
                            collected_reasoning.push_str(r);
                        }
                    }
                    if let Some(fr) = choice.get("finish_reason").and_then(|v| v.as_str()) {
                        finish_reason = Some(fr.to_string());
                    }
                }
            }
        }

        if done {
            break;
        }
    }

    info!(
        chunk_count = chunk_count,
        content_len = collected_content.len(),
        reasoning_len = collected_reasoning.len(),
        "[<- collected]"
    );

    let ts = now_ts();

    let mut assistant_msg = json!({
        "role": "assistant",
        "content": collected_content
    });
    if !collected_reasoning.is_empty() {
        assistant_msg["reasoning_content"] = json!(collected_reasoning);
    }

    let full_response = json!({
        "id": response_id.unwrap_or_else(|| format!("proxy-{ts}")),
        "object": "chat.completion",
        "created": ts,
        "model": model_name.unwrap_or_else(|| default_model.to_string()),
        "choices": [{
            "index": 0,
            "message": assistant_msg,
            "finish_reason": finish_reason.unwrap_or_else(|| "stop".to_string())
        }]
    });

    Json(full_response).into_response()
}

async fn handle_models() -> Json<Value> {
    let ts = now_ts();
    Json(json!({
        "object": "list",
        "data": ALLOWED_MODELS.iter().map(|id| {
            json!({
                "id": id,
                "object": "model",
                "created": ts,
                "owned_by": "llm-proxy"
            })
        }).collect::<Vec<_>>()
    }))
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .build()
        .expect("Failed to build HTTP client");

    let state = AppState { client };

    let app = Router::new()
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route("/v1/models", get(handle_models))
        .with_state(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "8000".to_string());
    let addr = format!("0.0.0.0:{port}");

    info!("{}", "=".repeat(50));
    info!("LLM Proxy starting:   http://{}", addr);
    info!("Allowed models:       {}", ALLOWED_MODELS.join(", "));
    info!("Upstream URL:         {}", UPSTREAM_URL);
    info!("{}", "=".repeat(50));

    let listener = TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind {addr}: {e}"));
    axum::serve(listener, app)
        .await
        .unwrap_or_else(|e| panic!("Server error: {e}"));
}
