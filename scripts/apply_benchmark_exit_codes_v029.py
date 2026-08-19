from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:160]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once(
    "src/main.rs",
    """        if arg == \"--benchmark-material-eval\" {
            if let Some(value) = args.next() {
                return StartupMode::MaterialEval(PathBuf::from(value));
            }
        }""",
    """        if arg == \"--benchmark-material-eval\" {
            return StartupMode::MaterialEval(args.next().map(PathBuf::from).unwrap_or_default());
        }""",
)

replacements = [
    (
        'Err(err) => eprintln!("ANN benchmark failed: {err:#}"),',
        'Err(err) => benchmark_failed("ANN benchmark", &err),',
    ),
    (
        'Err(err) => eprintln!("CLIP preview benchmark failed: {err:#}"),',
        'Err(err) => benchmark_failed("CLIP preview benchmark", &err),',
    ),
    (
        'Err(err) => eprintln!("CLIP runtime benchmark failed: {err:#}"),',
        'Err(err) => benchmark_failed("CLIP runtime benchmark", &err),',
    ),
    (
        'Err(err) => eprintln!("Image model benchmark failed: {err:#}"),',
        'Err(err) => benchmark_failed("Image model benchmark", &err),',
    ),
    (
        'Err(err) => eprintln!("Library profile benchmark failed: {err:#}"),',
        'Err(err) => benchmark_failed("Library profile benchmark", &err),',
    ),
    (
        'Err(err) => eprintln!("Labeled material evaluation failed: {err:#}"),',
        'Err(err) => benchmark_failed("Labeled material evaluation", &err),',
    ),
    (
        'Err(err) => eprintln!("Material texture benchmark failed: {err:#}"),',
        'Err(err) => benchmark_failed("Material texture benchmark", &err),',
    ),
]
for old, new in replacements:
    replace_once("src/main.rs", old, new)

replace_once(
    "src/main.rs",
    """fn write_benchmark_report(destination: &Path, report: &str, label: &str) {
    match std::fs::write(destination, report) {
        Ok(()) => {
            println!(\"{report}\");
            println!(\"report={}\", destination.display());
        }
        Err(err) => {
            println!(\"{report}\");
            eprintln!(\"Cannot save {label} benchmark report: {err}\");
        }
    }
}

fn main() -> eframe::Result<()> {""",
    """fn write_benchmark_report(destination: &Path, report: &str, label: &str) {
    match std::fs::write(destination, report) {
        Ok(()) => {
            println!(\"{report}\");
            println!(\"report={}\", destination.display());
        }
        Err(err) => {
            println!(\"{report}\");
            eprintln!(\"Cannot save {label} benchmark report: {err}\");
        }
    }
}

fn benchmark_failed(label: &str, err: &anyhow::Error) -> ! {
    eprintln!(\"{label} failed: {err:#}\");
    std::process::exit(1);
}

fn main() -> eframe::Result<()> {""",
)

replace_once(
    "src/material_eval.rs",
    """pub fn benchmark(db_path: &Path, model_cache: &Path, manifest_path: &Path) -> Result<String> {
    let manifest_text = std::fs::read_to_string(manifest_path)""",
    """pub fn benchmark(db_path: &Path, model_cache: &Path, manifest_path: &Path) -> Result<String> {
    if manifest_path.as_os_str().is_empty() {
        bail!(\"--benchmark-material-eval requires a TSV manifest path\");
    }
    let manifest_text = std::fs::read_to_string(manifest_path)""",
)

replace_once(
    "src/material_eval.rs",
    """    #[test]
    fn manifest_supports_header_comments_relative_paths_and_same_group_dedup() {""",
    """    #[test]
    fn empty_manifest_path_reports_required_argument() {
        let error = benchmark(Path::new(\"unused.sqlite3\"), Path::new(\"models\"), Path::new(\"\"))
            .unwrap_err();
        assert!(error.to_string().contains(\"requires a TSV manifest path\"));
    }

    #[test]
    fn manifest_supports_header_comments_relative_paths_and_same_group_dedup() {""",
)
