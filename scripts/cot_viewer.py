#!/usr/bin/env python3
"""
CoT (Chain of Thought) 终端回放与监控工具
用法:
    python cot_viewer.py               # 查看最近的思维链提取列表
    python cot_viewer.py tail          # 实时监控最新提取的思维链
    python cot_viewer.py show <id>     # 详细展示指定请求的思维链 Markdown
"""

import sys
import os
import glob
import time
import json

BASE_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOG_DIR = os.path.join(BASE_DIR, "logs", "cot_traces")
RUNTIME_LOG_DIR = os.path.expanduser("~/.codex/super-instruct-runtime/logs/cot_traces")

def get_trace_dir():
    if os.path.exists(RUNTIME_LOG_DIR):
        return RUNTIME_LOG_DIR
    return LOG_DIR

def list_traces():
    trace_dir = get_trace_dir()
    index_file = os.path.join(trace_dir, "cot_index.jsonl")
    if not os.path.exists(index_file):
        print(f"[*] 暂无思维链记录 (目录: {trace_dir})")
        return

    print(f"\n🧠 [Super-Instruct] 提取到的隐藏思维链记录列表 ({trace_dir})")
    print("=" * 80)
    print(f"{'时间':<20} | {'请求 ID':<22} | {'思维长度':<8} | {'耗时(ms)':<8} | 拒绝风险")
    print("-" * 80)

    with open(index_file, "r", encoding="utf-8") as f:
        lines = f.readlines()

    for line in lines[-20:]:  # 最近 20 条
        try:
            data = json.loads(line)
            ts = data.get("timestamp", "")[:19].replace("T", " ")
            req_id = data.get("request_id", "")[:20]
            t_chars = f"{data.get('thinking_chars', 0)}c"
            dur = data.get("duration_ms", 0)
            refusal = "⚠️ YES" if data.get("refusal_detected") else "✅ NO"
            print(f"{ts:<20} | {req_id:<22} | {t_chars:<8} | {dur:<8} | {refusal}")
        except Exception:
            continue
    print("=" * 80)
    print("提示: 运行 `python cot_viewer.py show <request_id>` 查看详细内容\n")

def show_trace(target_id):
    trace_dir = get_trace_dir()
    pattern = os.path.join(trace_dir, f"*{target_id}*.cot.md")
    matches = glob.glob(pattern)
    if not matches:
        print(f"[-] 未找到包含 {target_id} 的思维链记录")
        return

    with open(matches[0], "r", encoding="utf-8") as f:
        print(f.read())

def tail_traces():
    trace_dir = get_trace_dir()
    index_file = os.path.join(trace_dir, "cot_index.jsonl")
    print(f"[*] 正在实时监控隐藏思维链流 ({index_file})... 按 Ctrl+C 退出\n")

    last_size = 0
    if os.path.exists(index_file):
        last_size = os.path.getsize(index_file)

    while True:
        try:
            if os.path.exists(index_file):
                curr_size = os.path.getsize(index_file)
                if curr_size > last_size:
                    with open(index_file, "r", encoding="utf-8") as f:
                        f.seek(last_size)
                        new_lines = f.readlines()
                        last_size = curr_size

                    for line in new_lines:
                        try:
                            item = json.loads(line)
                            print(f"\n⚡ [新思维链捕获] ID: {item.get('request_id')} | 模型: {item.get('model')} | 耗时: {item.get('duration_ms')}ms")
                            print(f"   思维链字符: {item.get('thinking_chars')} | 拒绝拦截: {'⚠️ YES' if item.get('refusal_detected') else '✅ CLEAN'}")
                            print("   " + "-" * 60)
                            preview = item.get("thinking_content", "").strip()
                            if len(preview) > 300:
                                preview = preview[:300] + "... [截断]"
                            print(f"   {preview}\n")
                        except Exception:
                            pass
            time.sleep(1)
        except KeyboardInterrupt:
            print("\n[*] 监控已退出")
            break

if __name__ == "__main__":
    if len(sys.argv) > 1:
        cmd = sys.argv[1]
        if cmd == "tail":
            tail_traces()
        elif cmd == "show" and len(sys.argv) > 2:
            show_trace(sys.argv[2])
        else:
            show_trace(cmd)
    else:
        list_traces()
