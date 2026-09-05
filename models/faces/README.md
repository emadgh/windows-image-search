# Face models

These ONNX files are vendored so installed builds can bootstrap face search from this repository instead of depending on a third-party runtime URL.

- `face_detection_yunet_2026may.onnx` — OpenCV Zoo YuNet, MIT. SHA-256 `ebafce4e3c118d6554634be5c27ab333b4c047a9a8c3faf1d7cf93101c22f0f0`.
- `face_recognition_sface_2021dec.onnx` — OpenCV Zoo SFace, Apache-2.0. SHA-256 `0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79`.

The application downloads the managed defaults from this repository's `models/faces` paths, verifies exact file size and SHA-256, validates the ONNX session, and only then installs them into the local face-model cache.

Upstream source: https://github.com/opencv/opencv_zoo
