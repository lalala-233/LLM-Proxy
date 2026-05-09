#!/usr/bin/env python3
"""
LLM Proxy — An OpenAI-compatible proxy that sits between your client
and an upstream LLM API (default: SiliconFlow).

Key behaviours:
  - Always sends stream=true + enable_thinking=false upstream, which
    cuts latency significantly (see bench.py / result.md for data).
  - If the client asks for stream=true, the upstream SSE is forwarded
    as-is.
  - If the client asks for stream=false (or omits stream), chunks are
    collected internally and returned as a single non-streaming JSON.

Why force stream=true + enable_thinking=false?
  - enable_thinking=false saves ~6 seconds per request on average
    (based on 400 real requests across 4 combinations).
  - stream=true lets the proxy start forwarding content immediately
    while still being able to emulate non-streaming responses.
"""

import json
import logging
import os
import time
from typing import Optional

import asyncio

import aiohttp
from aiohttp import web

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)
logger = logging.getLogger(__name__)

# =============================================
# Configuration — edit these to fit your setup
# =============================================
UPSTREAM_URL = "https://api.siliconflow.cn/v1/chat/completions"
ALLOWED_MODELS = ["Qwen/Qwen3-8B", "THUDM/GLM-4-9B-0414"]


def build_upstream_body(body: dict) -> dict:
    """Build the upstream request body with forced parameters."""
    upstream = {
        "model": body.get("model", ALLOWED_MODELS[0]),
        "messages": body.get("messages", []),
        "stream": True,
        "enable_thinking": False,
    }
    # Forward optional parameters unchanged
    for key in (
        "temperature",
        "max_tokens",
        "top_p",
        "frequency_penalty",
        "presence_penalty",
        "stop",
    ):
        if key in body:
            upstream[key] = body[key]
    return upstream


async def handle_chat_completions(request: web.Request) -> web.Response:
    # CORS preflight
    if request.method == "OPTIONS":
        return web.Response(
            status=200,
            headers={
                "Access-Control-Allow-Origin": "*",
                "Access-Control-Allow-Methods": "POST, OPTIONS",
                "Access-Control-Allow-Headers": "Content-Type, Authorization",
            },
        )

    if request.method != "POST":
        return web.json_response({"error": "Method not allowed"}, status=405)

    # Parse request body
    try:
        body = await request.json()
    except json.JSONDecodeError:
        return web.json_response({"error": "Invalid JSON"}, status=400)

    # Validate model against allowlist
    model = body.get("model", "")
    if model not in ALLOWED_MODELS:
        allowed_str = ", ".join(ALLOWED_MODELS)
        return web.json_response(
            {"error": f"Model '{model}' not allowed. Allowed: {allowed_str}"},
            status=400,
        )

    # Validate authorization
    auth_header = request.headers.get("Authorization", "")
    if not auth_header.startswith("Bearer ") or len(auth_header) < 15:
        return web.json_response(
            {"error": "Missing or invalid Authorization header"},
            status=401,
        )

    client_wants_stream = body.get("stream", False)

    upstream_payload = build_upstream_body(body)

    headers = {
        "Content-Type": "application/json",
        "Authorization": auth_header,
    }

    msg_count = len(upstream_payload.get("messages", []))
    logger.info(
        f"[-> upstream] model={model} msgs={msg_count} "
        f"client_stream={client_wants_stream}"
    )

    try:
        async with aiohttp.ClientSession(
            timeout=aiohttp.ClientTimeout(total=120),
        ) as session:
            async with session.post(
                UPSTREAM_URL,
                headers=headers,
                json=upstream_payload,
            ) as resp:
                if resp.status != 200:
                    err_text = await resp.text()
                    logger.error(f"[upstream error] {resp.status}: {err_text[:500]}")
                    return web.json_response(
                        {"error": f"Upstream returned {resp.status}"},
                        status=resp.status,
                    )

                if client_wants_stream:
                    return await passthrough_stream(resp)

                return await collect_and_return(resp, model)

    except asyncio.TimeoutError:
        logger.error("Upstream request timed out")
        return web.json_response({"error": "Upstream request timed out"}, status=504)
    except aiohttp.ClientError as e:
        logger.error(f"Upstream connection error: {e}")
        return web.json_response(
            {"error": f"Upstream connection error: {str(e)}"}, status=502
        )


async def passthrough_stream(
    upstream_resp: aiohttp.ClientResponse,
) -> web.StreamResponse:
    """Forward the upstream SSE stream to the client unchanged."""
    stream_resp = web.StreamResponse(
        status=200,
        headers={
            "Content-Type": "text/event-stream; charset=utf-8",
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
            "X-Accel-Buffering": "no",
            "Access-Control-Allow-Origin": "*",
        },
    )
    await stream_resp.prepare()

    try:
        async for chunk in upstream_resp.content:
            if chunk:
                await stream_resp.write(chunk)
    except Exception as e:
        logger.error(f"Stream passthrough error: {e}")
    finally:
        await stream_resp.write_eof()

    return stream_resp


async def collect_and_return(
    upstream_resp: aiohttp.ClientResponse,
    model: str,
) -> web.Response:
    """Collect streaming chunks and return a standard non-streaming JSON response."""
    collected_content = ""
    collected_reasoning = ""
    finish_reason: Optional[str] = None
    response_id: Optional[str] = None
    model_name: Optional[str] = None
    chunk_count = 0

    while True:
        raw_line = await upstream_resp.content.readline()
        if not raw_line:
            break
        line = raw_line.decode("utf-8", errors="replace").strip()
        if not line or line.startswith(":"):
            continue
        if not line.startswith("data: "):
            continue

        data_str = line[6:]
        if data_str.strip() == "[DONE]":
            break

        try:
            chunk = json.loads(data_str)
            chunk_count += 1
        except json.JSONDecodeError:
            continue

        if response_id is None:
            response_id = chunk.get("id")
        if model_name is None:
            model_name = chunk.get("model")

        choices = chunk.get("choices", [])
        if choices:
            delta = choices[0].get("delta", {})
            collected_content += delta.get("content", "")
            collected_reasoning += delta.get("reasoning_content", "")
            if choices[0].get("finish_reason"):
                finish_reason = choices[0]["finish_reason"]

    logger.info(
        f"[<- collected] {chunk_count} chunks | "
        f"content={len(collected_content)} chars | "
        f"reasoning={len(collected_reasoning)} chars"
    )

    assistant_msg = {"role": "assistant", "content": collected_content}
    if collected_reasoning:
        assistant_msg["reasoning_content"] = collected_reasoning

    full_response = {
        "id": response_id or f"proxy-{int(time.time())}",
        "object": "chat.completion",
        "created": int(time.time()),
        "model": model_name or model,
        "choices": [
            {
                "index": 0,
                "message": assistant_msg,
                "finish_reason": finish_reason or "stop",
            }
        ],
    }

    return web.json_response(full_response)


async def handle_models(request: web.Request) -> web.Response:
    """OpenAI-compatible model list endpoint."""
    return web.json_response(
        {
            "object": "list",
            "data": [
                {
                    "id": model_id,
                    "object": "model",
                    "created": int(time.time()),
                    "owned_by": "llm-proxy",
                }
                for model_id in ALLOWED_MODELS
            ],
        }
    )


async def cors_preflight(request: web.Request) -> web.Response:
    return web.Response(
        status=200,
        headers={
            "Access-Control-Allow-Origin": "*",
            "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
            "Access-Control-Allow-Headers": "Content-Type, Authorization",
        },
    )


def create_app() -> web.Application:
    app = web.Application()

    for path in ("/v1/chat/completions", "/v1/models", "/{tail:.*}"):
        app.router.add_route("OPTIONS", path, cors_preflight)

    app.router.add_post("/v1/chat/completions", handle_chat_completions)
    app.router.add_get("/v1/models", handle_models)

    return app


if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8000"))
    logger.info("=" * 50)
    logger.info(f"LLM Proxy starting → http://localhost:{port}")
    logger.info(f"Allowed models:      {', '.join(ALLOWED_MODELS)}")
    logger.info(f"Upstream URL:        {UPSTREAM_URL}")
    logger.info("=" * 50)
    web.run_app(create_app(), host="0.0.0.0", port=port)
