# In-app image preview

Select or focus a displayed result and press **Space** to open a whole-image, contain-fit preview over the application. **Left** and **Right** move through the current filtered and sorted result order. Navigation also updates the focused selection. **Space** or **Escape** closes the preview. **Shift+Space** toggles the Inspector sidebar, including while preview mode is open.

Shortcuts do not activate while a text field or another keyboard-input widget has focus. Settings, Collections, Task Center, People Manager and close-confirmation windows keep their existing modal keyboard behavior.

Preview decoding runs on a background thread. Normal images are EXIF-oriented and reduced to at most 2560 pixels on the longest edge before GPU upload. Sources whose estimated RGBA decode would exceed 256 MiB, or whose file size exceeds the direct-decode safety threshold, use an existing 2048-pixel oversized derivative or current 512-pixel thumbnail without creating or modifying cache files. If neither exists, the preview reports that a rescan is required. This keeps very large images from causing an unbounded full-resolution allocation while still displaying the entire image content.
