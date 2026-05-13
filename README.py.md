# LLM Proxy (Python)

[中文版](./README_ZH_CN.py.md) | [Rust version](./README.md)

> This is the **Python implementation**. A [Rust version](./README.md) with better performance, CLI support, and JSON configuration is also available.

An OpenAI-compatible proxy server that sits between your client and an upstream LLM API (default: SiliconFlow).

It works great with [Immersive Translate](https://www.immersivetranslate.net/).

Immersive Translate supports custom OpenAI-compatible APIs. Configure it to use your LLM Proxy instance:

- **API URL**: `http://localhost:8000/v1/chat/completions`
- **API Key**: your upstream API key
- **Model**: any model in the allowlist (e.g. `THUDM/GLM-4-9B-0414`)

## Why LLM Proxy

By default, Immersive Translate sends non-streaming requests to the upstream and cannot control whether thinking is enabled. For some models this introduces significant latency.

LLM Proxy **forces** `stream=true` and `enable_thinking=false` upstream, making translation responses faster.

## Quick Start

```bash
# Install dependencies
pip install aiohttp

# Start the proxy (default port 8000)
python proxy.py

# With a custom port
python proxy.py --port 8080

# Or use environment variable
PORT=8080 python proxy.py
```

The proxy exposes two endpoints:

| Endpoint | Method | Description |
| :-: | :-: | :-: |
| `/v1/chat/completions` | POST | Chat completion (OpenAI-compatible) |
| `/v1/models` | GET | List available models |

---

## How It Works

On every request the proxy:

1. **Validates**: the model against the allowlist (configurable) and the `Authorization` header.
2. **Rewrites**: the request body to set `stream: true` and `enable_thinking: false`.
3. **Forwards**: optional parameters (`temperature`, `max_tokens`, `top_p`, etc.) unchanged.
4. **Responds** based on the client's original `stream` value:
   - **Client requested stream=true** -> upstream SSE is forwarded as-is.
   - **Client requested stream=false** (or omitted it) -> chunks are collected internally and returned as a single non-streaming JSON response.

### Why force `enable_thinking=false` and `stream=true`?

Data from `compare.py` (100 requests per combination, round-robin, 16 concurrent workers):

| stream | enable_thinking | Mean | Median | P95 | P99 |
| :-: | :-: | :-: | :-: | :-: | :-: |
| false | false | 8.55s | 6.45s | 21.17s | 26.42s |
| false | true | 13.93s | 11.88s | 28.58s | 48.78s |
| true | false | 7.82s | 6.64s | 17.31s | 25.80s |
| true | true | 14.18s | 11.73s | 30.54s | 34.84s |

**Main-effect analysis:**

- `enable_thinking=false` -> **mean 8.18s**, `enable_thinking=true` -> **mean 14.05s** — **~5.9s penalty** for thinking.
- `stream=true` -> **mean 10.99s**, `stream=false` -> **mean 11.24s** — negligible difference.

Disabling thinking cuts latency by nearly 40%.

---

## Configuration

The proxy reads `config.json` from the current working directory. If the file is missing, built-in defaults are used.

```json
{
    "upstream": "https://api.siliconflow.cn/v1/chat/completions",
    "allowed_models": ["Qwen/Qwen3-8B", "THUDM/GLM-4-9B-0414"],
    "timeout": 60,
    "port": 8000
}
```

All fields are optional — omitted fields fall back to their defaults shown above.

> **Note:** `enable_thinking` is a SiliconFlow-specific parameter. This proxy was designed primarily for SiliconFlow. If you change `upstream` to another provider, check whether it supports (or ignores) this field — otherwise remove it from `build_upstream_body()` in `proxy.py`.

Change `upstream` to point at any OpenAI-compatible API. Update `ALLOWED_MODELS` to restrict which model names the proxy accepts.

### Port resolution

The listening port is determined in this order (first match wins):

1. `--port` / `-p` CLI flag
2. `PORT` environment variable
3. `port` field in `config.json`
4. Default: `8000`

When multiple sources specify different values, a warning is logged.

### CLI reference

```bash
llm-proxy --help
```

| Flag | Description | Default |
| :-: | :-: | :-: |
| `-c`, `--config` | Path to config file | `config.json` |
| `-p`, `--port` | Listening port | None |

## Latency Comparison Tool

The companion script `compare.py` can be used for latency comparison.

```bash
pip install -r requirements.txt

export SILICONFLOW_API_KEY="sk-..."
python compare.py
```

It sends 100 requests for each of the 4 (stream, enable_thinking) combinations, round-robin, with 16 concurrent workers.

Key constants you can tweak in `compare.py`:

| Constant | Default | Description |
| :-: | :-: | :-: |
| `REQUESTS_PER_COMBO` | 100 | Requests per combination |
| `MAX_WORKERS` | 16 | Thread pool size |
| `TIMEOUT` | 60 | Per-request timeout (seconds) |
| `MAX_RETRIES` | 3 | Retries for network errors |

---

## Acknowledgements

The default upstream is [SiliconFlow](https://siliconflow.cn), which provides many free models with generous rate limits (RPM 1000, TPM 50000).
