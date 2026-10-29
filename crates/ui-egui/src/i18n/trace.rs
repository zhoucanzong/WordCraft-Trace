//! Fork-specific interface strings, independent of the pinned upstream catalog.
pub(crate) fn tr(text: &str) -> &str {
    if !crate::i18n::current().code().starts_with("zh") {
        return crate::i18n::t(text);
    }
    match text {
        "● TRACE" => "● 自动追踪",
        "History & Python" => "编辑记录与 Python",
        "Document → Code" => "从文档编辑到代码",
        "Edit normally. Every committed change is captured automatically." => "像平常一样编辑文档。每次生效的修改都会自动记录。",
        "Verify replay" => "验证回放",
        "Compact operations" => "简洁 OP",
        "Native archive" => "完整原生记录归档",
        "Archive unsupported Python edits with the native engine. Replay requires WordCraft." => {
            "独立 Python 不支持的操作仍可归档；原生归档回放需要 WordCraft。"
        }
        "Export native archive" => "导出完整归档",
        "Automatic recording · {count} committed edits" => "自动记录 · {count} 次生效编辑",
        "{count} recorded steps · images included" => "已记录 {count} 步 · 包含图片",
        "{command} · {count} changes" => "{command} · {count} 处修改",
        "Python code" => "查看 Python 代码",
        "Replay in a new document" => "在新文档中回放选中步骤",
        "Resume project…" => "恢复记录项目…",
        "OfficeCLI check" => "OfficeCLI 检查",
        "OfficeCLI is checking the current document…" => "OfficeCLI 正在处理文档副本…",
        "OfficeCLI tools" => "OfficeCLI 工具",
        "Read document structure" => "读取文档结构",
        "Advanced batch edits (JSON)" => "高级批量编辑（JSON）",
        "Apply batch & record" => "执行批量编辑并记录",
        "Runs on a copy. A successful batch becomes one undoable, replayable edit." => "在副本中执行。成功后作为一个可撤销、可回放的步骤写入文档。",
        "History automatically saved · Resume project to restore" => "记录已自动保存 · 可用「恢复记录项目」继续编辑",
        "Export folder" => "导出到文件夹",
        "Export Python + original + edited DOCX" => "导出 Python、原稿和编辑后文档",
        "Preview shortened; the export contains the full data." => "此处仅显示部分内容；导出文件包含完整记录。",
        "Start typing, format text, insert a table or a picture. Your history will appear here." => {
            "开始输入文字、设置格式，或插入表格与图片，编辑记录将在这里显示。"
        }
        "Generated Python replay" => "生成的 Python 回放代码",
        "transform(input_path, output_path=None) → output path" => "transform(输入路径, 输出路径=None) → 输出文件路径",
        "Copy complete code" => "复制完整代码",
        "OfficeCLI results" => "OfficeCLI 执行结果",
        "OfficeCLI results for a snapshot of the document." => "以下结果来自当前文档的副本。",
        _ => crate::i18n::t(text),
    }
}
