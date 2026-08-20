# Face model licensing policy

Status: v0.3 Face Search & People

Last upstream review: 2026-08-20

This document is an engineering distribution policy, not legal advice. A model may only be auto-downloaded or bundled when its model-weight license and provenance explicitly permit redistribution and the intended commercial use. A permissive software/code license does not automatically grant the same rights for pretrained weights or training data.

## Distribution states

- `bundled`: the checked model artifact has an upstream license that permits redistribution and commercial use, subject to that license's notice/attribution obligations.
- `external`: the application may load a model file supplied by the user, but does not download, redistribute, cache from a restricted upstream source, or package the weights.
- `blocked`: the application must not use the artifact until its license/provenance is resolved.

If license metadata is missing, ambiguous, contradictory, or changes upstream, fail closed: treat the candidate as `external` or `blocked`, never silently promote it to `bundled`.

## Initial candidate matrix

| Candidate | Role | Weight/source license verified | Commercial use | Redistribution | v0.3 policy | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| OpenCV Zoo YuNet `face_detection_yunet_2026may.onnx` | detector | MIT, model-directory `LICENSE` | Yes | Yes, with MIT notice | `bundled` candidate | Benchmark against the existing detector contract before selection. The older `2023mar` family is covered by the same directory license but remains a separate model revision. |
| OpenCV Zoo SFace `face_recognition_sface_2021dec.onnx` | embedder | Apache-2.0, model-directory `LICENSE` | Yes | Yes, subject to Apache-2.0 redistribution/notice terms | `bundled` candidate | Keep model revision explicit in persisted embedding metadata. |
| InsightFace SCRFD pretrained weights | detector | InsightFace code is MIT; upstream states training data and models trained with it are non-commercial research only | No by default | No by default | `external` only | A separately licensed artifact may be reconsidered only when its exact license is supplied and recorded. |
| InsightFace ArcFace / `buffalo_l` pretrained recognition weights | embedder | InsightFace code is MIT; upstream states downloaded pretrained models follow non-commercial research policy and directs users to licensing contacts for recognition packages | No by default | No by default | `external` only | Never auto-download or bundle the upstream pretrained package under the default policy. |
| User-supplied ONNX detector/embedder | either | User-provided / unknown | Unknown | Unknown | `external` only | Benchmarking is allowed when the user supplies the file; manifest metadata must identify source/license and may not claim `bundled` unless redistribution + commercial-use flags are both true. |

The machine-readable mirror is `docs/face-model-candidates.tsv` and should be updated together with this document.

## Upstream evidence checked

### OpenCV Zoo YuNet

The current OpenCV Zoo `models/face_detection_yunet` directory contains a dedicated `LICENSE` file. That license is MIT and grants use, modification, distribution, sublicensing, and sale subject to retaining the copyright/license notice. The directory currently exposes `face_detection_yunet_2026may.onnx` as well as the `2023mar` variants.

Evidence path: `opencv/opencv_zoo: models/face_detection_yunet/LICENSE` and the model directory listing.

### OpenCV Zoo SFace

The current OpenCV Zoo `models/face_recognition_sface` directory contains a dedicated Apache License 2.0 file and the `face_recognition_sface_2021dec.onnx` model family. Apache-2.0 permits use and redistribution subject to its license/notice requirements.

Evidence path: `opencv/opencv_zoo: models/face_recognition_sface/LICENSE` and the model directory listing.

### InsightFace / SCRFD / ArcFace

The current InsightFace README distinguishes code from model/data rights: it states that InsightFace code is MIT and available for academic/commercial use, while training data with annotations and models trained with those data are for non-commercial research only. It further states that manually and automatically downloaded models follow that policy, and lists separate licensing contacts for open-source face-recognition model packages such as `buffalo_l`.

Therefore the default application policy is not to redistribute InsightFace pretrained detector or recognition weights. Their algorithms/code may be useful references, and user-supplied or separately licensed ONNX artifacts may be benchmarked, but the exact artifact license must be recorded independently.

Evidence path: `deepinsight/insightface: README.md`, `License` section (reviewed 2026-08-20).

## Runtime enforcement requirements

The benchmark manifest introduced in v0.3.0-alpha.3 already records model id/version, execution provider, license, redistribution permission, commercial-use permission, source mode, and provenance. It rejects a model declared `bundled` when either redistribution or commercial use is false.

Model adapters in #93 must preserve that boundary:

1. A built-in downloader/installer may only target allow-listed `bundled` candidates with pinned artifact identity and matching license metadata.
2. Restricted candidates must require an explicit local path and remain `external`; no application-managed download URL is allowed.
3. Hash/model revision and license metadata must be written into benchmark reports so results are attributable to the exact artifact tested.
4. Changing a model file, version, or license state creates a new candidate revision; do not silently reuse benchmark conclusions from another revision.
5. Packaged Windows builds must include required third-party license/NOTICE text for any bundled model.
6. The application remains fully local/offline at inference time; licensing policy must not introduce a cloud dependency.

## Review gate before production selection

#62 may select a production detector/embedder only after #93 supplies accuracy/performance measurements and this matrix confirms a distributable path. If a restricted external model wins technically, it may remain an optional user-supplied adapter, but it cannot become the default bundled model without separate rights.