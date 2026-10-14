# glycin-ng HEIF test artifacts

Used to verify the `heif` feature against the Ristretto AppImage build.

- `screenshots/ristretto-heif-rgb-strips.png` — Ristretto showing
  `rgb-strips-96.heic` (96x32, single `hvc1` item) at 100%.
- `screenshots/ristretto-heif-photo-2048.png` — Ristretto showing
  `photo-2048.heic` (2048x1536, 3x4 `grid` derivation) at 39.3%.
- `fixtures/glycin-ng-heif-test-images.tar.gz` — the two HEIC files,
  archived so GitHub does not transcode them.

The HEIC files come from the `heic-rs` generated fixture set
(`scripts/make-fixtures.sh`, synthesised PNGs encoded with Apple's
`sips`), licensed MIT OR Apache-2.0.
