# Face Search validation

Use this checklist for the v0.3 face-search workflow after configuring external YuNet and SFace ONNX models.

1. Enable **Detect faces** for at least one collection and run the face pipeline.
2. Open **Face Search** and confirm searchable database faces appear as cropped face cards.
3. Select or double-click a database face. Confirm parent images are ranked by identity similarity, the query parent image is excluded, and the matching face box is shown on result thumbnails.
4. Adjust **Minimum similarity** and **Top-K** and confirm result count/order updates on a new search.
5. Click **Face from file…** and choose an image containing multiple faces. Confirm every detected face is shown as a separate crop.
6. Select one external face and search. Confirm the external source image is not required to be indexed and results come from compatible persisted SFace embeddings.
7. Verify no-face external images report no detected faces without changing the existing index.
8. Verify missing/unavailable YuNet or SFace model paths produce a clear error instead of blocking the UI.

Database face suggestions are face instances until issue #59 adds persistent People clustering and unique-person representatives.
