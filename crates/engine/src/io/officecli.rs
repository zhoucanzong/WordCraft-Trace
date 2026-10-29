//! Native OfficeCLI I/O runs against a private DOCX copy. A failed batch never
//! replaces the editor document; a successful one is committed as one undoable edit.
use wordcraft_doc::Document;

pub struct OfficeResult {
    pub report: String,
    pub document: Option<Document>,
    pub expected: String,
}

#[cfg(not(target_arch = "wasm32"))]
pub fn execute(doc: &Document, mode: &str, batch: &str) -> Result<OfficeResult, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    if !matches!(mode, "validate" | "outline" | "batch") {
        return Err("Unknown OfficeCLI operation".into());
    }
    let expected = crate::trace::fingerprint(doc)?;
    let sibling = std::env::current_exe().ok().map(|p| p.with_file_name("officecli"));
    let binary = std::env::var_os("OFFICECLI_PATH")
        .map(std::path::PathBuf::from)
        .or_else(|| sibling.filter(|p| p.is_file()))
        .unwrap_or_else(|| "officecli".into());
    let stamp = crate::cmd::now_iso().replace([':', '-'], "");
    let dir = std::env::temp_dir().join(format!("wordcraft-officecli-{}-{stamp}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    crate::file_access::check_write(&dir.to_string_lossy())?;
    std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
    let result = execute_in(doc, mode, batch, &binary, &dir).map(|(report, document)| OfficeResult { report, document, expected });
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(not(target_arch = "wasm32"))]
fn execute_in(
    doc: &Document,
    mode: &str,
    batch: &str,
    binary: &std::path::Path,
    dir: &std::path::Path,
) -> Result<(String, Option<Document>), String> {
    use std::process::{Command, Stdio};
    let file = dir.join("document.docx");
    crate::io::save_path(&file, doc)?;
    let mut command = Command::new(binary);
    match mode {
        "validate" => {
            command.arg("validate").arg(&file);
        }
        "outline" => {
            command.arg("view").arg(&file).arg("outline");
        }
        "batch" => {
            if batch.len() > 1024 * 1024 {
                return Err("Batch is larger than 1 MiB".into());
            }
            let items: serde_json::Value = serde_json::from_str(batch).map_err(|e| format!("Invalid batch JSON: {e}"))?;
            if !items.is_array() {
                return Err("OfficeCLI batch must be a JSON array".into());
            }
            let recipe = dir.join("batch.json");
            crate::io::write_file(&recipe.to_string_lossy(), batch.as_bytes())?;
            command.arg("batch").arg(&file).arg("--input").arg(recipe).arg("--stop-on-error");
        }
        _ => return Err("Unknown OfficeCLI operation".into()),
    }
    let stdout = dir.join("stdout.txt");
    let stderr = dir.join("stderr.txt");
    command
        .arg("--json")
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&stdout).map_err(|e| e.to_string())?)
        .stderr(std::fs::File::create(&stderr).map_err(|e| e.to_string())?);
    let mut child = command.spawn().map_err(|e| format!("Cannot launch OfficeCLI: {e}"))?;
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if start.elapsed().as_secs() >= 60 {
            let _ = child.kill();
            let _ = child.wait();
            return Err("OfficeCLI timed out after 60 seconds".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let out = crate::io::read_file_capped(&stdout.to_string_lossy(), 4 * 1024 * 1024, "OfficeCLI output")?;
    let err = crate::io::read_file_capped(&stderr.to_string_lossy(), 1024 * 1024, "OfficeCLI diagnostics")?;
    let report = format!("OfficeCLI {mode} · exit {status}\n\n{}\n{}", String::from_utf8_lossy(&out), String::from_utf8_lossy(&err));
    if mode == "batch" {
        if !status.success()
            || serde_json::from_slice::<serde_json::Value>(&out).ok().and_then(|v| v.get("success").and_then(|s| s.as_bool())) == Some(false)
        {
            return Err(format!("Batch was not applied to the editor.\n{report}"));
        }
        let bytes = crate::io::read_document(&file)?;
        let result = crate::trace::raw_document("document.docx", &bytes)?;
        Ok((report, Some(result)))
    } else {
        Ok((report, None))
    }
}

#[cfg(target_arch = "wasm32")]
pub fn execute(_doc: &Document, _mode: &str, _batch: &str) -> Result<OfficeResult, String> {
    Err("OfficeCLI integration requires the desktop application".into())
}
