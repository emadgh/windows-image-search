# Face Search workflow

Face Search uses the portable `.imagesearch` face tables created by the YuNet + SFace pipeline.

## Indexed face suggestions

Opening **Face Search** loads a bounded gallery of searchable face instances from the currently available indexed roots. Only faces with current detection state and a current normalized face embedding are shown, so every suggestion can immediately be used as a query.

The initial gallery is capped at 240 face instances and balanced across roots. Each card renders the persisted face bounding box as a crop, shows detector confidence, and can be selected or double-clicked to search.

At this stage the gallery contains **face instances**, not unique people. The same person may therefore appear more than once when they occur in multiple indexed images. The People clustering milestone (#59) will group those repeated appearances into person-level suggestions.

## Search

The selected face is searched against compatible face-embedding revisions across all available portable roots. The UI exposes minimum similarity and Top-K controls. Results are collapsed to the best matching face per parent image and are shown in the normal image result list, ordered by identity similarity.

For face-search results, the matched face bounding box is overlaid on the result thumbnail and the normal score field contains face similarity.

## Pipeline prerequisites

1. Enable **Detect faces** for the collections that should participate.
2. Configure an external YuNet-compatible ONNX detector in Settings.
3. Configure an external SFace-compatible ONNX embedder in Settings.
4. Run the face pipeline, or let it run automatically after base indexing.

YuNet detection is followed by incremental SFace embedding backfill. No face-model weights are bundled or downloaded by the application.

## Remaining #58 work

An arbitrary external query image with a multi-face chooser is the remaining query-source slice before #58 can close. Person-level deduplication/clustering remains tracked separately by #59.
