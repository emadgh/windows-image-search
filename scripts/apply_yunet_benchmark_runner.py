from pathlib import Path


def replace_once(path: str, old: str, new: str):
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if old not in text:
        raise SystemExit(f"expected anchor missing in {path}: {old[:120]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once(
    "src/main.rs",
    "mod face_sface_benchmark;\n",
    "mod face_sface_benchmark;\nmod face_yunet_benchmark;\n",
)

replace_once(
    "src/main.rs",
    "    SFaceBenchmark(PathBuf),\n",
    "    YuNetBenchmark(PathBuf),\n    SFaceBenchmark(PathBuf),\n",
)

replace_once(
    "src/main.rs",
    "        if let Some(value) = arg.strip_prefix(\"--benchmark-sface=\") {\n",
    "        if let Some(value) = arg.strip_prefix(\"--benchmark-yunet=\") {\n"
    "            if !value.trim().is_empty() {\n"
    "                return StartupMode::YuNetBenchmark(PathBuf::from(value));\n"
    "            }\n"
    "        }\n"
    "        if arg == \"--benchmark-yunet\" {\n"
    "            return StartupMode::YuNetBenchmark(args.next().map(PathBuf::from).unwrap_or_default());\n"
    "        }\n"
    "        if let Some(value) = arg.strip_prefix(\"--benchmark-sface=\") {\n",
)

replace_once(
    "src/main.rs",
    "fn sface_benchmark_report_path(db_path: &Path) -> PathBuf {\n",
    "fn yunet_benchmark_report_path(db_path: &Path) -> PathBuf {\n"
    "    benchmark_report_path(db_path, \"yunet\")\n"
    "}\n\n"
    "fn sface_benchmark_report_path(db_path: &Path) -> PathBuf {\n",
)

replace_once(
    "src/main.rs",
    "    if let StartupMode::SFaceBenchmark(manifest_path) = &mode {\n",
    "    if let StartupMode::YuNetBenchmark(manifest_path) = &mode {\n"
    "        match face_yunet_benchmark::benchmark(manifest_path) {\n"
    "            Ok(report) => write_benchmark_report(\n"
    "                &yunet_benchmark_report_path(&db_path),\n"
    "                &report,\n"
    "                \"YuNet ONNX\",\n"
    "            ),\n"
    "            Err(err) => benchmark_failed(\"YuNet ONNX benchmark\", &err),\n"
    "        }\n"
    "        return Ok(());\n"
    "    }\n\n"
    "    if let StartupMode::SFaceBenchmark(manifest_path) = &mode {\n",
)
