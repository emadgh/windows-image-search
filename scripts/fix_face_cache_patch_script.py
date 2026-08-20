from pathlib import Path

path = Path(__file__).with_name("patch_face_cache_revisions.py")
text = path.read_text(encoding="utf-8")
for needle in [
    '''    ''' + '''2,\n)''',
]:
    pass

blocks = [
    '''    ''' + '''2,\n)''',
]

# The two similarity-query predicates occur once each in the current source.
# Relax only those two replace() assertions from count=2 to the default count=1.
for marker in [
    '''              AND e.detector_version = f.detector_version\n              AND e.detection_schema_version = f.schema_version\n''',
    '''              AND s.detector_version = f.detector_version\n              AND s.schema_version = f.schema_version\n''',
]:
    pos = text.find(marker)
    if pos < 0:
        raise RuntimeError(f"marker not found: {marker!r}")
    tail = text.find("    2,\n)", pos)
    if tail < 0:
        raise RuntimeError(f"count assertion not found after: {marker!r}")
    text = text[:tail] + ")" + text[tail + len("    2,\n)"):]

path.write_text(text, encoding="utf-8")
print("patch-script assertions fixed")
