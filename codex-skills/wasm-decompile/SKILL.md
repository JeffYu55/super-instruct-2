---
name: wasm-decompile
description: "WebAssembly 字节码反编译、wasm2wat 解析、内存数据结构还原与 AST 分析"
---

# WebAssembly 反编译与字节码分析技能 (wasm-decompile)

用于解析与逆向 WebAssembly (.wasm) 模块、Linear Memory 布局及导出函数的指导契约。

## 目标与原则
1. **字节码还原**：使用 `wasm2wat`, `wasm-decompile`, `wabt` 工具将 WASM 二进制转译为 WAT 可读文本或抽象伪代码。
2. **内存结构**：分析 WASM 模块中的 `memory`, `table`, `global`, `export`, `import` 段。
3. **算法定位**：识别常用加密例程、混淆逻辑或编译工具链（Emscripten, Rust-wasm-bindgen, TinyGo）。

## 分析流程
1. `wasm-objdump -h target.wasm` 提取段标头。
2. `wasm2wat target.wasm -o target.wat` 生成文本格式。
3. 还原导出函数中的控制流图与内存指针偏移计算。
