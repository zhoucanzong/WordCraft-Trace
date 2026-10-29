use super::*;
use crate::Selection;
use wordcraft_doc::{Pos, StoryRef};

fn traced(text: &str) -> Session {
    let mut s = Session::new(Document::from_text(text));
    s.run("trace.begin", &json!({})).unwrap();
    s
}

#[test]
fn nested_structural_diff_roundtrip_and_conflict() {
    let a = json!({"body":[{"text":"你好", "bold":false},{"text":"tail"}], "media":{}, "old": 1});
    let b = json!({"body":[{"text":"你好世界", "bold":true},{"text":"insert"},{"text":"tail"}], "media":{"img":"AA=="}, "new":null});
    let mut changes = Vec::new();
    differences(&a, &b, &mut Vec::new(), &mut changes);
    let step = Step { command: "test".into(), label: "Test".into(), time: String::new(), selection: Value::Null, changes };
    assert_eq!(apply_steps(a.clone(), std::slice::from_ref(&step)).unwrap(), b);
    assert!(apply_steps(json!({}), &[step]).is_err());
}

#[test]
fn typing_formatting_undo_redo_and_direct_updates_are_replayable() {
    let mut s = traced("Original title\nSecond paragraph");
    s.run("select.text", &json!({"text":"Original"})).unwrap();
    s.run("text.insert", &json!({"text":"年度"})).unwrap();
    s.run("select.all", &json!({})).unwrap();
    s.run("format.bold", &json!({})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    s.run("edit.redo", &json!({})).unwrap();
    s.doc.core.title = "External direct update".into();
    s.trace_sync("document.change", "Document update");
    let j = s.trace_current().unwrap();
    assert!(j.steps.iter().any(|x| x.command == "edit.undo"));
    assert!(j.steps.iter().any(|x| x.command == "edit.redo"));
    assert!(j.verify().is_ok());
}

#[test]
fn failed_and_nonmutating_commands_do_not_create_steps() {
    let mut s = traced("abc");
    let count = s.trace_current().unwrap().steps.len();
    assert!(s.run("no.such.command", &json!({})).is_err());
    s.run("caret.docEnd", &json!({})).unwrap();
    assert_eq!(s.trace_current().unwrap().steps.len(), count);
}

#[test]
fn selected_range_is_not_needed_at_replay_time() {
    let mut s = traced("first\nsecond");
    s.sel = Selection { anchor: Pos::new(StoryRef::Body, wordcraft_doc::Path::top(1), 0), focus: Pos::body(1, 6) };
    s.run("text.insert", &json!({"text":"replacement"})).unwrap();
    assert!(s.trace_current().unwrap().verify().is_ok());
}

#[test]
fn table_and_embedded_media_survive_trace() {
    let mut s = traced("Table demo");
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("insert.table", &json!({"rows":2,"cols":3})).unwrap();
    // Media is carried by the structural transaction, never by a mutable source path.
    s.doc.media.insert("test.png".into(), std::sync::Arc::new(vec![1, 2, 3, 4]));
    s.trace_sync("insert.picture", "Insert picture");
    let j = s.trace_current().unwrap();
    assert_eq!(apply_steps(j.base.clone(), &j.steps).unwrap(), j.last);
    assert!(j.verify().is_ok());
}

#[test]
fn input_and_recipe_tampering_are_rejected() {
    let mut s = traced("abc");
    s.run("text.insert", &json!({"text":"x"})).unwrap();
    let j = s.trace_current().unwrap();
    let mut b = j.bundle().unwrap();
    assert!(replay(&b, &j.source_name, b"wrong input").is_err());
    b.result_sha256 = "wrong".into();
    assert!(replay(&b, &j.source_name, &j.source).is_err());
}

#[test]
fn unicode_quotes_and_code_in_text_export_as_data() {
    let mut s = traced("abc");
    s.run("text.insert", &json!({"text":"'''\\\"\n中文😀 __import__('os')"})).unwrap();
    let code = python_script(&s.trace_current().unwrap().bundle().unwrap()).unwrap();
    assert!(code.contains("\\u0027"));
    let mut imported = s.trace_current().unwrap().bundle().unwrap();
    imported.steps[0].command = "text.insert\nraise RuntimeError('injected')\r".into();
    let code = python_script(&imported).unwrap();
    assert!(!code.lines().any(|line| line.starts_with("raise RuntimeError")));
    assert!(s.trace_current().unwrap().verify().is_ok());
}

#[test]
fn saved_project_resumes_without_losing_prior_history() {
    let dir = std::env::temp_dir().join(format!("wordcraft-trace-resume-test-{}", std::process::id()));
    let mut s = traced("Original");
    s.run("text.insert", &json!({"text":"first "})).unwrap();
    let count = s.trace_current().unwrap().steps.len();
    s.trace_current().unwrap().checkpoint(&dir).unwrap();
    let mut resumed = Session::new(Document::new());
    resumed.trace_resume(&dir).unwrap();
    assert_eq!(resumed.trace_current().unwrap().steps.len(), count);
    resumed.run("caret.docEnd", &json!({})).unwrap();
    resumed.run("text.insert", &json!({"text":" next"})).unwrap();
    assert_eq!(resumed.trace_current().unwrap().steps.len(), count + 1);
    resumed.trace_current().unwrap().verify().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn historical_replay_branches_without_overwriting_the_open_document() {
    let mut s = traced("Original");
    let original_id = s.document_id();
    s.run("text.insert", &json!({"text":"first "})).unwrap();
    let first = fingerprint(&s.doc).unwrap();
    s.run("text.insert", &json!({"text":"second "})).unwrap();
    let final_state = fingerprint(&s.doc).unwrap();
    s.run("trace.replay", &json!({"steps":1})).unwrap();
    assert_ne!(s.document_id(), original_id);
    assert_eq!(fingerprint(&s.doc).unwrap(), first);
    assert_eq!(s.trace_current().unwrap().steps.len(), 1);
    s.run("text.insert", &json!({"text":"branch "})).unwrap();
    s.trace_current().unwrap().verify().unwrap();
    s.run("view.switchWindows", &json!({"id":original_id})).unwrap();
    assert_eq!(fingerprint(&s.doc).unwrap(), final_state);
}

#[test]
fn compact_python_replays_unicode_formatting_without_native_engine() {
    let mut s = traced("Hello 世界 😀");
    s.run("select.text", &json!({"text":"世界"})).unwrap();
    s.run("text.insert", &json!({"text":"你好"})).unwrap();
    s.run("select.text", &json!({"text":"你好"})).unwrap();
    s.run("format.bold", &json!({})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    s.run("edit.redo", &json!({})).unwrap();
    let plan = s.trace_current().unwrap().compact().unwrap();
    assert_eq!(plan["ops"].as_array().unwrap().len(), 2);
    assert_eq!(plan["ops"][0]["start"], 6);
    assert_eq!(plan["ops"][0]["end"], 8);
    assert!(!plan["code"].as_str().unwrap().contains("wordcraft-cli"));
    let dir = std::env::temp_dir().join(format!("compact-export-test-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    let result = s.trace_current().unwrap().export_compact(&dir).unwrap();
    assert_eq!(result["verified"], true);
    let mut resumed = Session::new(Document::new());
    resumed.trace_resume(&dir).unwrap();
    assert_eq!(resumed.trace_current().unwrap().steps.len(), s.trace_current().unwrap().steps.len());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn semantic_batch_is_atomic_undoable_and_rejects_stale_versions() {
    let mut s = traced("Hello 世界");
    let version = fingerprint(&s.doc).unwrap();
    let bad = json!({"version":version,"ops":[{"op":"replace_text","target":"p0","start":6,"end":8,"text":"你好"},{"op":"unknown"}]});
    assert!(s.run("ops.apply", &bad).is_err());
    assert_eq!(fingerprint(&s.doc).unwrap(), version);
    let args = json!({"version":version,"ops":[{"op":"replace_text","target":"p0","start":6,"end":8,"text":"你好"},{"op":"format_text","target":"p0","start":6,"end":8,"bold":true}]});
    s.run("ops.apply", &args).unwrap();
    assert!(serde_json::to_string(&s.doc).unwrap().contains("你好"));
    assert!(s.run("ops.apply", &args).is_err());
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(fingerprint(&s.doc).unwrap(), version);
}

#[test]
fn native_docx_input_can_be_saved_over_without_changing_the_baseline() {
    let dir = std::env::temp_dir().join(format!("wordcraft-trace-source-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("source.docx");
    crate::io::save_path(&path, &Document::from_text("源文件\n第二段")).unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut s = Session::new(crate::io::open_path(&path).unwrap());
    s.path = Some(path.clone());
    s.run("trace.begin", &json!({})).unwrap();
    s.run("text.insert", &json!({"text":"修改："})).unwrap();
    s.run("file.save", &json!({"path":path})).unwrap();
    let j = s.trace_current().unwrap();
    assert_eq!(j.source, original);
    j.verify().unwrap();
    assert!(replay(&j.bundle().unwrap(), &path.to_string_lossy(), &std::fs::read(&path).unwrap()).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn officecli_commit_rejects_stale_results_and_is_undoable() {
    let mut s = traced("before");
    let expected = fingerprint(&s.doc).unwrap();
    let edited = Document::from_text("after");
    s.run("officecli.apply", &json!({"expected":expected,"document":edited})).unwrap();
    assert!(s.trace_current().unwrap().steps.iter().any(|x| x.command == "officecli.apply"));
    assert!(s.run("officecli.apply", &json!({"expected":expected,"document":edited})).is_err());
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(fingerprint(&s.doc).unwrap(), expected);
    s.trace_current().unwrap().verify().unwrap();
}

#[test]
#[ignore = "requires the bundled OfficeCLI binary via OFFICECLI_PATH"]
fn officecli_real_batch_and_validation_are_recorded_and_replayed() {
    let mut s = traced("Original text");
    let batch = r#"[{"command":"set","path":"/body/p[1]","props":{"text":"Edited by OfficeCLI"}}]"#;
    let result = crate::officecli::execute(&s.doc, "batch", batch).unwrap();
    s.run("officecli.apply", &json!({"expected":result.expected,"document":result.document.unwrap()})).unwrap();
    assert!(serde_json::to_string(&s.doc).unwrap().contains("Edited by OfficeCLI"));
    let check = crate::officecli::execute(&s.doc, "validate", "").unwrap();
    assert!(!check.report.is_empty());
    s.trace_current().unwrap().verify().unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(serde_json::to_string(&s.doc).unwrap().contains("Original text"));
    assert!(crate::officecli::execute(&s.doc, "batch", r#"[{"command":"set","path":"/body/p[999]","props":{"text":"bad"}}]"#).is_err());
}
