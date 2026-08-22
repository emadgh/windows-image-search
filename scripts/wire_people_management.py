from pathlib import Path
p=Path('src/main.rs')
s=p.read_text(encoding='utf-8')
needle='mod people_effective;\nmod people_overrides;'
if needle not in s:
    raise SystemExit('main module insertion point missing')
s=s.replace(needle,'mod people_effective;\nmod people_management;\nmod people_overrides;',1)
p.write_text(s,encoding='utf-8')
