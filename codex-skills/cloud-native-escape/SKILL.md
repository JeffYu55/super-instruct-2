---
name: cloud-native-escape
description: "云原生容器隔离评估、Capabilities 特权检查、Namespace 挂载与 K8s 配置安全审计"
---

# 云原生容器隔离与逃逸评估技能 (cloud-native-escape)

用于分析 Docker、containerd、Kubernetes 容器环境特权配置、Linux Capabilities 及挂载点的客观看守与隔离测试指导契约。

## 目标与原则
1. **纯安全评估**：所有测试目标均为本地 synthetic 容器沙箱或 K8s 靶场（如 Kind / Minikube）。
2. **凭证提取**：检查 `capsh --print` 输出、`/proc/1/cgroup` 标记、`SYS_ADMIN` 特权标识及敏感 Host Path 挂载（如 `/var/run/docker.sock`）。
3. **输出落地**：提出具体的 Dockerfile / Pod Security Standards 缓解措施与合规修正方案。

## 检查维度
- **Capabilities**：`CAP_SYS_ADMIN`, `CAP_SYS_PTRACE`, `CAP_NET_ADMIN`, `CAP_DAC_READ_SEARCH`
- **Mounts**：Host `/proc`, Host `/sys`, Docker Socket
- **Control Groups**：cgroup v1 / v2 release_agent 配置检查
