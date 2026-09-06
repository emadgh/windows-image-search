"""Export a read-only portable-index review worksheet; labels stay unassigned."""
import argparse
import csv
import json
import math
from pathlib import Path
import sqlite3
import struct


def export(root: Path, destination: Path) -> dict:
    source = (root / ".imagesearch" / "index.sqlite3").resolve(strict=True)
    # An existing directory might contain human labels. Never overwrite it.
    destination.mkdir(parents=True, exist_ok=False)
    with sqlite3.connect(source.as_uri() + "?mode=ro", uri=True) as connection:
        connection.execute("BEGIN")
        images = connection.execute("SELECT count(*) FROM images").fetchone()[0]
        faces = connection.execute(
            "SELECT face_id,image_path,face_ordinal,bbox_x,bbox_y,bbox_width,bbox_height "
            "FROM faces ORDER BY image_path,face_ordinal"
        ).fetchall()
        vectors = connection.execute(
            "SELECT dimension,normalized,embedding FROM face_embeddings"
        ).fetchall()
        invalid = 0
        for dimension, normalized, blob in vectors:
            if dimension <= 0 or len(blob) != 4 * dimension or normalized != 1:
                invalid += 1
                continue
            values = struct.unpack("<" + "f" * dimension, blob)
            norm = math.sqrt(sum(value * value for value in values))
            if not all(math.isfinite(value) for value in values) or abs(norm - 1) > 0.01:
                invalid += 1
        summary = {
            "images": images,
            "stored_faces": len(faces),
            "stored_embeddings": len(vectors),
            "invalid_or_unnormalized_stored_vectors": invalid,
            "independent_identity_labels": 0,
            "identity_accuracy": "not_evaluated",
            "detection_recall": "not_evaluated",
            "note": "Stored row counts do not establish current searchable coverage or identity correctness. Use the Rust root benchmark for revision-compatible retrieval.",
        }
    with (destination / "face-labels.csv").open("x", newline="", encoding="utf-8-sig") as file:
        writer = csv.writer(file)
        writer.writerow(["face_id", "relative_image_path", "ordinal", "x", "y", "width", "height",
                         "human_identity", "valid_face", "split", "review_notes"])
        for row in faces:
            writer.writerow([*row, "", "", "", ""])
    with (destination / "summary.json").open("x", encoding="utf-8") as file:
        json.dump(summary, file, indent=2)
    (destination / "README.txt").write_text(
        "Review each source image and its proposed face box. Fill human_identity with an anonymous "
        "consistent label, valid_face with yes/no, and split with calibration/heldout. Leave uncertain "
        "identities blank. Review complete images separately for missed faces; this worksheet contains "
        "detected boxes only and cannot measure detector recall. Automatic People groups are not labels. "
        "Keep this worksheet private; it contains local image paths and face references. This is a review "
        "worksheet, not an executable benchmark manifest. See docs/face-benchmark-manifest.md for the "
        "evaluation input format after review.\n", encoding="utf-8"
    )
    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    print(json.dumps(export(args.root, args.destination), indent=2))
