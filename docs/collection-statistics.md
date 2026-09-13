# Collection management and statistics

The Collections window has three columns: collection navigation, Contents, and Settings with Statistics. Collection and folder rows align their text to the left. On narrow screens, horizontal scrolling preserves the three columns. Library-wide indexing controls remain in the footer.

Select a collection for its combined statistics, select a folder for that folder and its descendants, or choose **All collection images** to return to the combined view. Settings always apply to the selected collection, including when Statistics is scoped to a folder.

Statistics are collected by one background worker at a time. Selection changes discard the old result. Use **Refresh statistics** for a new snapshot; while indexing, snapshots refresh approximately every 15 seconds after the previous result. Filesystem latency can delay completion. Each SQLite database has its own read transaction, so the combined result is not an atomic snapshot across all roots and cache files.

- Indexed images, CLIP vectors, visual/color/material descriptors, file formats, source sizes and recorded decode failures come from the session database. Discovered images include indexed members not present in the discovery table. Overlapping folder and explicit file memberships do not double count images.
- Thumbnail counts represent existing, non-empty portable or fallback cache files keyed to current source metadata. They do not represent a historical generation count or certify that a cache file decodes. Unavailable sources cannot have their current cache identity verified.
- Face detection records, detected faces and stored face embeddings come from each image's owning portable root. Stored records do not certify freshness against the current detector or recognition model. A structurally incomplete embedding is not counted as stored. An unreadable root produces a warning and marks the face totals as partial.
- Source size is the sum saved in indexed image records. Thumbnail storage is the sum of the matching cache files found during the scan. Recorded failures can overlap the count of images without index records.

Statistics open existing databases with SQLite read-only flags. They do not invoke schema migration, attach a portable root, generate thumbnails, decode/delete invalid cache files, or update an index. Automated fixtures verify scope boundaries, deduplication, missing-database handling, partial face results, and unchanged database/cache bytes. A headless egui regression checks the horizontal position of sidebar text.
