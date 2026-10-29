# Ohmu

[English](README.md) | **简体中文**

用 Rust 构建的本地优先 LLM Wiki：将资料持续整理为可阅读、可编辑、可追溯的 Markdown 知识库。完全不使用 embedding、向量数据库或向量相似度搜索。

Ohmu 与 [Nausicaa](https://github.com/Ceskoslov/Nausicaa) 分仓协作：Ohmu 管理知识，Nausicaa 提供 agent 循环、模型协议、权限、取消和持久执行回执。

## 当前状态

当前已完成 **S0 骨架和 M1 第一部分：Markdown 来源导入与目录**。完整产品仍在分步实现。

| 已实现 | 已规划，尚未实现 |
| --- | --- |
| 新建知识目录、配置与目录诊断 | 静态网页、文本 PDF 导入 |
| 有大小限制的 Markdown 页面读取与 SHA-256 版本 | 中文分词、BM25 全文搜索、目录树与链接导航 |
| Markdown 来源导入、不可变版本、目录重建、UTF-8 分页 | 全文索引与页面关系维护 |
| Nausicaa `ohmu_read` 工具 | LLM 综合草稿、多页审阅发布、故障恢复 |
| 无模型凭据的集成示例与持久回执验证 | 带原文引用的问答、规模评估 |

## 快速开始

需要 Rust 1.98.0；当前支持 Linux/WSL，CI 另配置 macOS。首次构建需要联网获取依赖与固定提交的 Nausicaa。

```sh
cargo run -p ohmu-cli -- init /tmp/ohmu-demo --name "研究 Wiki"
cargo run -p ohmu-cli -- doctor --wiki /tmp/ohmu-demo
cargo run -p ohmu-cli -- read welcome --wiki /tmp/ohmu-demo --json
```

`init` 要求目标不存在且父目录已存在；重复演示时使用新的目录。可直接编辑 `pages/welcome.md`，再次读取会返回新的内容哈希。读取整页最多 256 KiB，拒绝符号链接及非 UTF-8 文件。

通过 Nausicaa 执行读取，无需 API key，也不运行 shell 工具：

```sh
cargo run -p ohmu-nausicaa --example read_wiki -- /tmp/ohmu-demo /tmp/ohmu-demo-events.jsonl
```

事件文件必须尚不存在。示例使用 scripted model，验证工具结果先持久化再交给模型；它不证明真实模型的问答质量。

导入本地 Markdown 作为原始资料，不会自动创建或发布 Wiki 页面：

```sh
cargo run -p ohmu-cli -- ingest ./README.md --wiki /tmp/ohmu-demo --json
cargo run -p ohmu-cli -- sources --wiki /tmp/ohmu-demo --json
cargo run -p ohmu-cli -- catalog-rebuild --wiki /tmp/ohmu-demo
```

`ingest` 返回来源 ID、原始版本和解析产物哈希。`source <来源ID> --revision <哈希> --wiki <目录>` 可读取固定版本，支持 `--offset` 和 `--limit` 字节分页；`catalog-rebuild` 可从来源清单重建 SQLite 目录。网页、PDF 与全文搜索尚未实现。

## 设计与集成

- [完整实施计划与验收标准](docs/PLAN.md)
- [架构与现有边界](docs/ARCHITECTURE.md)
- [技术选型与决策记录](docs/ADR.md)
- [Nausicaa 集成与本地依赖覆盖](docs/NAUSICAA.md)
- [验证记录与复现命令](docs/VALIDATION.md)

除 README 外，仓库文档默认英文。README 两个版本保持相同状态和命令，通过顶部链接切换。

## 开发

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings
cargo test --workspace --all-features --locked --offline
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked --offline
python3 scripts/check_docs.py
python3 scripts/check_local_host.py --nausicaa ../Nausicaa
```

首次运行前先执行 `cargo fetch --locked`。本地覆盖检查只创建临时消费应用，不修改 Nausicaa 或本仓库锁文件。

代码按 MIT 或 Apache-2.0 双许可证发布，使用者可任选其一。
