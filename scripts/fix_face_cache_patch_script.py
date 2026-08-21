from pathlib import Path

path = Path(__file__).with_name("patch_face_cache_revisions.py")
text = path.read_text(encoding="utf-8")

replacements = [
    (
        '''replace(
    "src/face_similarity.rs",
    ''' + "'''" + '''              AND e.detector_version = f.detector_version
              AND e.detection_schema_version = f.schema_version
''' + "'''" + ''',
    ''' + "'''" + '''              AND e.detector_version = f.detector_version
              AND e.detector_cache_revision = f.detector_cache_revision
              AND e.detection_schema_version = f.schema_version
''' + "'''" + ''',
    2,
)
''',
        '''text = read("src/face_similarity.rs")
needle = "AND e.detector_version = f.detector_version\\n"
if text.count(needle) != 2:
    raise RuntimeError(f"src/face_similarity.rs: expected 2 embedding detector-version predicates, found {text.count(needle)}")
text = text.replace(
    needle,
    "AND e.detector_version = f.detector_version\\n              AND e.detector_cache_revision = f.detector_cache_revision\\n",
)
write("src/face_similarity.rs", text)
''',
    ),
    (
        '''replace(
    "src/face_similarity.rs",
    ''' + "'''" + '''              AND s.detector_version = f.detector_version
              AND s.schema_version = f.schema_version
''' + "'''" + ''',
    ''' + "'''" + '''              AND s.detector_version = f.detector_version
              AND s.detector_cache_revision = f.detector_cache_revision
              AND s.schema_version = f.schema_version
''' + "'''" + ''',
    2,
)
''',
        '''text = read("src/face_similarity.rs")
needle = "AND s.detector_version = f.detector_version\\n"
if text.count(needle) != 2:
    raise RuntimeError(f"src/face_similarity.rs: expected 2 detection-state detector-version predicates, found {text.count(needle)}")
text = text.replace(
    needle,
    "AND s.detector_version = f.detector_version\\n              AND s.detector_cache_revision = f.detector_cache_revision\\n",
)
write("src/face_similarity.rs", text)
''',
    ),
]

for old, new in replacements:
    if old not in text:
        raise RuntimeError("target similarity replacement block not found in patch script")
    text = text.replace(old, new, 1)

path.write_text(text, encoding="utf-8")
print("patch-script similarity replacements fixed")
