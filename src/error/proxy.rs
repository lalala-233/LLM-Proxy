use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::json;
/// Unified error type for the LLM Proxy.
///
/// Each variant maps to an appropriate HTTP status code and error message.
#[derive(Debug)]
pub enum ProxyError {
    /// The requested model is not in the allowlist.
    ModelNotAllowed {
        /// The model the client requested.
        model: String,
        /// The allowed models (from configuration).
        allowed: Vec<String>,
    },
    /// The `Authorization` header is missing or invalid.
    Unauthorized,
    /// The upstream request exceeded the configured timeout.
    UpstreamTimeout,
    /// A network-level failure when connecting to the upstream.
    UpstreamConnection(String),
    /// The upstream returned a non-200 status.
    UpstreamResponse { status: StatusCode, body: String },
    /// An error occurred while reading the upstream SSE stream.
    StreamRead(String),
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::ModelNotAllowed { model, allowed } => (
                StatusCode::BAD_REQUEST,
                format!(
                    "Model '{model}' not allowed. Allowed: {}",
                    allowed.join(", ")
                ),
            ),
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "Missing or invalid Authorization header".into(),
            ),
            Self::UpstreamTimeout => (
                StatusCode::GATEWAY_TIMEOUT,
                "Upstream request timed out".into(),
            ),
            Self::UpstreamConnection(e) => (
                StatusCode::BAD_GATEWAY,
                format!("Upstream connection error: {e}"),
            ),
            Self::UpstreamResponse { status, body } => {
                (status, format!("Upstream returned {status}: {body}"))
            }
            Self::StreamRead(e) => (StatusCode::BAD_GATEWAY, format!("Stream read error: {e}")),
        };
        (status, Json(json!({"error": message}))).into_response()
    }
}
