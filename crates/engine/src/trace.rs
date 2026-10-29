//! Committed document transactions. Commands are audit metadata; structural changes are
//! the replay authority, including automatic edits, rich paste, media, undo and redo.
//! No UI state, clipboard, original image path or wall clock is needed during replay.

use crate::{CmdError, CommandSpec, Session};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use wordcraft_doc::Document;

pub const ENGINE: &str = "wordcraft-trace-0.1.0/wordcraft-0.12.0";
const MAX_TRACE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Change {
    Set { path: Vec<String>, before: Option<Value>, after: Option<Value> },
    Splice { path: Vec<String>, index: usize, before: Vec<Value>, after: Vec<Value> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub command: String,
    pub label: String,
    pub time: String,
    pub selection: Value,
    pub changes: Vec<Change>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bundle {
    pub schema: u32,
    pub engine: String,
    pub source_sha256: String,
    pub base_sha256: String,
    pub result_sha256: String,
    pub steps: Vec<Step>,
}

pub struct Journal {
    pub base: Value,
    pub last: Value,
    pub steps: Vec<Step>,
    pub source_name: String,
    source: Vec<u8>,
    observed: Document,
}

#[derive(Default)]
pub struct Recorder {
    pub enabled: bool,
    pub depth: usize,
    pub journals: BTreeMap<u64, Journal>,
    pub error: Option<String>,
}

pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn model_digest(value: &Value) -> Result<String, String> {
    serde_json::to_vec(value).map(|b| digest(&b)).map_err(|e| e.to_string())
}

pub fn fingerprint(document: &Document) -> Result<String, String> {
    model_digest(&serde_json::to_value(document).map_err(|e| e.to_string())?)
}

fn normalize_media(value: &mut Value) {
    let names: BTreeMap<String, String> = value
        .get("media")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), digest(v.as_str().unwrap_or_default().as_bytes()))).collect())
        .unwrap_or_default();
    fn visit(value: &mut Value, names: &BTreeMap<String, String>) {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(name)) = map.get_mut("media")
                    && let Some(hash) = names.get(name)
                {
                    *name = hash.clone();
                }
                for value in map.values_mut() {
                    visit(value, names);
                }
            }
            Value::Array(array) => {
                for value in array {
                    visit(value, names);
                }
            }
            _ => {}
        }
    }
    visit(value, &names);
    if let Some(Value::Object(media)) = value.get_mut("media") {
        let original = std::mem::take(media);
        for (name, data) in original {
            media.insert(names.get(&name).cloned().unwrap_or(name), data);
        }
    }
}

pub fn raw_document(name: &str, bytes: &[u8]) -> Result<Document, String> {
    if name.to_ascii_lowercase().ends_with(".json") {
        serde_json::from_slice(bytes).map_err(|e| e.to_string())
    } else if name.to_ascii_lowercase().ends_with(".docx") {
        wordcraft_docx::read(bytes).map_err(|e| e.to_string())
    } else {
        Err("Trace replay accepts .docx or the exported .wcraft.json baseline".into())
    }
}

fn differences(a: &Value, b: &Value, path: &mut Vec<String>, out: &mut Vec<Change>) {
    if a == b {
        return;
    }
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, old) in a {
                path.push(key.clone());
                if let Some(new) = b.get(key) {
                    differences(old, new, path, out);
                } else {
                    out.push(Change::Set { path: path.clone(), before: Some(old.clone()), after: None });
                }
                path.pop();
            }
            for (key, new) in b {
                if !a.contains_key(key) {
                    path.push(key.clone());
                    out.push(Change::Set { path: path.clone(), before: None, after: Some(new.clone()) });
                    path.pop();
                }
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (i, (old, new)) in a.iter().zip(b).enumerate() {
                path.push(i.to_string());
                differences(old, new, path, out);
                path.pop();
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
            let suffix = a.iter().rev().zip(b.iter().rev()).take(a.len().min(b.len()).saturating_sub(prefix)).take_while(|(x, y)| x == y).count();
            let before = a.get(prefix..a.len().saturating_sub(suffix)).unwrap_or_default().to_vec();
            let after = b.get(prefix..b.len().saturating_sub(suffix)).unwrap_or_default().to_vec();
            out.push(Change::Splice { path: path.clone(), index: prefix, before, after });
        }
        _ => out.push(Change::Set { path: path.clone(), before: Some(a.clone()), after: Some(b.clone()) }),
    }
}

fn at_mut<'a>(root: &'a mut Value, path: &[String]) -> Result<&'a mut Value, String> {
    let mut node = root;
    for key in path {
        node = match node {
            Value::Object(map) => map.get_mut(key),
            Value::Array(array) => key.parse::<usize>().ok().and_then(|i| array.get_mut(i)),
            _ => None,
        }
        .ok_or_else(|| format!("Trace path missing: {}", path.join("/")))?;
    }
    Ok(node)
}

fn apply_change(root: &mut Value, change: &Change) -> Result<(), String> {
    match change {
        Change::Set { path, before, after } => {
            if let Some((key, parent)) = path.split_last() {
                let node = at_mut(root, parent)?;
                match node {
                    Value::Object(map) => {
                        if map.get(key) != before.as_ref() {
                            return Err(format!("Precondition failed: {}", path.join("/")));
                        }
                        if let Some(v) = after {
                            map.insert(key.clone(), v.clone());
                        } else {
                            map.remove(key);
                        }
                    }
                    Value::Array(array) => {
                        let i = key.parse::<usize>().map_err(|e| e.to_string())?;
                        let slot = array.get_mut(i).ok_or("Array target missing")?;
                        if Some(&*slot) != before.as_ref() {
                            return Err("Array precondition failed".into());
                        }
                        *slot = after.clone().ok_or("Array removal requires splice")?;
                    }
                    _ => return Err("Target parent is not a container".into()),
                }
            } else {
                if Some(&*root) != before.as_ref() {
                    return Err("Root precondition failed".into());
                }
                *root = after.clone().ok_or("Cannot remove the document root")?;
            }
        }
        Change::Splice { path, index, before, after } => {
            let a = at_mut(root, path)?.as_array_mut().ok_or("Splice target is not an array")?;
            let end = index.checked_add(before.len()).ok_or("Splice overflow")?;
            if a.get(*index..end) != Some(before.as_slice()) {
                return Err(format!("Splice precondition failed: {}", path.join("/")));
            }
            a.splice(*index..end, after.iter().cloned());
        }
    }
    Ok(())
}

pub fn apply_steps(mut value: Value, steps: &[Step]) -> Result<Value, String> {
    for (i, step) in steps.iter().enumerate() {
        for change in &step.changes {
            apply_change(&mut value, change).map_err(|e| format!("Step {} ({}): {e}", i + 1, step.command))?;
        }
    }
    Ok(value)
}

impl Journal {
    pub fn compact(&self) -> Result<Value, String> {
        let source = self.compact_source()?;
        let mut result = crate::io::semantic::compile(&json!({"base":self.base,"final":self.last,"source_sha256":digest(&source)}))?;
        if let Some(map) = result.as_object_mut() {
            map.remove("assets");
        }
        Ok(result)
    }

    fn compact_source(&self) -> Result<Vec<u8>, String> {
        if self.source_name.to_ascii_lowercase().ends_with(".docx") {
            Ok(self.source.clone())
        } else {
            let baseline: Document = serde_json::from_value(self.base.clone()).map_err(|e| e.to_string())?;
            crate::io::save_bytes("original.docx", &baseline)
        }
    }

    pub fn export_compact(&self, directory: &std::path::Path) -> Result<Value, String> {
        if directory.exists() {
            return Err("Choose a new export folder".into());
        }
        let source = self.compact_source()?;
        let mut plan = crate::io::semantic::compile(&json!({"base":self.base,"final":self.last,"source_sha256":digest(&source)}))?;
        let code = plan.get("code").and_then(Value::as_str).ok_or("Compiler did not return code")?.to_string();
        let targets = plan.get("targets").cloned().unwrap_or(json!({}));
        let assets = plan.get("assets").cloned().unwrap_or(json!({}));
        let count = plan.get("ops").and_then(Value::as_array).map_or(0, Vec::len);
        if let Some(map) = plan.as_object_mut() {
            map.remove("code");
            map.remove("targets");
            map.remove("assets");
        }
        crate::io::create_directory(directory)?;
        let write = |name: &str, data: &[u8]| crate::io::write_file(&directory.join(name).to_string_lossy(), data);
        if let Some(assets) = assets.as_object() {
            if !assets.is_empty() {
                crate::io::create_directory(&directory.join("assets"))?;
            }
            for (name, data) in assets {
                use base64::Engine;
                if !name.starts_with("assets/image") || name.contains("..") || name.matches('/').count() != 1 {
                    return Err("Invalid asset name".into());
                }
                let bytes =
                    base64::engine::general_purpose::STANDARD.decode(data.as_str().ok_or("Invalid asset data")?).map_err(|e| e.to_string())?;
                write(name, &bytes)?;
            }
        }
        write("original.docx", &source)?;
        if !self.source_name.to_ascii_lowercase().ends_with(".docx") {
            write("original.wcraft.json", &self.source)?;
        }
        write("replay.py", code.as_bytes())?;
        write("docx_ops.py", crate::io::semantic::RUNTIME.as_bytes())?;
        write("targets.json", &serde_json::to_vec_pretty(&targets).map_err(|e| e.to_string())?)?;
        write("ops.json", &serde_json::to_vec_pretty(&plan).map_err(|e| e.to_string())?)?;
        write("trace.json", &serde_json::to_vec(&self.bundle()?).map_err(|e| e.to_string())?)?;
        let expected: Document = serde_json::from_value(self.last.clone()).map_err(|e| e.to_string())?;
        crate::io::save_path(&directory.join("edited.docx"), &expected)?;
        crate::io::semantic::verify(directory)?;
        let expected = crate::io::open_path(&directory.join("edited.docx"))?;
        let actual = crate::io::open_path(&directory.join("replayed.docx"))?;
        // Core revision/timestamps are save bookkeeping, not edit operations.
        let mut a = serde_json::to_value(expected).map_err(|e| e.to_string())?;
        let mut b = serde_json::to_value(actual).map_err(|e| e.to_string())?;
        for value in [&mut a, &mut b] {
            if let Some(core) = value.get_mut("core").and_then(Value::as_object_mut) {
                for key in ["modified", "revision", "lastModifiedBy"] {
                    core.remove(key);
                }
            }
        }
        normalize_media(&mut a);
        normalize_media(&mut b);
        if a != b {
            return Err(format!("Independent Python result differs from WordCraft. Export is unverified; inspect {}", directory.display()));
        }
        write("verification.json", br#"{"success":true,"backend":"independent-python-ooxml","model":"equal-except-save-metadata"}"#)?;
        write("README.txt", b"WordCraft Trace compact export\nRun: python3 replay.py original.docx output.docx\nPython 3.9+ standard library only. No WordCraft/OfficeCLI process is needed.\nKeep docx_ops.py and targets.json beside replay.py. ops.json contains compact operations.\ntrace.json is the full audit history, never loaded by replay.py.\nverification.json is written only after independent execution and document-model comparison pass.\n")?;
        Ok(json!({"directory":directory,"operations":count,"codeLines":code.lines().count(),"verified":true,"engineRequired":false}))
    }

    pub fn checkpoint(&self, directory: &std::path::Path) -> Result<(), String> {
        crate::io::create_directory(directory)?;
        let ext = if self.source_name.to_ascii_lowercase().ends_with(".docx") { "docx" } else { "wcraft.json" };
        let source = directory.join(format!("original.{ext}"));
        if !source.exists() {
            crate::io::write_file(&source.to_string_lossy(), &self.source)?;
        }
        let data = serde_json::to_vec(&self.bundle()?).map_err(|e| e.to_string())?;
        crate::io::write_file(&directory.join("trace.json").to_string_lossy(), &data)
    }

    fn new(doc: &Document, path: Option<&std::path::Path>) -> Result<Self, String> {
        let imported = path
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("docx")))
            .and_then(|p| crate::io::read_document(p).ok().map(|b| (p.to_string_lossy().into_owned(), b)))
            .and_then(|(p, b)| raw_document(&p, &b).ok().map(|d| (p, b, d)));
        let (source_name, source, initial) = match imported {
            Some(v) => v,
            None => ("baseline.wcraft.json".into(), serde_json::to_vec(doc).map_err(|e| e.to_string())?, doc.clone()),
        };
        let base = serde_json::to_value(&initial).map_err(|e| e.to_string())?;
        Ok(Self { base: base.clone(), last: base, steps: Vec::new(), source_name, source, observed: initial })
    }

    fn capture(&mut self, doc: &Document, command: &str, label: &str, selection: Value) -> Result<(), String> {
        if self.observed.same_content(doc) {
            return Ok(());
        }
        let next = serde_json::to_value(doc).map_err(|e| e.to_string())?;
        let mut changes = Vec::new();
        differences(&self.last, &next, &mut Vec::new(), &mut changes);
        if !changes.is_empty() {
            self.steps.push(Step { command: command.into(), label: label.into(), time: crate::cmd::now_iso(), selection, changes });
        }
        self.last = next;
        self.observed = doc.clone();
        Ok(())
    }

    pub fn bundle(&self) -> Result<Bundle, String> {
        Ok(Bundle {
            schema: 1,
            engine: ENGINE.into(),
            source_sha256: digest(&self.source),
            base_sha256: model_digest(&self.base)?,
            result_sha256: model_digest(&self.last)?,
            steps: self.steps.clone(),
        })
    }

    pub fn verify(&self) -> Result<Value, String> {
        let result = replay(&self.bundle()?, &self.source_name, &self.source)?;
        let replayed = serde_json::to_value(&result).map_err(|e| e.to_string())?;
        if replayed != self.last {
            return Err("Replayed model differs from the editor".into());
        }
        let actual: Document = serde_json::from_value(self.last.clone()).map_err(|e| e.to_string())?;
        let a = crate::io::save_bytes("verify.docx", &result)?;
        let b = crate::io::save_bytes("verify.docx", &actual)?;
        let a = wordcraft_docx::read(&a).map_err(|e| e.to_string())?;
        let b = wordcraft_docx::read(&b).map_err(|e| e.to_string())?;
        if !a.same_content(&b) {
            return Err("DOCX write/read verification differs".into());
        }
        Ok(json!({"success": true, "steps": self.steps.len(), "model": "equal", "docxRoundtrip": "equal"}))
    }

    pub fn export(&self, directory: &std::path::Path) -> Result<Value, String> {
        if directory.exists() {
            return Err("Export folder already exists; choose a new folder".into());
        }
        self.verify()?;
        let bundle = self.bundle()?;
        let data = serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())?;
        let ext = if self.source_name.to_ascii_lowercase().ends_with(".docx") { "docx" } else { "wcraft.json" };
        let input = directory.join(format!("original.{ext}"));
        let script = directory.join("replay.py");
        crate::io::create_directory(directory)?;
        crate::io::write_file(&input.to_string_lossy(), &self.source)?;
        crate::io::write_file(&directory.join("trace.json").to_string_lossy(), data.as_bytes())?;
        crate::io::write_file(&script.to_string_lossy(), python_script(&bundle)?.as_bytes())?;
        let expected: Document = serde_json::from_value(self.last.clone()).map_err(|e| e.to_string())?;
        crate::io::save_path(&directory.join("edited.docx"), &expected)?;
        crate::io::write_file(&directory.join("README.txt").to_string_lossy(), format!(
            "WordCraft Trace 0.1\n\n1. Keep the original file unchanged.\n2. Run: python3 replay.py original.{ext} replayed.docx --engine /path/to/wordcraft-cli\n3. Or import replay and call replay.transform(input_path, output_path).\n\nThe script contains all recorded structural operations and inserted media.\nIt requires this WordCraft Trace engine build. It validates the input SHA-256, each change, and the final model.\noriginal.{ext}: recorded baseline; edited.docx: expected result; trace.json: readable audit history.\nDOCX fidelity is limited by WordCraft's importer/exporter. Different templates are rejected.\nThe script writes atomically, and will not overwrite the input or an existing output.\n").as_bytes())?;
        Ok(json!({"directory": directory, "script": script, "input": input, "steps": self.steps.len()}))
    }
}

pub fn replay(bundle: &Bundle, input_name: &str, input: &[u8]) -> Result<Document, String> {
    if bundle.schema != 1 || bundle.engine != ENGINE {
        return Err("Trace schema/engine version mismatch".into());
    }
    if digest(input) != bundle.source_sha256 {
        return Err("Input differs from the recorded original (SHA-256 mismatch)".into());
    }
    let initial = raw_document(input_name, input)?;
    let initial = serde_json::to_value(initial).map_err(|e| e.to_string())?;
    if model_digest(&initial)? != bundle.base_sha256 {
        return Err("Input model differs from the recorded baseline".into());
    }
    let result = apply_steps(initial, &bundle.steps)?;
    if model_digest(&result)? != bundle.result_sha256 {
        return Err("Final document verification failed".into());
    }
    serde_json::from_value(result).map_err(|e| e.to_string())
}

pub fn replay_files(trace: &str, input: &str, output: &str) -> Result<(), String> {
    if !output.to_ascii_lowercase().ends_with(".docx") {
        return Err("Output must end in .docx".into());
    }
    let out = std::path::Path::new(output);
    if out.exists() {
        return Err("Output already exists; choose a new output path".into());
    }
    let bytes = crate::io::read_file_capped(trace, MAX_TRACE_BYTES, "trace")?;
    let bundle: Bundle = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let source = crate::io::read_document(std::path::Path::new(input))?;
    let document = replay(&bundle, input, &source)?;
    crate::io::save_path(out, &document)
}

pub fn python_script(bundle: &Bundle) -> Result<String, String> {
    let payload = serde_json::to_string_pretty(bundle).map_err(|e| e.to_string())?;
    let literal = payload.replace('\'', "\\u0027");
    let mut out = String::from(
        "#!/usr/bin/env python3\n\"\"\"Generated document edit program. Python 3 standard library only.\nRequires the matching WordCraft Trace native engine.\n\"\"\"\nimport argparse\nimport json\nimport os\nfrom pathlib import Path\nimport shutil\nimport subprocess\nimport tempfile\n\n",
    );
    for (i, step) in bundle.steps.iter().enumerate() {
        let label = step.label.replace(['\r', '\n'], " ");
        let command = step.command.replace(['\r', '\n'], " ");
        out.push_str(&format!("# Step {}: {} ({}) — {} structural changes\n", i + 1, label, command, step.changes.len()));
    }
    out.push_str(&format!("\nTRACE = json.loads(r'''\n{literal}\n''')\n"));
    let hint = std::env::current_exe().ok().map(|p| p.with_file_name("wordcraft-cli").to_string_lossy().into_owned()).unwrap_or_default();
    out.push_str(&format!("\nENGINE_HINT = {}\n", serde_json::to_string(&hint).map_err(|e| e.to_string())?));
    out.push_str(
        r#"
def transform(input_path, output_path=None, *, engine=None):
    """Replay this recorded edit sequence; return the absolute output DOCX path."""
    source = Path(input_path).expanduser().resolve(strict=True)
    target = Path(output_path).expanduser().absolute() if output_path else source.with_name(source.stem + '_replayed.docx')
    if source == target.resolve() or target.exists():
        raise ValueError('Choose a new output path; the original is never overwritten')
    candidates = [engine, os.environ.get('WORDCRAFT_TRACE_ENGINE'), ENGINE_HINT,
                  str(Path(__file__).resolve().parent / 'wordcraft-cli'),
                  str(Path(__file__).resolve().parent / 'WordCraft Trace.app/Contents/MacOS/wordcraft-cli'),
                  shutil.which('wordcraft-cli')]
    for folder in [Path(__file__).resolve().parent, *list(Path(__file__).resolve().parents)[:5],
                   Path('/Applications'), Path.home() / 'Applications']:
        candidates.extend(str(folder / name / 'Contents/MacOS/wordcraft-cli')
                          for name in ('WordCraftTrace.app', 'WordCraft Trace.app'))
    executable = next((str(p) for p in candidates if p and Path(p).is_file()), None)
    if not executable:
        raise RuntimeError('Set WORDCRAFT_TRACE_ENGINE or pass --engine /path/to/wordcraft-cli from WordCraft Trace')
    with tempfile.TemporaryDirectory(prefix='wordcraft-trace-') as temporary:
        recipe = Path(temporary) / 'trace.json'
        recipe.write_text(json.dumps(TRACE, ensure_ascii=False), encoding='utf-8')
        result = subprocess.run([executable, 'trace-replay', str(recipe), str(source), str(target)],
                                capture_output=True, text=True, check=False)
        if result.returncode:
            raise RuntimeError(result.stderr.strip() or result.stdout.strip() or 'Replay failed')
    if not target.is_file():
        raise RuntimeError('The engine did not create the output document')
    return str(target)

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input_path')
    parser.add_argument('output_path', nargs='?')
    parser.add_argument('--engine')
    args = parser.parse_args()
    print(transform(args.input_path, args.output_path, engine=args.engine))
"#,
    );
    Ok(out)
}

impl Session {
    pub fn trace_branch(&mut self, count: usize) -> Result<u64, String> {
        let journal = self.trace_current().ok_or("No trace for this document")?;
        let steps = journal.steps.get(..count).ok_or("Step is outside the trace history")?.to_vec();
        let last = apply_steps(journal.base.clone(), &steps)?;
        let document: Document = serde_json::from_value(last.clone()).map_err(|e| e.to_string())?;
        let branch = Journal {
            base: journal.base.clone(),
            last,
            steps,
            source_name: journal.source_name.clone(),
            source: journal.source.clone(),
            observed: document.clone(),
        };
        let id = self.add_document(document);
        self.file_name = Some(format!("Replayed step {count}.docx"));
        self.dirty = true;
        self.trace.journals.insert(id, branch);
        Ok(id)
    }

    pub fn trace_resume(&mut self, directory: &std::path::Path) -> Result<u64, String> {
        let data = crate::io::read_file_capped(&directory.join("trace.json").to_string_lossy(), MAX_TRACE_BYTES, "trace")?;
        let bundle: Bundle = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
        let path =
            if directory.join("original.wcraft.json").exists() { directory.join("original.wcraft.json") } else { directory.join("original.docx") };
        let source = crate::io::read_document(&path)?;
        let document = replay(&bundle, &path.to_string_lossy(), &source)?;
        let base = serde_json::to_value(raw_document(&path.to_string_lossy(), &source)?).map_err(|e| e.to_string())?;
        let last = serde_json::to_value(&document).map_err(|e| e.to_string())?;
        let journal =
            Journal { base, last, steps: bundle.steps, source_name: path.to_string_lossy().into_owned(), source, observed: document.clone() };
        let id = self.add_document(document);
        self.file_name = Some("Recorded document.docx".into());
        self.dirty = true;
        self.trace.enabled = true;
        self.trace.journals.insert(id, journal);
        Ok(id)
    }

    pub fn trace_sync(&mut self, command: &str, label: &str) {
        if !self.trace.enabled || self.trace.depth > 0 || self.trace.error.is_some() {
            return;
        }
        let id = self.document_id();
        if !self.trace.journals.contains_key(&id) {
            match Journal::new(&self.doc, self.path.as_deref()) {
                Ok(journal) => {
                    self.trace.journals.insert(id, journal);
                }
                Err(e) => {
                    self.trace.error = Some(e);
                    return;
                }
            }
        }
        let selection = serde_json::to_value(&self.sel).unwrap_or(Value::Null);
        if let Some(journal) = self.trace.journals.get_mut(&id)
            && let Err(e) = journal.capture(&self.doc, command, label, selection)
        {
            self.trace.error = Some(e);
        }
    }

    pub fn trace_current(&self) -> Option<&Journal> {
        self.trace.journals.get(&self.document_id())
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("officecli.validate", "Validate DOCX using OfficeCLI", "Trace", |s, _| {
            crate::officecli::execute(&s.doc, "validate", "").map(|v| json!({"report":v.report})).map_err(CmdError::Failed)
        })
        .pure(),
        CommandSpec::new("trace.exportArchive", "Export complete native trace archive", "Trace", |s, v| {
            let path = v.get("directory").and_then(Value::as_str).ok_or_else(|| CmdError::Params("directory is required".into()))?;
            s.trace_current().ok_or_else(|| CmdError::Failed("No trace".into()))?.export(std::path::Path::new(path)).map_err(CmdError::Failed)
        })
        .params(r#"{"directory": string}"#)
        .pure(),
        CommandSpec::new("ops.targets", "Inspect semantic targets", "Trace", |s, _| {
            let mut value = crate::io::semantic::compile(&json!({"action":"targets","base":s.doc})).map_err(CmdError::Failed)?;
            value["version"] = json!(fingerprint(&s.doc).map_err(CmdError::Failed)?);
            Ok(value)
        })
        .pure(),
        CommandSpec::new("ops.apply", "Apply semantic operations", "Trace", |s, v| {
            let expected = v.get("version").and_then(Value::as_str).ok_or_else(|| CmdError::Params("version from ops.targets is required".into()))?;
            if expected != fingerprint(&s.doc).map_err(CmdError::Failed)? {
                return Err(CmdError::Failed("Document version changed; inspect targets again".into()));
            }
            let ops = v.get("ops").filter(|v| v.is_array()).ok_or_else(|| CmdError::Params("ops array is required".into()))?;
            if let Some(items) = ops.as_array() {
                for op in items {
                    if let Some(path) = op.get("path").and_then(Value::as_str) {
                        crate::file_access::check_read(path).map_err(CmdError::Failed)?;
                    }
                }
            }
            let document = crate::io::semantic::apply(&s.doc, ops).map_err(CmdError::Failed)?;
            s.doc = document;
            s.sel = crate::Selection::caret(s.doc.start_of(wordcraft_doc::StoryRef::Body));
            s.pending = None;
            s.column = None;
            s.math = None;
            s.also_selected.clear();
            Ok(json!({"applied":ops.as_array().map_or(0,Vec::len),"version":fingerprint(&s.doc).map_err(CmdError::Failed)?}))
        })
        .params(r#"{"version": string, "ops": array}"#),
        CommandSpec::new("trace.ops", "Compact semantic operations", "Trace", |s, _| {
            s.trace_current().ok_or_else(|| CmdError::Failed("No trace".into()))?.compact().map_err(CmdError::Failed)
        })
        .pure(),
        CommandSpec::new("trace.replay", "Replay into a new document", "Trace", |s, v| {
            let count = match v.get("steps") {
                Some(n) => {
                    n.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| CmdError::Params("steps must be a nonnegative integer".into()))?
                }
                None => s.trace_current().map_or(0, |j| j.steps.len()),
            };
            s.trace_branch(count).map(|id| json!({"document":id})).map_err(CmdError::Failed)
        })
        .params(r#"{"steps"?: integer}"#)
        .pure(),
        CommandSpec::new("officecli.apply", "Apply OfficeCLI batch", "Trace › OfficeCLI", |s, v| {
            let expected = v.get("expected").and_then(Value::as_str).ok_or_else(|| CmdError::Params("expected fingerprint is required".into()))?;
            if fingerprint(&s.doc).map_err(CmdError::Failed)? != expected {
                return Err(CmdError::Failed("The document changed while OfficeCLI was working. Run the batch again.".into()));
            }
            let doc = v.get("document").ok_or_else(|| CmdError::Params("document is required".into()))?;
            let mut doc: Document = serde_json::from_value(doc.clone()).map_err(|e| CmdError::Params(e.to_string()))?;
            doc.ensure_nonempty();
            s.doc = doc;
            s.sel = crate::Selection::caret(s.doc.start_of(wordcraft_doc::StoryRef::Body));
            s.pending = None;
            s.column = None;
            s.math = None;
            s.also_selected.clear();
            Ok(json!({"applied":true}))
        })
        .params(r#"{"expected": string, "document": object}"#),
        CommandSpec::new("trace.begin", "Start automatic tracing", "Trace", |s, _| {
            s.trace.enabled = true;
            s.trace_sync("document.open", "Open document");
            Ok(json!({"recording": true}))
        })
        .pure(),
        CommandSpec::new("trace.status", "Trace status", "Trace", |s, _| {
            Ok(json!({"recording": s.trace.enabled, "steps": s.trace_current().map_or(0, |j| j.steps.len()), "error": s.trace.error}))
        })
        .pure(),
        CommandSpec::new("trace.verify", "Verify replay", "Trace", |s, _| {
            s.trace_current().ok_or_else(|| CmdError::Failed("No trace for this document".into()))?.verify().map_err(CmdError::Failed)
        })
        .pure(),
        CommandSpec::new("trace.export", "Export Python replay", "Trace", |s, v| {
            let path = v
                .get("directory")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| CmdError::Params("directory is required".into()))?;
            if let Some(e) = &s.trace.error {
                return Err(CmdError::Failed(e.clone()));
            }
            s.trace_current()
                .ok_or_else(|| CmdError::Failed("No trace for this document".into()))?
                .export_compact(std::path::Path::new(path))
                .map_err(CmdError::Failed)
        })
        .params(r#"{"directory": string}"#)
        .pure(),
        CommandSpec::new("trace.resume", "Resume recorded project", "Trace", |s, v| {
            let path = v
                .get("directory")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| CmdError::Params("directory is required".into()))?;
            s.trace_resume(std::path::Path::new(path)).map(|id| json!({"document":id})).map_err(CmdError::Failed)
        })
        .params(r#"{"directory": string}"#)
        .pure(),
    ]
}

#[cfg(test)]
mod tests;
