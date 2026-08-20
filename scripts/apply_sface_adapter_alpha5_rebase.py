from pathlib import Path

path = Path("src/main.rs")
text = path.read_text(encoding="utf-8")

old = "mod face_scope;\nmod face_store;"
new = "mod face_scope;\nmod face_sface_adapter;\nmod face_sface_benchmark;\nmod face_store;"
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"module anchor count={text.count(old)}")
    text = text.replace(old, new, 1)

old = "    FaceBenchmark(PathBuf),\n    FaceBenchmarkValidate(PathBuf),\n}"
new = "    FaceBenchmark(PathBuf),\n    FaceBenchmarkValidate(PathBuf),\n    SFaceBenchmark(PathBuf),\n}"
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"startup mode anchor count={text.count(old)}")
    text = text.replace(old, new, 1)

anchor = '''        if arg == "--benchmark-face" {\n            return StartupMode::FaceBenchmark(args.next().map(PathBuf::from).unwrap_or_default());\n        }\n'''
addition = '''        if let Some(value) = arg.strip_prefix("--benchmark-sface=") {\n            if !value.trim().is_empty() {\n                return StartupMode::SFaceBenchmark(PathBuf::from(value));\n            }\n        }\n        if arg == "--benchmark-sface" {\n            return StartupMode::SFaceBenchmark(args.next().map(PathBuf::from).unwrap_or_default());\n        }\n'''
if addition not in text:
    if text.count(anchor) != 1:
        raise SystemExit(f"CLI anchor count={text.count(anchor)}")
    text = text.replace(anchor, anchor + addition, 1)

old = '''fn face_benchmark_report_path(db_path: &Path) -> PathBuf {\n    benchmark_report_path(db_path, "face")\n}\n'''
new = '''fn face_benchmark_report_path(db_path: &Path) -> PathBuf {\n    benchmark_report_path(db_path, "face")\n}\n\nfn sface_benchmark_report_path(db_path: &Path) -> PathBuf {\n    benchmark_report_path(db_path, "sface")\n}\n'''
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"report path anchor count={text.count(old)}")
    text = text.replace(old, new, 1)

anchor = '''    if let StartupMode::FaceBenchmark(manifest_path) = &mode {\n        match face_benchmark::benchmark(manifest_path) {\n            Ok(report) => {\n                write_benchmark_report(&face_benchmark_report_path(&db_path), &report, "face")\n            }\n            Err(err) => benchmark_failed("Face benchmark", &err),\n        }\n        return Ok(());\n    }\n'''
addition = '''\n    if let StartupMode::SFaceBenchmark(manifest_path) = &mode {\n        match face_sface_benchmark::benchmark(manifest_path) {\n            Ok(report) => write_benchmark_report(\n                &sface_benchmark_report_path(&db_path),\n                &report,\n                "SFace ONNX",\n            ),\n            Err(err) => benchmark_failed("SFace ONNX benchmark", &err),\n        }\n        return Ok(());\n    }\n'''
if addition.strip() not in text:
    index = text.rfind(anchor)
    if index < 0:
        raise SystemExit("FaceBenchmark execution anchor not found")
    index += len(anchor)
    text = text[:index] + addition + text[index:]

path.write_text(text, encoding="utf-8")
print("SFace alpha5 rebase integration applied")
