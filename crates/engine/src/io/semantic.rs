//! The independent, standard-library Python operation compiler and verifier.
use serde_json::Value;

pub const RUNTIME: &str = include_str!("../../../../python/docx_ops/__init__.py");
const COMPILER: &str = include_str!("../../../../python/compiler.py");

#[cfg(not(target_arch = "wasm32"))]
fn python() -> std::path::PathBuf {
    std::env::var_os("WORDCRAFT_TRACE_PYTHON").map(std::path::PathBuf::from).unwrap_or_else(|| "python3".into())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn compile(job: &Value) -> Result<Value, String> {
    process(job, None).map(|(value, _)| value)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn apply(document: &wordcraft_doc::Document, ops: &Value) -> Result<wordcraft_doc::Document, String> {
    let job = serde_json::json!({"action":"apply","base":document,"ops":ops});
    process(&job, Some(document))?.1.ok_or("Python did not return a document".into())
}

#[cfg(not(target_arch = "wasm32"))]
fn process(job: &Value, document: Option<&wordcraft_doc::Document>) -> Result<(Value, Option<wordcraft_doc::Document>), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "wordcraft-ops-{}-{}-{}",
        std::process::id(),
        crate::cmd::now_iso().replace(':', ""),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    // Compiler scratch contains only the caller's already-loaded model. It is not an
    // agent-selected file path; all exported paths still go through the checked I/O gateway.
    std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut job = job.clone();
        if let Some(document) = document {
            let input = dir.join("input.docx");
            std::fs::write(&input, crate::io::save_bytes("input.docx", document)?).map_err(|e| e.to_string())?;
            job["input"] = serde_json::json!(input);
            job["output"] = serde_json::json!(dir.join("output.docx"));
        }
        std::fs::write(dir.join("compiler.py"), COMPILER).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("docx_ops.py"), RUNTIME).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("input.json"), serde_json::to_vec(&job).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let args = vec![dir.join("compiler.py").into_os_string()];
        let output = run(&args, Some(&dir.join("input.json")), &dir)?;
        let result = serde_json::from_slice(&output).map_err(|e| e.to_string())?;
        let document = if document.is_some() {
            Some(crate::trace::raw_document("output.docx", &std::fs::read(dir.join("output.docx")).map_err(|e| e.to_string())?)?)
        } else {
            None
        };
        Ok((result, document))
    })();
    let _ = std::fs::remove_dir_all(dir);
    result
}

#[cfg(not(target_arch = "wasm32"))]
fn run(args: &[std::ffi::OsString], input: Option<&std::path::Path>, dir: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::process::{Command, Stdio};
    let out = dir.join(".python-out");
    let err = dir.join(".python-err");
    let mut command = Command::new(python());
    command
        .arg("-B")
        .arg("-E")
        .arg("-s")
        .args(args)
        .stdin(match input {
            Some(p) => Stdio::from(std::fs::File::open(p).map_err(|e| e.to_string())?),
            None => Stdio::null(),
        })
        .stdout(std::fs::File::create(&out).map_err(|e| e.to_string())?)
        .stderr(std::fs::File::create(&err).map_err(|e| e.to_string())?);
    let mut child =
        command.spawn().map_err(|e| format!("Python 3.9+ is required for compact export. Set WORDCRAFT_TRACE_PYTHON if necessary: {e}"))?;
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if start.elapsed().as_secs() >= 60 {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Python operation exceeded 60 seconds".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    // Both files were created by this subprocess in its private scratch directory.
    let read = |p: &std::path::Path| -> Result<Vec<u8>, String> {
        if std::fs::metadata(p).map_err(|e| e.to_string())?.len() > 16 * 1024 * 1024 {
            return Err("Python result exceeds 16 MiB".into());
        }
        std::fs::read(p).map_err(|e| e.to_string())
    };
    if !status.success() {
        return Err(String::from_utf8_lossy(&read(&err)?).trim().to_string());
    }
    read(&out)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn verify(directory: &std::path::Path) -> Result<(), String> {
    crate::file_access::check_read(&directory.to_string_lossy())?;
    crate::file_access::check_write(&directory.to_string_lossy())?;
    let args = [directory.join("replay.py"), directory.join("original.docx"), directory.join("replayed.docx")].map(|p| p.into_os_string());
    run(&args, None, directory)?;
    let _ = std::fs::remove_file(directory.join(".python-out"));
    let _ = std::fs::remove_file(directory.join(".python-err"));
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub fn apply(_document: &wordcraft_doc::Document, _ops: &Value) -> Result<wordcraft_doc::Document, String> {
    Err("Semantic operations require the desktop application".into())
}
#[cfg(target_arch = "wasm32")]
pub fn compile(_job: &Value) -> Result<Value, String> {
    Err("Compact Python export requires the desktop application".into())
}
#[cfg(target_arch = "wasm32")]
pub fn verify(_directory: &std::path::Path) -> Result<(), String> {
    Err("Compact Python export requires the desktop application".into())
}
