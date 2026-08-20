from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:220]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once("Cargo.toml", 'version = "0.3.0-alpha.2"', 'version = "0.3.0-alpha.3"')

replace_once(
    "src/main.rs",
    "mod face_detection;\nmod face_pipeline;\n",
    "mod face_benchmark;\nmod face_detection;\nmod face_pipeline;\n",
)

replace_once(
    "src/main.rs",
    "    MaterialTextureBenchmark(usize),\n}",
    "    MaterialTextureBenchmark(usize),\n    FaceBenchmark(PathBuf),\n    FaceBenchmarkValidate(PathBuf),\n}",
)

replace_once(
    "src/main.rs",
    "        if arg == \"--benchmark-library-profile\" {\n            return StartupMode::LibraryProfile;\n        }\n",
    "        if arg == \"--benchmark-library-profile\" {\n            return StartupMode::LibraryProfile;\n        }\n        if let Some(value) = arg.strip_prefix(\"--benchmark-face=\") {\n            if !value.trim().is_empty() {\n                return StartupMode::FaceBenchmark(PathBuf::from(value));\n            }\n        }\n        if arg == \"--benchmark-face\" {\n            return StartupMode::FaceBenchmark(args.next().map(PathBuf::from).unwrap_or_default());\n        }\n        if let Some(value) = arg.strip_prefix(\"--validate-face-benchmark=\") {\n            if !value.trim().is_empty() {\n                return StartupMode::FaceBenchmarkValidate(PathBuf::from(value));\n            }\n        }\n        if arg == \"--validate-face-benchmark\" {\n            return StartupMode::FaceBenchmarkValidate(\n                args.next().map(PathBuf::from).unwrap_or_default(),\n            );\n        }\n",
)

replace_once(
    "src/main.rs",
    "fn material_texture_benchmark_report_path(db_path: &Path) -> PathBuf {\n    benchmark_report_path(db_path, \"material-texture\")\n}\n",
    "fn material_texture_benchmark_report_path(db_path: &Path) -> PathBuf {\n    benchmark_report_path(db_path, \"material-texture\")\n}\n\nfn face_benchmark_report_path(db_path: &Path) -> PathBuf {\n    benchmark_report_path(db_path, \"face\")\n}\n",
)

replace_once(
    "src/main.rs",
    "    if let StartupMode::MaterialTextureBenchmark(sample_count) = &mode {\n        match texture_benchmark::benchmark(&db_path, *sample_count) {\n            Ok(report) => write_benchmark_report(\n                &material_texture_benchmark_report_path(&db_path),\n                &report,\n                \"material texture\",\n            ),\n            Err(err) => benchmark_failed(\"Material texture benchmark\", &err),\n        }\n        return Ok(());\n    }\n",
    "    if let StartupMode::MaterialTextureBenchmark(sample_count) = &mode {\n        match texture_benchmark::benchmark(&db_path, *sample_count) {\n            Ok(report) => write_benchmark_report(\n                &material_texture_benchmark_report_path(&db_path),\n                &report,\n                \"material texture\",\n            ),\n            Err(err) => benchmark_failed(\"Material texture benchmark\", &err),\n        }\n        return Ok(());\n    }\n\n    if let StartupMode::FaceBenchmarkValidate(manifest_path) = &mode {\n        match face_benchmark::validate_manifest(manifest_path) {\n            Ok(report) => println!(\"{report}\"),\n            Err(err) => benchmark_failed(\"Face benchmark manifest validation\", &err),\n        }\n        return Ok(());\n    }\n\n    if let StartupMode::FaceBenchmark(manifest_path) = &mode {\n        match face_benchmark::benchmark(manifest_path) {\n            Ok(report) => write_benchmark_report(\n                &face_benchmark_report_path(&db_path),\n                &report,\n                \"face\",\n            ),\n            Err(err) => benchmark_failed(\"Face benchmark\", &err),\n        }\n        return Ok(());\n    }\n",
)

replace_once(
    "docs/face-benchmark-manifest.md",
    ".\\windows-image-search.exe --benchmark-face-eval .\\face-benchmark.tsv",
    ".\\windows-image-search.exe --benchmark-face .\\face-benchmark.tsv",
)

replace_once(
    "src/face_benchmark.rs",
    '''fn validate_identity_consistency(pairs: &[IdentityPair]) -> Result<()> {\n    let mut query_people: HashMap<&str, &str> = HashMap::new();\n    let mut seen_pairs = BTreeSet::new();\n    for pair in pairs {\n        if let Some(existing) = query_people.insert(&pair.query_id, &pair.query_person) {\n            if existing != pair.query_person {\n                bail!(\n                    "identity query {:?} is assigned to multiple person ids ({:?}, {:?})",\n                    pair.query_id,\n                    existing,\n                    pair.query_person\n                );\n            }\n        }\n        if !seen_pairs.insert((pair.query_id.as_str(), pair.candidate_id.as_str())) {\n            bail!(\n                "duplicate identity score for query {:?} and candidate {:?}",\n                pair.query_id,\n                pair.candidate_id\n            );\n        }\n    }\n    Ok(())\n}\n''',
    '''fn validate_identity_consistency(pairs: &[IdentityPair]) -> Result<()> {\n    let mut face_people: HashMap<&str, &str> = HashMap::new();\n    let mut seen_pairs = BTreeSet::new();\n    for pair in pairs {\n        for (face_id, person_id) in [\n            (pair.query_id.as_str(), pair.query_person.as_str()),\n            (pair.candidate_id.as_str(), pair.candidate_person.as_str()),\n        ] {\n            if let Some(existing) = face_people.insert(face_id, person_id) {\n                if existing != person_id {\n                    bail!(\n                        "identity face {:?} is assigned to multiple person ids ({:?}, {:?})",\n                        face_id,\n                        existing,\n                        person_id\n                    );\n                }\n            }\n        }\n        if !seen_pairs.insert((pair.query_id.as_str(), pair.candidate_id.as_str())) {\n            bail!(\n                "duplicate identity score for query {:?} and candidate {:?}",\n                pair.query_id,\n                pair.candidate_id\n            );\n        }\n    }\n    Ok(())\n}\n''',
)

replace_once(
    "src/face_benchmark.rs",
    '''    #[test]\n    fn duplicate_identity_pair_is_rejected() {\n        let content = format!(\n            "{}identity\\tq\\tp1\\tc\\tp2\\t0.2\\nidentity\\tq\\tp1\\tc\\tp2\\t0.3\\n",\n            model()\n        );\n        let path = temp_manifest("duplicate-pair", &content);\n        let error = load_manifest(&path).unwrap_err().to_string();\n        assert!(error.contains("duplicate identity score"));\n        let _ = std::fs::remove_file(path);\n    }\n''',
    '''    #[test]\n    fn duplicate_identity_pair_is_rejected() {\n        let content = format!(\n            "{}identity\\tq\\tp1\\tc\\tp2\\t0.2\\nidentity\\tq\\tp1\\tc\\tp2\\t0.3\\n",\n            model()\n        );\n        let path = temp_manifest("duplicate-pair", &content);\n        let error = load_manifest(&path).unwrap_err().to_string();\n        assert!(error.contains("duplicate identity score"));\n        let _ = std::fs::remove_file(path);\n    }\n\n    #[test]\n    fn identity_face_person_assignment_must_be_consistent_for_candidates_too() {\n        let content = format!(\n            "{}identity\\tq1\\tp1\\tc\\tp2\\t0.2\\nidentity\\tq2\\tp3\\tc\\tp4\\t0.3\\n",\n            model()\n        );\n        let path = temp_manifest("candidate-person-conflict", &content);\n        let error = load_manifest(&path).unwrap_err().to_string();\n        assert!(error.contains("assigned to multiple person ids"));\n        let _ = std::fs::remove_file(path);\n    }\n''',
)
