# LLM Proxy (Rust)

[English](./README.md) | [Python 版本](./README_ZH_CN.py.md)

一个 OpenAI 兼容的代理服务器，位于客户端与上游 LLM API（默认是 SiliconFlow）之间。这是 **Rust 实现**——作为 [Python 版本](./README_ZH_CN.py.md)的编译型高性能替代方案。

可搭配[沉浸式翻译](https://www.immersivetranslate.net/)使用。

沉浸式翻译支持自定义 OpenAI 兼容接口。将其配置为使用您的 LLM Proxy 实例：

- **API URL**: `http://localhost:8000/v1/chat/completions`
- **API Key**: 您的上游 API Key
- **Model**: 白名单中的任一模型（如 `THUDM/GLM-4-9B-0414`）

## 与 Python 版本的区别

| | Rust | Python |
| :-: | - | - |
| 运行环境 | 编译为单个二进制文件 | 需要 Python |
| 配置方式 | JSON 配置文件（`config.json`） | 直接编辑 `proxy.py` 或 JSON 配置文件 |
| 命令行 | `--config`/`-c`、`--port`/`-p` | 与 Rust 版本一致 |
| 端口优先级 | `--port` > `PORT` 环境变量 > config > 8000（默认） | 与 Rust 版本一致 |
| 架构 | 异步（tokio + axum） | 异步（aiohttp） |

Rust 版本可直接替换 Python 版本，除启动方式与日志外，API 与行为完全一致。

## 为什么要使用 LLM Proxy

默认情况下，沉浸式翻译对上游发出的请求不是流式的，也不能指定思考是否开启，这对于一些模型来说增大了延迟。

LLM Proxy **强制上游使用** `stream=true` 和 `enable_thinking=false`，使翻译响应更快。

## 从源码安装

```bash
git clone https://github.com/lalala-233/LLM-Proxy
cd LLM-Proxy
cargo install --path .
```

## 快速开始

```bash
# 启动代理（默认端口 8000）
llm-proxy

# 自定义端口
llm-proxy --port 8080

# 或使用环境变量
PORT=8080 llm-proxy
```

代理暴露两个端点：

| 端点 | 方法 | 说明 |
| :-: | :-: | :-: |
| `/v1/chat/completions` | POST | 聊天补全（OpenAI 兼容） |
| `/v1/models` | GET | 列出可用模型 |

---

## 工作原理

每次请求代理会执行以下操作：

1. **校验**：模型是否在白名单中（可配置），以及 `Authorization` 头是否有效。
2. **重写**：请求体，强制设置 `stream: true` 和 `enable_thinking: false`。
3. **透传**：可选参数（`temperature`、`max_tokens`、`top_p` 等）不变。
4. **根据客户端的原始 `stream` 值决定响应方式：**
   - **客户端请求 stream=true** -> 直接转发上游 SSE 流。
   - **客户端请求 stream=false**（或省略）-> 内部收集各 chunk，组装成标准非流式 JSON 返回。

### 为什么强制 `enable_thinking=false` 且 `stream=true`？

以下是使用 `compare.py` 获得的统计数据（每种组合 100 次，轮询调度，16 并发）：

| stream | enable_thinking | 均值 | 中位数 | P95 | P99 |
| :-: | :-: | :-: | :-: | :-: | :-: |
| false | false | 8.55s | 6.45s | 21.17s | 26.42s |
| false | true | 13.93s | 11.88s | 28.58s | 48.78s |
| true | false | 7.82s | 6.64s | 17.31s | 25.80s |
| true | true | 14.18s | 11.73s | 30.54s | 34.84s |

**主效应分析：**

- `enable_thinking=false` -> **均值 8.18s**，`enable_thinking=true` -> **均值 14.05s** — **开启思考带来约 5.9s 延迟惩罚**。
- `stream=true` -> **均值 10.99s**，`stream=false` -> **均值 11.24s**，差异可忽略。

关闭思考可以减少近 40% 的延迟。

---

## 配置

代理会从当前工作目录读取 `config.json`。若文件不存在，则使用默认值。

```json
{
    "upstream": "https://api.siliconflow.cn/v1/chat/completions",
    "allowed_models": ["Qwen/Qwen3-8B", "THUDM/GLM-4-9B-0414"],
    "timeout": 60,
    "port": 8000
}
```

所有字段均为可选，缺失字段会回退到上方列出的默认值。

> **注意：**`enable_thinking` 是 SiliconFlow 的特有参数。本代理主要为 SiliconFlow 设计。若将 `upstream` 改为其他提供商，请先确认对方是否支持（或会忽略）该字段——否则需从 `src/proxy.rs` 的 `build_upstream_body()` 中移除它。

修改 `upstream` 指向任意 OpenAI 兼容的 API。修改 `allowed_models` 来限制代理接受的模型名称。

代理默认监听 `8000` 端口，也可通过 `PORT` 环境变量覆盖

### 端口解析

监听端口按以下优先级确定：

1. `--port` / `-p` 命令行参数
2. `PORT` 环境变量
3. `config.json` 中的 `port` 字段
4. 默认值：`8000`

当多个来源指定不同值时，会输出警告。

### CLI 参考

```bash
llm-proxy --help
```

| 参数 | 说明 | 默认值 |
| :-: | :-: | :-: |
| `-c`, `--config` | 配置文件路径 | `config.json` |
| `-p`, `--port` | 监听端口 | 无 |

## 延迟对比工具

配套脚本 `compare.py` 可用于进行延迟比较。

```bash
pip install -r requirements.txt

export SILICONFLOW_API_KEY="sk-..."
python compare.py
```

它针对 4 种（stream, enable_thinking）组合各发送 100 次请求，轮询调度，16 并发。

可在 `compare.py` 中调整的关键常量：

| 常量 | 默认值 | 说明 |
| :-: | :-: | :-: |
| `REQUESTS_PER_COMBO` | 100 | 每种组合的请求数 |
| `MAX_WORKERS` | 16 | 线程池大小 |
| `TIMEOUT` | 60 | 单请求超时时间（秒） |
| `MAX_RETRIES` | 3 | 网络错误重试次数 |

---

## 致谢

默认上游为 [SiliconFlow](https://siliconflow.cn)，它提供了许多免费模型，并附有慷慨的频率限制（RPM 1000，TPM 50000）。
