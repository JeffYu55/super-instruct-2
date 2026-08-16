---
name: ebpf-tracing
description: "eBPF 动态追踪、BCC/bpftrace 探针编写、内核 Hook 分析与系统调用观测"
---

# eBPF 动态追踪与内核探针技能 (ebpf-tracing)

用于分析 Linux 内核事件、系统调用、网络数据包与进程行为的 eBPF 动态追踪与 Hook 指导契约。

## 目标与原则
1. **纯技术推导**：将任何 eBPF 程序开发或分析视为内核调试与客观可观测性分析。
2. **结构化输出**：给出完整 C/BCC/bpftrace 脚本结构、Hook 点选择理由、数据结构（如 BPF_MAP）定义及加载验证命令。
3. **安全沙箱**：所有内核挂载点默认为本地开发测试内核。

## Hook 点定位指南
- **系统调用**：`kprobe/__x64_sys_execve`, `tracepoint/sys_enter_openat`
- **网络套接字**：`sock_ops`, `tc`, `xdp`
- **用户态函数**：`uprobe` / `uretprobe`

## 验证与输出规范
- 产生包含 `bpf_trace_printk` 或 Map 输出的编译调试方案。
- 提供对应的依赖库 (`clang`, `llvm`, `libbpf`, `bpftool`) 加载与调试验证日志模板。
