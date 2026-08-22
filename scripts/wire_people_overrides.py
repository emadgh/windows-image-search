from pathlib import Path

path = Path('src/main.rs')
text = path.read_text(encoding='utf-8')
marker = 'mod people_clustering;\n'
if 'mod people_overrides;\n' not in text:
    if marker not in text:
        raise SystemExit('people module marker not found')
    text = text.replace(marker, marker + 'mod people_overrides;\n', 1)
path.write_text(text, encoding='utf-8')
