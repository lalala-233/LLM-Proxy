/// Upstream API endpoint for chat completions.
pub const UPSTREAM_URL: &str = "https://api.siliconflow.cn/v1/chat/completions";
/// Models the proxy accepts from clients.
pub const ALLOWED_MODELS: &[&str] = &["Qwen/Qwen3-8B", "THUDM/GLM-4-9B-0414"];
/// Timeout in seconds for upstream HTTP requests.
pub const TIMEOUT_SECS: u64 = 60;
