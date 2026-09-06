# Face Search workflow

Face Search uses the portable `.imagesearch` face tables created by the YuNet + SFace pipeline.

## Indexed face suggestions

Opening **Face Search** loads a bounded gallery of searchable faces from the currently available indexed roots. Only faces with current detection state and a current normalized face embedding are shown, so every suggestion can immediately be used as a query.

The initial suggestion request is capped at 240 entries; the face-instance fallback is balanced across roots. Each card renders the persisted face bounding box as a crop and can be selected or double-clicked to search.

The gallery prefers effective People representatives with manual names and corrections. When People suggestions are unavailable it falls back to searchable face instances; in that fallback the same person can appear more than once.

## Search

The selected face is searched against compatible face-embedding revisions across all available portable roots. The UI exposes minimum similarity and Top-K controls. Results are collapsed to the best matching face per parent image and are shown in the normal image result list, ordered by identity similarity.

For face-search results, the matched face bounding box is overlaid on the result thumbnail and the score field contains face similarity. This is a ranking score, not identity probability. Detection confidence describes face detection, not identity matching.

## Pipeline prerequisites

1. Enable **Detect faces** for the collections that should participate.
2. Set up the managed YuNet detector or an external compatible ONNX detector in Preferences.
3. Set up the managed SFace embedder or an external compatible ONNX embedder in Preferences.
4. Run the face pipeline, or let it run automatically after base indexing.

YuNet detection is followed by incremental SFace embedding backfill and People maintenance. The application supports checksum-verified managed model downloads. Models are cached locally and inference does not upload source images.

## External query and People management

Choose **Face from file…**, select one of the detected face crops, then search. The external image need not be indexed. No-face images report that outcome without adding index records. The People manager supports naming, merges, splits, assignments, ignored faces, and representative selection; see [People management](people-management.md).
