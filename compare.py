#!/usr/bin/env python3
"""
LLM Proxy — Latency Comparison Tool

This script sends requests in all 4 combinations of
(stream, enable_thinking) to demonstrate why the proxy
forces stream=true + enable_thinking=false upstream.

Key finding from real runs (see result.md):
  - enable_thinking=false saves ~6 seconds per request on average
  - stream=true vs stream=false makes little difference (~0.2s)

It is NOT a generic benchmark — it is a proof-of-concept
that validates the proxy's default behaviour.
"""

import os
import time
import statistics
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed
import requests
from requests.adapters import HTTPAdapter
from urllib3.util.retry import Retry

# =============================================
# Configuration
# =============================================
API_URL = "https://api.siliconflow.cn/v1/chat/completions"
API_KEY = os.environ.get("SILICONFLOW_API_KEY", "YOUR_API_KEY_HERE")
MODEL = "Pro/zai-org/GLM-4.7"
MESSAGES = [
    {"role": "system", "content": "You are a helpful assistant"},
    {"role": "user", "content": "Hello, please introduce yourself"},
]
REQUESTS_PER_COMBO = 100
MAX_WORKERS = 16
TIMEOUT = 60
MAX_RETRIES = 3
RETRY_BACKOFF = 1.0

HEADERS = {"Content-Type": "application/json", "Authorization": f"Bearer {API_KEY}"}

# Four (stream, thinking) combinations
COMBOS = [
    {"stream": False, "enable_thinking": False, "name": "S_F_ T_F"},
    {"stream": False, "enable_thinking": True, "name": "S_F_ T_T"},
    {"stream": True, "enable_thinking": False, "name": "S_T_ T_F"},
    {"stream": True, "enable_thinking": True, "name": "S_T_ T_T"},
]

print_lock = threading.Lock()


def safe_print(*args, **kwargs):
    with print_lock:
        print(*args, **kwargs)


def create_session() -> requests.Session:
    """Create a requests.Session with retry strategy and a large connection pool."""
    session = requests.Session()
    retry_strategy = Retry(
        total=MAX_RETRIES,
        backoff_factor=RETRY_BACKOFF,
        status_forcelist=[429, 500, 502, 503, 504],
        allowed_methods=["POST"],
        raise_on_status=False,
    )
    adapter = HTTPAdapter(
        max_retries=retry_strategy,
        pool_connections=50,
        pool_maxsize=50,
    )
    session.mount("https://", adapter)
    session.mount("http://", adapter)
    return session


def send_request(
    stream: bool, enable_thinking: bool, request_id: int, combo_name: str
) -> dict:
    """Send one request with its own session (thread-safe), retry on network errors."""
    payload = {
        "model": MODEL,
        "messages": MESSAGES,
        "stream": stream,
        "enable_thinking": enable_thinking,
    }
    start_time = time.perf_counter()
    last_error = None
    session = create_session()

    for attempt in range(1, MAX_RETRIES + 1):
        try:
            with session.post(
                API_URL, headers=HEADERS, json=payload, stream=stream, timeout=TIMEOUT
            ) as resp:
                resp.raise_for_status()
                if stream:
                    for _ in resp.iter_content(chunk_size=None):
                        pass
                else:
                    resp.json()
            elapsed = time.perf_counter() - start_time
            session.close()
            return {
                "request_id": request_id,
                "stream": stream,
                "enable_thinking": enable_thinking,
                "combo_name": combo_name,
                "success": True,
                "total_time": elapsed,
                "error": None,
            }
        except Exception as e:
            last_error = e
            error_str = str(e)
            is_network_error = any(
                phrase in error_str
                for phrase in [
                    "NameResolutionError",
                    "Failed to resolve",
                    "ConnectionError",
                    "Timeout",
                    "Max retries exceeded",
                    "Connection refused",
                    "Connection reset",
                ]
            )
            if attempt < MAX_RETRIES and is_network_error:
                wait = RETRY_BACKOFF * (2 ** (attempt - 1))
                safe_print(
                    f"[ID:{request_id:4d}] {combo_name:15s} "
                    f"network error, retry in {wait:.1f}s "
                    f"(attempt {attempt}/{MAX_RETRIES}): {error_str[:80]}"
                )
                time.sleep(wait)
                continue
            else:
                break

    session.close()
    elapsed = time.perf_counter() - start_time
    return {
        "request_id": request_id,
        "stream": stream,
        "enable_thinking": enable_thinking,
        "combo_name": combo_name,
        "success": False,
        "total_time": elapsed,
        "error": str(last_error),
    }


def generate_tasks():
    """Generate a round-robin task list so combos are interleaved fairly."""
    tasks = []
    counts = {idx: 0 for idx in range(len(COMBOS))}
    global_id = 1
    while any(counts[idx] < REQUESTS_PER_COMBO for idx in range(len(COMBOS))):
        for idx, combo in enumerate(COMBOS):
            if counts[idx] < REQUESTS_PER_COMBO:
                tasks.append(
                    (
                        global_id,
                        combo["stream"],
                        combo["enable_thinking"],
                        f"{combo['name']}#{counts[idx] + 1}",
                    )
                )
                global_id += 1
                counts[idx] += 1
    return tasks


def print_stats(times: list, label: str):
    if not times:
        print(f"{label}: no successful requests")
        return
    sorted_times = sorted(times)
    print(f"=== {label} ===")
    print(f"  Success: {len(times)} / {REQUESTS_PER_COMBO}")
    print(f"  Mean:    {statistics.mean(times):.3f}s")
    print(f"  Median:  {statistics.median(times):.3f}s")
    print(f"  P95:     {sorted_times[int(0.95 * len(sorted_times))]:.3f}s")
    print(f"  P99:     {sorted_times[int(0.99 * len(sorted_times))]:.3f}s")
    print(f"  Min:     {min(times):.3f}s")
    print(f"  Max:     {max(times):.3f}s")


def main():
    if API_KEY == "YOUR_API_KEY_HERE":
        print(
            "Error: Set SILICONFLOW_API_KEY or replace the placeholder in the script."
        )
        return

    tasks = generate_tasks()
    total = len(tasks)
    print(f"Total requests: {total} ({REQUESTS_PER_COMBO} per combo, round-robin)")
    print(f"Concurrency: {MAX_WORKERS}, max retries: {MAX_RETRIES}")
    print("Starting...\n")

    results = []
    start_batch = time.perf_counter()

    with ThreadPoolExecutor(max_workers=MAX_WORKERS) as executor:
        future_map = {}
        for rid, stream, thinking, combo_name in tasks:
            future = executor.submit(send_request, stream, thinking, rid, combo_name)
            future_map[future] = (rid, combo_name)

        for future in as_completed(future_map):
            rid, combo_name = future_map[future]
            result = future.result()
            results.append(result)
            if result["success"]:
                safe_print(
                    f"[ID:{rid:4d}] {result['combo_name']:15s} "
                    f"elapsed: {result['total_time']:.3f}s"
                )
            else:
                safe_print(
                    f"[ID:{rid:4d}] {result['combo_name']:15s} "
                    f"FAILED: {result['error'][:80]}"
                )

    elapsed = time.perf_counter() - start_batch
    print(f"\nAll done. Wall-clock: {elapsed:.2f}s\n")

    # Group by combo name (strip the #N suffix)
    combo_times = {combo["name"]: [] for combo in COMBOS}
    for r in results:
        if r["success"]:
            key = r["combo_name"].split("#")[0]
            combo_times[key].append(r["total_time"])

    for combo in COMBOS:
        times = combo_times[combo["name"]]
        label = f"stream={combo['stream']}, thinking={combo['enable_thinking']}"
        print_stats(times, label)

    # Main-effect analysis
    all_ok = [r for r in results if r["success"]]
    if all_ok:
        s_true = [r["total_time"] for r in all_ok if r["stream"]]
        s_false = [r["total_time"] for r in all_ok if not r["stream"]]
        t_true = [r["total_time"] for r in all_ok if r["enable_thinking"]]
        t_false = [r["total_time"] for r in all_ok if not r["enable_thinking"]]

        print("\n--- Main-effect analysis (mean comparison) ---")
        if s_true and s_false:
            avg_s_t = statistics.mean(s_true)
            avg_s_f = statistics.mean(s_false)
            print(f"stream=true   mean: {avg_s_t:.3f}s")
            print(f"stream=false  mean: {avg_s_f:.3f}s")
            print(f"delta: {avg_s_f - avg_s_t:+.3f}s")
        if t_true and t_false:
            avg_t_t = statistics.mean(t_true)
            avg_t_f = statistics.mean(t_false)
            print(f"\nenable_thinking=true   mean: {avg_t_t:.3f}s")
            print(f"enable_thinking=false  mean: {avg_t_f:.3f}s")
            print(f"delta: {avg_t_t - avg_t_f:+.3f}s")


if __name__ == "__main__":
    main()
