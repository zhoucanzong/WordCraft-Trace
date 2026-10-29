# WordCraft Trace — 带自动编辑追踪与回放的 WordCraft

**WordCraft Trace 是在 WordCraft 上增加 Trace 的 DOCX 编辑器分支。** 保留 WordCraft 的可视化编辑界面，集成 OfficeCLI，自动记录实际生效的编辑，并导出简洁 OP 与可独立运行的 Python。

本分支基于 [WordCraft](https://github.com/storytold/wordcraft) 0.12.0；应用包集成 [OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) 1.0.159。不是两个上游的官方联合发布。上游介绍保留在 [README-UPSTREAM.md](README-UPSTREAM.md)。

## Trace 0.2 的工作方式

1. 打开 DOCX，像在 Word 中一样编辑文字、格式、段落、表格和图片。
2. 右侧时间线自动记录文档事务，包括撤销、重做和 OfficeCLI 批量编辑；选区移动和失败操作不记为修改。
3. 查看「Python code」或「简洁 OP」，查看从原稿到当前结果的精简操作。
4. 点击导出，程序运行生成的 Python，再重新读取结果与编辑器导出的文档模型比较。只有通过才生成 `verification.json`。
5. 将导出目录交给另一台安装 Python 3.9+ 的电脑，即可回放，不需要 WordCraft、OfficeCLI 或 pip 包。

例如多次输入、局部加粗、撤销再重做，最终可精简为：

```python
from docx_ops import Editor

def transform(input_path, output_path=None):
    doc = Editor(input_path, targets="targets.json")
    doc.replace_text("p0", 6, 8, "你好")
    doc.format_text("p0", 6, 8, bold=True)
    return doc.save(output_path)
```

实际导出代码还包含原稿 SHA-256 检查、相对于脚本位置读取辅助文件和命令行入口。每条 OP 通常是一行方法调用。共享的 `docx_ops.py` 使用 Python 标准库直接操作 OOXML；它没有内嵌文档快照，也不调用原生编辑器。图片存为独立素材文件，代码中只有素材路径。

## 使用应用

本次应用包为 macOS Apple Silicon。解压后打开 `WordCraftTrace.app`。应用为本地 ad-hoc 签名，未经过 Apple 公证；若系统阻止，使用系统「隐私与安全性 → 仍要打开」流程。

编辑、原生历史回放、OfficeCLI 功能不依赖 Python。**精简 OP、独立 Python 导出和 MCP `apply_ops` 需要 Python 3.9+**。默认调用 `python3`，可通过 `WORDCRAFT_TRACE_PYTHON` 指定完整解释器路径。

导出目录包含：

| 文件 | 用途 |
| --- | --- |
| `replay.py` | 简短编辑程序，接收输入路径，返回输出绝对路径 |
| `docx_ops.py` | 独立 OOXML 操作库，必须与脚本一起保留 |
| `ops.json` | 精简后的语义操作 |
| `targets.json` | 目标定位与原始文本指纹 |
| `original.docx` | 确定性回放的原稿 |
| `edited.docx` | 编辑器导出的预期结果 |
| `replayed.docx` | 导出时实际执行 Python 得到的结果 |
| `assets/` | 有新增图片时的独立素材 |
| `trace.json` | 完整事务审计记录，不被独立 Python 加载 |
| `verification.json` | 独立执行及模型比较成功的标记 |

```sh
cd 导出目录
python3 replay.py original.docx another-result.docx
```

```python
from replay import transform
output_path = transform("original.docx", "another-result.docx")
```

输出路径必须尚不存在。省略输出路径会使用原稿旁的 `original_replayed.docx`。默认严格绑定同一原稿；不是对任意不同模板自动迁移修改。

## 完整审计与精简操作

完整历史记录每次提交的命令、时间、选区和前后结构差异；精简导出将原稿与最终状态编译成语义 OP，合并输入和撤销抵消的效果。**当前不是所有 GUI 命令都直接产生语义 OP**；GUI 的完整事务记录是事实来源，导出时编译净变化。MCP `apply_ops` 与导出的 Python 使用同一个操作库。

在时间线选择步骤可在新文档中回放到该步骤，保留原文档。完整历史也可通过「完整原生记录归档」保存，不支持独立 Python 的复杂修改仍可用这个归档回放，但这种归档需要 WordCraft 引擎。原生归档和精简 Python 是两种明确区分的导出方式。

历史约每 5 秒保存，正常退出也保存。默认位置是 `~/Library/Application Support/WordCraftTrace/History`；可用 `WORDCRAFT_TRACE_HOME` 修改。点击「恢复记录项目」恢复并继续追加。普通保存 DOCX 不会清空历史。新建空白文档导出时也生成 DOCX 原稿，另外保留内部 JSON 基线供原生历史恢复。

## OfficeCLI 集成

- 「OfficeCLI 检查」验证当前文档副本的 OOXML。
- 「OfficeCLI 工具」可读取文档结构，或运行 JSON 批量编辑。
- 批处理只在副本上运行；全部成功且原文档版本仍匹配，才作为一步写回，支持撤销和追踪。
- 应用包内置 OfficeCLI。源码运行时设置 `OFFICECLI_PATH=/绝对路径/officecli`，或将可执行文件放在 WordCraft 二进制旁。

```json
[{"command":"set","path":"/body/p[1]","props":{"bold":"true"}}]
```

OfficeCLI 的所有命令不会被盲目转成 Python；只有操作库覆盖且独立验证通过的最终修改才能精简导出。

## 大模型 MCP 接口

保留 WordCraft 原有 MCP，并新增 Trace 与语义操作工具。使用 stdio 启动：

```sh
/路径/WordCraftTrace.app/Contents/MacOS/wordcraft-cli mcp
```

连接已开启控制端口的可视化编辑器：

```sh
/路径/WordCraftTrace.app/Contents/MacOS/wordcraft --control 7981
/路径/WordCraftTrace.app/Contents/MacOS/wordcraft-cli mcp --connect 127.0.0.1:7981
```

| 工具 | 用途 |
| --- | --- |
| `inspect_targets` | 获取目标 ID 与当前版本指纹 |
| `apply_ops` | 提交语义 OP；必须带上述版本，过期则拒绝；整批可撤销 |
| `get_trace` | 获取当前精简 OP、目标与 Python |
| `export_python` | 导出并实际验证独立 Python 包 |
| `replay_trace` | 按完整历史回放到指定步骤 |
| `validate_document` | 调用 OfficeCLI 验证文档 |

先通过原有 MCP 打开文档，再 `inspect_targets`，然后提交：

```json
{"version":"inspect_targets 返回的 version","ops":[
  {"op":"replace_text","target":"p0","start":6,"end":8,"text":"你好"},
  {"op":"format_text","target":"p0","start":6,"end":8,"bold":true}
]}
```

偏移量按 **Unicode 字符** 计数，不是 UTF-8 字节或 UTF-16 单元。目标在同一批操作中绑定到 XML 节点，前面插入或删除段落后不会发生索引漂移。更多原有接口见 [docs/mcp.md](docs/mcp.md)。

## 支持范围与限制

独立 Python 当前覆盖：文字替换、局部字符格式、常用段落格式、段落标记格式、正文段落插入和删除、现有表格单元格文字、简单表格插入、新增行内 PNG/JPEG/GIF、常用页面尺寸与边距。

复杂表格合并、已有图片替换/浮动布局、页眉页脚、修订字段、样式表/编号定义等改变尚未全部适配；文档标题、作者等元数据修改也会明确拒绝精简导出。遇到未覆盖的修改会报错，或在模型比较时拒绝验证，不会标为成功。可用完整原生归档保留这些修改。

回放验证比较 WordCraft 重新读入的文档模型，忽略保存时间、修订计数和最后保存者，并按图片内容规范化资源名称；不代表 Microsoft Word 像素级一致或 OOXML 包字节一致。格式保真继承自上游 WordCraft，尚不能承诺完整替代 Microsoft Word。未修改的 ZIP 部件由独立运行库原样保留。

Trace 不是 Git：没有多人合并或跨任意模板自动重定位。记录包含原稿及编辑前后内容，未加密。当前历史仍使用内存与定时落盘，大图片和长时间编辑会增加体积；异常退出可能损失最近几秒。

## 从源码构建

需要 Rust 1.96+、平台开发工具和 Python 3.9+。首次构建会下载 Cargo 依赖及固定版本 Git 依赖；翻译资源按仓库中的 `craft-i18n.rev` 固定。OfficeCLI 为独立程序，请从其官方项目取得适合平台的版本并设置路径。

```sh
git clone https://github.com/zhoucanzong/WordCraft-Trace.git
cd WordCraft-Trace
cargo build --locked -p wordcraft -p wordcraft-cli
export OFFICECLI_PATH=/绝对路径/officecli
cargo run -p wordcraft -- --sample
```

翻译资源下载受限时，可预先取得 `storytold/craft-i18n` 中 `craft-i18n.rev` 指向的版本，通过 `CRAFT_I18N_DIR` 指向解压目录。完整上游构建文档见 [README-UPSTREAM.md](README-UPSTREAM.md)。本分支仅实测 macOS arm64；没有宣称 Windows、Linux 和 Web 集成全部通过。

```sh
python3 -m unittest discover -s python -v
cargo test -p wordcraft-engine -p wordcraft-mcp --lib -- --include-ignored
cargo clippy -p wordcraft-engine -p wordcraft-ui-egui -p wordcraft-cli -p wordcraft-mcp --no-deps -- -D warnings
```

主要实现：`crates/engine/src/trace.rs`（事务与导出）、`python/compiler.py`（语义编译）、`python/docx_ops/`（独立操作库）、`crates/engine/src/io/semantic.rs`（Python 桥接）、`crates/engine/src/io/officecli.rs`、`crates/ui-egui/src/trace_panel.rs`、`crates/mcp/src/tools.rs`。

## 许可证与来源

WordCraft 与本分支代码遵循 MIT OR Apache-2.0。OfficeCLI 遵循 Apache-2.0；分发应用附有许可证和 NOTICE。保留上游版权、资产归属与贡献者信息，见 [ATTRIBUTION.md](ATTRIBUTION.md)、[LICENSE-MIT](LICENSE-MIT)、[LICENSE-APACHE](LICENSE-APACHE)。
