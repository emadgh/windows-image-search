from pathlib import Path

main = Path('src/main.rs')
text = main.read_text(encoding='utf-8')
marker = 'mod people_clustering;\n'
if 'mod people_effective;\n' not in text:
    if marker not in text:
        raise SystemExit('people module marker not found')
    text = text.replace(marker, marker + 'mod people_effective;\n', 1)
main.write_text(text, encoding='utf-8')

path = Path('src/people_effective.rs')
text = path.read_text(encoding='utf-8')
text = text.replace(
    '#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub enum EffectivePersonSource',
    '#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]\npub enum EffectivePersonSource',
    1,
)
old = '''    for cluster in automatic_clusters.values() {\n        let candidate = (\n            cluster.representative_library_id.as_str(),\n            cluster.representative_face_id.as_str(),\n        );\n        if !member_set.contains(&candidate) {\n            continue;\n        }\n        let belongs = effective_members.iter().any(|member| {\n            member.library_id == cluster.representative_library_id\n                && member.face_id == cluster.representative_face_id\n                && member.person_id.as_deref() == Some(manual_person_id)\n                && member.source == Some(EffectivePersonSource::Manual)\n        });\n        if belongs {\n            return Some((candidate.0.to_owned(), candidate.1.to_owned()));\n        }\n    }\n    None\n'''
new = '''    automatic_clusters\n        .values()\n        .filter_map(|cluster| {\n            let candidate = (\n                cluster.representative_library_id.as_str(),\n                cluster.representative_face_id.as_str(),\n            );\n            if !member_set.contains(&candidate) {\n                return None;\n            }\n            let belongs = effective_members.iter().any(|member| {\n                member.library_id == cluster.representative_library_id\n                    && member.face_id == cluster.representative_face_id\n                    && member.person_id.as_deref() == Some(manual_person_id)\n                    && member.source == Some(EffectivePersonSource::Manual)\n            });\n            belongs.then(|| (candidate.0.to_owned(), candidate.1.to_owned()))\n        })\n        .min()\n'''
if old not in text:
    raise SystemExit('deterministic representative block not found')
text = text.replace(old, new, 1)
path.write_text(text, encoding='utf-8')
