# Ohmu

[English](README.md) | **简体中文**

用 Rust 构建的本地优先 LLM Wiki：将资料持续整理为可阅读、可编辑、可追溯的 Markdown 知识库。完全不使用 embedding、向量数据库或向量相似度搜索。

Ohmu 与 [Nausicaa](https://github.com/Ceskoslov/Nausicaa) 分仓协作：Ohmu 管理知识，Nausicaa 提供 agent 循环、模型协议、权限、取消和持久执行回执。

## 当前状态

**M1 已实现：资料导入、页面关系及无向量全文检索。** M2–M4 持续分步推进。

| 已实现 | 后续阶段 |
| --- | --- |
| 本地 Markdown/HTML/PDF 与公共 HTTP(S) URL 导入 | 草稿生成、审阅发布与故障恢复 |
| 不可变来源版本、页码/字节定位、目录重建 | 模型驱动的综合与引用问答 |
| 页面元数据、别名、双向链接和健康检查 | 备份、恢复与规模评估 |
| 中文 Jieba + Tantivy BM25、增量索引及失效检测 | 发布打包与兼容性矩阵 |
| Nausicaa 页面读取、搜索、关系与来源读取工具 | OCR 和多用户服务不在 v1 范围 |

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

导入本地 Markdown、HTML 或文本 PDF 作为原始资料，不会自动创建或发布 Wiki 页面：

```sh
cargo run -p ohmu-cli -- ingest ./README.md --wiki /tmp/ohmu-demo --json
cargo run -p ohmu-cli -- sources --wiki /tmp/ohmu-demo --json
cargo run -p ohmu-cli -- catalog-rebuild --wiki /tmp/ohmu-demo
```

`ingest` 返回来源 ID、原始版本和解析产物哈希。`source <来源ID> --revision <哈希> --wiki <目录>` 可读取固定版本，支持 `--offset` 和 `--limit` 字节分页；`catalog-rebuild` 可从来源清单重建 SQLite 目录。PDF 读取保留物理页码且不跨页；HTML 引用定位到提取后的 Markdown。扫描件需要 OCR。通过显式 `ingest-url` 可抓取公共 HTTP(S) 资料；本地、私网和保留地址默认拒绝。

构建或增量更新索引，然后检索已发布 Wiki；`--collection sources` 可检索原始资料：

```sh
cargo run -p ohmu-cli -- reindex --wiki /tmp/ohmu-demo
cargo run -p ohmu-cli -- search Ohmu --wiki /tmp/ohmu-demo --json
cargo run -p ohmu-cli -- search Rust --collection sources --wiki /tmp/ohmu-demo --json
cargo run -p ohmu-cli -- lint --wiki /tmp/ohmu-demo --json
```

外部编辑或新增来源会使旧索引失效；重新运行 `reindex` 更新。`read --offset 0 --limit 16384` 支持读取大页面；`lint` 和 `neighbors` 始终读取当前 Markdown。模型生成与发布将在后续阶段加入。

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
