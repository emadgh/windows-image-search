from pathlib import Path

main = Path("src/main.rs")
text = main.read_text(encoding="utf-8")
old = "mod portable;\nmod preview_benchmark;"
new = "mod portable;\nmod portable_verify;\nmod preview_benchmark;"
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"main module anchor count={text.count(old)}")
    text = text.replace(old, new, 1)

old = "    FaceBenchmarkValidate(PathBuf),\n}"
new = "    FaceBenchmarkValidate(PathBuf),\n    PortableVerify(PathBuf, bool),\n}"
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"startup mode anchor count={text.count(old)}")
    text = text.replace(old, new, 1)

anchor = '''        if arg == "--validate-face-benchmark" {\n            return StartupMode::FaceBenchmarkValidate(\n                args.next().map(PathBuf::from).unwrap_or_default(),\n            );\n        }\n'''
addition = '''        if let Some(value) = arg.strip_prefix("--verify-portable=") {\n            if !value.trim().is_empty() {\n                return StartupMode::PortableVerify(PathBuf::from(value), false);\n            }\n        }\n        if arg == "--verify-portable" {\n            return StartupMode::PortableVerify(\n                args.next().map(PathBuf::from).unwrap_or_default(),\n                false,\n            );\n        }\n        if let Some(value) = arg.strip_prefix("--verify-portable-deep=") {\n            if !value.trim().is_empty() {\n                return StartupMode::PortableVerify(PathBuf::from(value), true);\n            }\n        }\n        if arg == "--verify-portable-deep" {\n            return StartupMode::PortableVerify(\n                args.next().map(PathBuf::from).unwrap_or_default(),\n                true,\n            );\n        }\n'''
if addition not in text:
    if text.count(anchor) != 1:
        raise SystemExit(f"CLI anchor count={text.count(anchor)}")
    text = text.replace(anchor, anchor + addition, 1)

anchor = '''    // Keep GUI launch lightweight: database open/migration, portable-root hydration,\n'''
addition = '''    if let StartupMode::PortableVerify(root, deep) = &mode {\n        let verify_mode = if *deep {\n            portable_verify::VerifyMode::DeepFingerprint\n        } else {\n            portable_verify::VerifyMode::Quick\n        };\n        match portable_verify::verify_root(\n            root,\n            portable_verify::VerifyOptions {\n                mode: verify_mode,\n                ..portable_verify::VerifyOptions::default()\n            },\n            |_| {},\n        ) {\n            Ok(report) => println!("{}", report.render_text(root, verify_mode)),\n            Err(err) => benchmark_failed("Portable index verification", &err),\n        }\n        return Ok(());\n    }\n\n'''
if addition not in text:
    if text.count(anchor) != 1:
        raise SystemExit(f"execution anchor count={text.count(anchor)}")
    text = text.replace(anchor, addition + anchor, 1)

main.write_text(text, encoding="utf-8")
print("portable verifier alpha5 integration applied")
