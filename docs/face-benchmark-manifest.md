# Face benchmark manifest

The v0.3 face benchmark uses one UTF-8, tab-separated manifest per model/runtime candidate. Blank lines and lines starting with `#` are ignored. Coordinates are normalized to the EXIF-oriented image in `[0,1]`.

## Model metadata

Exactly one `model` row is required:

```text
model<TAB>model-id<TAB>model-version<TAB>execution-provider<TAB>license<TAB>redistributable<TAB>commercial-use<TAB>source-mode<TAB>source
```

`source-mode` is `bundled` or `external`. A model marked non-redistributable or non-commercial must be `external`; the benchmark will not treat restricted weights as bundleable application assets.

Example permissive candidate:

```text
model	yunet+sface	2023mar	CPU	Apache-2.0/MIT	true	true	bundled	https://github.com/opencv/opencv_zoo
```

Example user-supplied restricted candidate:

```text
model	user-insightface	local	DirectML	upstream-model-license	false	false	external	D:\Models\face-stack.onnx
```

## Detector evaluation

Declare every detector-evaluation image, including images that contain no faces:

```text
image	people/group-01.jpg
image	textures/marble-01.jpg
```

Each ground-truth face is one `gt` row:

```text
gt	people/group-01.jpg	0.10	0.12	0.20	0.24
```

Each model prediction is one `pred` row. The third column is confidence:

```text
pred	people/group-01.jpg	0.96	0.11	0.12	0.19	0.23
```

A no-face image has an `image` row and zero `gt` rows. Predictions on such an image count toward the no-face false-positive rate. Multiple `gt` and `pred` rows per image are supported. The evaluator greedily matches confidence-sorted predictions to unmatched ground-truth boxes using IoU >= 0.50; duplicate predictions therefore become false positives.

Detector report metrics include precision, recall, F1, false-positive rate on no-face images, and face-size recall buckets based on normalized face area:

- tiny: area <= 0.01
- small: 0.01 < area <= 0.04
- large: area > 0.04

## Identity / embedding evaluation

Each precomputed cosine-similarity comparison is one `identity` row:

```text
identity	query-face-id	query-person-id	candidate-face-id	candidate-person-id	cosine-similarity
```

Example:

```text
identity	q-001	person-a	c-101	person-a	0.91
identity	q-001	person-a	c-102	person-b	0.34
```

Do not include a query face as its own candidate. A query may have many candidates. The same query id must always use the same person id.

The evaluator reports Recall@1/5/10, MRR, same-person and different-person cosine-distance distributions (`1 - similarity`), and the best F1 similarity threshold from a deterministic sweep across `[-1,1]`.

## CLI

Validate syntax and licensing metadata without computing metrics:

```powershell
.\windows-image-search.exe --validate-face-benchmark .\face-benchmark.tsv
```

Evaluate the labeled detector predictions and/or identity scores:

```powershell
.\windows-image-search.exe --benchmark-face .\face-benchmark.tsv
```

The model-adapter work tracked separately can generate `pred` and `identity` rows for CPU/DirectML candidates while reusing this exact evaluator and dataset.
