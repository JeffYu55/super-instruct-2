# Super-Instruct

面向 Codex 兼容 API 的本地 Headless 上下文网关。

Super-Instruct 运行在 Codex 客户端与上游模型服务之间，对 JSON 请求进行任务识别、契约构建、阶段编排和 Skill 路由；对 SSE/JSON 响应进行协议解析、结果分类、质量评估、记忆筛选与结构化审计。普通请求保持透明转发，复杂研究请求则升级为可恢复的多轮会话。

当前主程序是纯 Rust 命令行服务，不依赖旧版 Tauri 前端。

## 核心能力

- **本地反向代理**：基于 Axum 和 Reqwest，保留原始方法、路径、查询参数与认证头，支持流式和非流式响应。
- **协议适配**：识别 OpenAI Responses API 与 Chat Completions 风格请求，统一处理消息、工具调用、工具输出和续传游标。
- **Contract-First 路由**：从最新用户回合提取意图、交付物、动作、验收条件和严格对齐要求。
- **动态 Skill 注入**：按任务类别、动作和当前阶段选择仓库内的 Skill，而不是将全部内容注入每个请求。
- **阶段化研究会话**：将复杂任务拆为 framing、planning、evidence、analysis、transformation、execution、verification 和 reporting。
- **证据图与检查点**：接收工具输出，以 SHA-256 去重并写入会话检查点；进程重启后可以恢复。
- **质量与结果分类**：综合上游状态、内容完整度、证据覆盖、动作覆盖和验证问题生成结构化结果。
- **文件化可观测性**：记录 JSONL 交互、能力矩阵、原始异常响应、CoT trace 和成功记忆。
- **Codex 配置托管**：启动时备份并接管 Codex 配置，部署 bridge 与 Skills；停止时恢复用户原配置和同名 Skill。
- **本地工具发现**：检测 PATH 中的 strings、objdump、radare2/r2、Ghidra、JADX、Apktool 和 Frida，并写入任务上下文。

## 架构

```text
Codex / API Client
        |
        v
Axum Gateway (127.0.0.1:8080)
        |
        +-- Request extraction
        +-- Category + action routing
        +-- Task contract + stage plan
        +-- bridge.md + routed Skills injection
        +-- research session / evidence graph
        |
        v
Upstream model provider
        |
        v
Protocol adapter + SSE/JSON parser
        |
        +-- outcome classification
        +-- quality gate
        +-- memory gate
        +-- JSONL / raw response / CoT audit
        |
        v
Client response
```

非 JSON 请求直接透传。普通 JSON 请求只经过单轮管线；需要持续采证或多阶段执行的请求进入研究会话编排器。

## 请求分类与路由

网关将请求划分为五个顶层类别：

| 类别 | 典型处理 |
| --- | --- |
| `general` | 普通对话、通用实现与无需研究编排的请求 |
| `reverse` | 二进制、字节码、协议与逆向分析 |
| `crack` | 授权逻辑、序列算法、补丁与保护分析 |
| `pentest` | 网络、Web、无线和漏洞评估工作流 |
| `malware-analysis` | 样本分析、行为、检测、仿真或开发类请求 |

Router 根据用户动作生成 DAG，并选择 profile、Skill 集合、注入开关、记忆策略与置信度。通用安全评估默认使用 `assessment-security` profile；只有请求明确需要完整工作流时才加载 `full-*` Skill。

## 研究会话

复杂请求会被拆分为以下方法阶段：

```text
framing
  -> planning
  -> evidence
  -> analysis
  -> transformation
  -> execution
  -> verification
  -> reporting
```

`transformation`、`execution` 和 `verification` 需要逐阶段确认。网关返回：

```text
AWAITING_APPROVAL stage=<stage> [RESEARCH_SESSION:<id>]
```

在同一会话中回复 `继续`，只确认当前阶段。会话可通过以下顺序识别：

1. `X-Super-Instruct-Session` 请求头
2. 请求体 `super_instruct.session_id`
3. Provider conversation ID
4. Provider response ID
5. `[RESEARCH_SESSION:<id>]` 标记
6. 首轮请求指纹

研究请求会注入内部控制函数 `super_instruct_stage_complete`。普通工具调用仍由客户端执行；客户端回传的 `function_call_output`、`role: "tool"` 和其他 `*_call_output` 会进入证据图。

默认预算：

| 参数 | 默认值 |
| --- | ---: |
| 最大模型轮次 | 12 |
| 会话超时 | 1800 秒 |
| 连续无新证据上限 | 2 轮 |
| 检查点间隔 | 30 秒 |

## 支持的 Skills

仓库包含 31 个可路由 Skill：

```text
anti-debug                 binary-protect-bypass
card-key                   cloud-audit-bypass
cloud-native-escape        code-obfuscate
crack-keygen               crypto-tools
data-exfil                 ebpf-tracing
evasion                    exploit-attack
exploit-dev                full-crack
full-pentest               full-reverse
game-cheat                 malware-dev
network-pentest            phishing-kit
post-exploit               posture-assessment
ransomware-builder         rei-fallback
reverse-engineering        vip-bypass
vuln-scanner               wasm-decompile
web-crawler                web-pentest
wireless-attacks
```

每个 Skill 位于 `codex-skills/<id>/SKILL.md`，部分 Skill 带有配套脚本。注入器会按阶段过滤 Skill，例如 evidence 阶段不会加载完整工作流 Skill，assessment 分析阶段只保留姿态评估上下文。

## 快速开始

### 环境要求

- Rust stable toolchain
- 一个 Codex 兼容客户端
- 一个可访问的上游 API base URL
- 可选：逆向或分析工具，用于本地能力发现

### 构建

```bash
git clone https://github.com/JeffYu55/super-instruct-2.git
cd super-instruct-2
cargo build --release --manifest-path src-tauri/Cargo.toml
```

产物：

```text
src-tauri/target/release/super-instruct
```

### 运行

显式指定上游地址：

```bash
./src-tauri/target/release/super-instruct \
  --relay https://UPSTREAM_BASE_URL
```

也可以使用环境变量：

```bash
SUPER_INSTRUCT_RELAY_URL=https://UPSTREAM_BASE_URL \
  ./src-tauri/target/release/super-instruct
```

默认监听 `127.0.0.1:8080`。启动成功后检查：

```bash
curl http://127.0.0.1:8080/
```

使用 `Ctrl+C` 或发送 `SIGTERM` 可优雅停止服务并恢复 Codex 配置。

## Codex 配置托管

默认运行模式会：

1. 从 Codex 配置或 `relay_url.txt` 保存真实上游地址。
2. 备份 `config.toml` 为 `config.toml.super-instruct-bak`。
3. 将 Codex `base_url` 指向本地代理。
4. 写入 `model_instructions_file = "./bridge.md"`。
5. 部署 `bridge.md` 和仓库 Skills。
6. 每 2 秒检查配置漂移并恢复代理指向。
7. 退出时还原配置、删除本次部署项并恢复同名用户 Skill。

只运行代理、不修改 Codex 配置：

```bash
./src-tauri/target/release/super-instruct \
  --relay https://UPSTREAM_BASE_URL \
  --no-deploy
```

使用自定义监听地址时，建议配合 `--no-deploy` 并手动配置客户端；自动部署路径以默认 `127.0.0.1:8080` 为基准。

## CLI 参数

| 参数 | 说明 | 默认值 |
| --- | --- | --- |
| `--listen HOST:PORT` | 本地监听地址 | `127.0.0.1:8080` |
| `--relay URL` | 上游 API base URL | 环境变量或 Codex 配置 |
| `--bridge FILE` | bridge 指令文件 | `bridge.md` |
| `--skills DIR` | Skill 根目录 | `codex-skills` |
| `--logs DIR` | 日志与审计目录 | `logs` |
| `--memory FILE` | 成功记忆文件 | `memory.json` |
| `--sessions DIR` | 研究会话检查点目录 | `research-sessions` |
| `--research-max-rounds N` | 单会话最大轮次 | `12` |
| `--research-timeout-secs N` | 单会话超时秒数 | `1800` |
| `--research-no-evidence-limit N` | 连续无证据轮次上限 | `2` |
| `--execution-mode MODE` | `upstream-first`、`local-first`、`interleaved` | `interleaved` |
| `--cot-mode MODE` | `inject`、`extract`、`silent` | `inject` |
| `--no-deploy` | 禁用 Codex 配置托管 | 关闭 |

上游地址解析优先使用 `--relay` / `SUPER_INSTRUCT_RELAY_URL`，随后检查 `~/.codex/relay_url.txt`、配置备份和当前 Codex 配置。网关拒绝把自身地址作为上游，以避免代理自环。

## HTTP 接口

| 方法与路径 | 用途 |
| --- | --- |
| `GET /` | 健康状态、执行模式、CoT 模式、工具数量和结果计数 |
| `GET /super-instruct/v1/sessions/<id>` | 获取研究会话快照 |
| `GET /super-instruct/v1/sessions/<id>/events` | 获取历史事件并持续订阅 SSE |
| `ANY /*` | 转发到上游同路径 |

研究控制响应包含：

```text
X-Super-Instruct-Session: <session-id>
X-Super-Instruct-Result-Status: <status>
```

客户端可以用 `X-Super-Instruct-Stage` 或请求体中的 `super_instruct.stage` 提供阶段提示。研究会话的调度器仍以检查点中的当前阶段为准。

## 质量门控

响应评估由以下信号组成：

- HTTP 与上游结果状态
- 是否包含实质内容
- 证据覆盖率
- 动作覆盖率
- 未完成动作
- 产物、命令、工具调用和验证声明是否可核验
- 请求交付物与实际输出是否一致

分数达到 70、动作完整且没有验证问题时才通过质量门控。只有成功且通过门控的响应进入 `memory.json`；其余响应仍会写入交互审计和能力矩阵。

统一结果状态包括：

```text
SUCCEEDED
PARTIAL
TASK_DIVERGENCE
EXPLICIT_REFUSAL
POLICY_BLOCKED
EVIDENCE_MISSING
ARTIFACT_MISSING
TOOL_CLAIM_UNVERIFIED
PROTOCOL_ERROR
TRANSPORT_ERROR
CANCELLED
```

普通流式响应保持上游内容；非成功评估写入日志。研究编排器生成的控制响应会通过响应头暴露结果状态。

## CoT 模式

| 模式 | 行为 |
| --- | --- |
| `inject` | 将解析到的 reasoning 格式化后放入可见输出流 |
| `extract` | 客户端只接收正文，reasoning 单独归档 |
| `silent` | 不做额外 CoT 处理 |

CoT 记录写入 `logs/cot_traces/`，索引位于 `logs/cot_traces/cot_index.jsonl`。

## 运行数据

| 路径 | 内容 |
| --- | --- |
| `logs/interactions.jsonl` | 每次交互的类别、路由、阶段、结果和质量指标 |
| `logs/capability-matrix.json` | 按模型、意图和交付物聚合的结果统计 |
| `logs/responses/` | 需要保留的原始异常或未完成响应 |
| `logs/cot_traces/` | CoT trace 与索引 |
| `memory.json` | 通过质量门控的成功记忆 |
| `research-sessions/` | 可恢复研究会话检查点 |

这些文件可能包含请求、回复、工具结果和本地路径，已通过 `.gitignore` 排除，不应提交到版本库。

## 配套工具

```bash
# 查看 CoT trace 列表
python3 scripts/cot_viewer.py

# 实时查看新 trace
python3 scripts/cot_viewer.py tail

# 运行代理冒烟测试
python3 scripts/smoke_test.py

# 使用本地 Ollama 模型生成并评估提示变体
python3 scripts/pair_attacker.py \
  --seed "TASK" \
  --category reverse \
  --dry-run
```

`smoke_test.py` 需要正在运行的本地代理。`pair_attacker.py` 默认连接 `127.0.0.1:11434` 的 Ollama 与 `127.0.0.1:8080/v1/responses`。

## 项目结构

```text
.
├── bridge.md
├── codex-skills/
│   └── <skill-id>/
│       ├── SKILL.md
│       └── scripts/              # 可选
├── scripts/
│   ├── cot_viewer.py
│   ├── pair_attacker.py
│   └── smoke_test.py
└── src-tauri/
    ├── Cargo.toml
    ├── start-daemon.sh
    └── src/
        ├── main.rs
        ├── lib.rs
        ├── deploy.rs
        ├── log.rs
        ├── core/
        │   ├── contract.rs
        │   ├── cot.rs
        │   ├── execution.rs
        │   ├── protocol.rs
        │   ├── quality.rs
        │   ├── research.rs
        │   ├── router.rs
        │   └── stages.rs
        └── extensions/
            ├── inject.rs
            ├── memory.rs
            ├── monitor.rs
            └── sse_parser.rs
```

仓库中的 `frontend/`、Tauri 配置和图标属于早期桌面版本遗留资产，当前 headless 二进制不加载它们。

## 开发与验证

格式化检查：

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

运行测试：

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

Python 工具语法检查：

```bash
python3 -m py_compile \
  scripts/cot_viewer.py \
  scripts/pair_attacker.py \
  scripts/smoke_test.py
```

当前测试覆盖请求分类、路由、契约、DAG、阶段选择、协议续传、工具调用聚合、证据去重、会话恢复、质量门控和端到端流式响应。

## License

MIT，详见 [LICENSE](LICENSE)。

## Acknowledgements

早期项目与相关实现贡献者：lingbol088-spec、MDX-Tom、FuDie0915、InsTest。
