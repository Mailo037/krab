# Developing Krab

## Building

```sh
cargo build --release
```

Linux needs a few system libraries; on Debian / Ubuntu:

```sh
sudo apt install pkg-config libclang-dev libxcb1-dev libxrandr-dev libdbus-1-dev \
                 libpipewire-0.3-dev libwayland-dev libegl-dev libgbm-dev
```

PipeWire 1.0 or newer is required (Ubuntu 24.04, Fedora 40, current Arch). Windows and macOS need nothing
beyond the Rust toolchain.

Install straight from the repository with `cargo install --git https://github.com/Mailo037/krab`. Builds
that run from a cargo `target` directory never update themselves.

Started from a source build (anything inside a cargo `target` folder), Krab neither installs nor updates itself.
Set `KRAB_NO_INSTALL=1` to stop a release build from installing itself on its first plain start.

## Testing

```sh
cargo test                                         # unit and integration tests
cargo test --test text_actions -- --ignored        # OCR test (needs an OCR engine)
cargo run --release -- --trace --standalone        # print startup timings on exit
```

`scripts/e2e-overlay.ps1` and `scripts/e2e-daemon.ps1` run the real application on Windows and drive it with
synthetic mouse and keyboard input (they take over the pointer for a few seconds).

CI builds and tests every commit on Windows, macOS and Linux (`.github/workflows/ci.yml`).

### Platform status

Windows is where Krab is developed and tested by hand, including the end-to-end scripts above. macOS and
Linux share all of that code and are built and unit-tested in CI on every commit, but have had less testing on
real desktops. Multi-monitor setups are supported by design but have mostly been exercised on a single display.

## Icons and images

- How the icons are drawn and how to add one: [ICONS.md](ICONS.md).
- `cargo run --example icon_sheet` previews all icons in `target/icon-sheet.png`.
- `cargo run --release --example gen_assets` regenerates the app icon and every README image. A synthetic
  desktop is drawn and the real overlay is driven with pointer and keyboard events, so the pictures show exactly
  what the code renders.

## How it is built

```text
src/
  main.rs        entry point: CLI, hand-off to a running instance, cold-start capture thread
  app.rs         winit event loop, overlay / status / settings windows, tray icon
  overlay/       the capture UI as a pure state machine (input events in, pixels out),
                 including the right-click menu (overlay/menu.rs)
  annotate.rs    markup objects: drawing and hit testing
  gfx/           canvas, text and icon rendering on a u32 pixel buffer
  brand.rs       the Krab mark (app and tray icon)
  capture.rs     screen capture front end
  platform/      GDI capture on Windows, xcap on macOS / Linux, autostart
  hud.rs         toast, countdown and recording indicator
  settings.rs    the settings window as a pure state machine (like the overlay)
  hotkey.rs      global shortcuts: registration, conflict status, recording key presses
  record.rs      region recording (ffmpeg or GIF)
  ocr.rs         text extraction and QR codes
  ipc.rs         single-instance hand-off over a local socket / named pipe
  update.rs      automatic updates from GitHub releases
  install.rs     per-user install: Start menu + Apps entry, Krab.app, .desktop file
```

The overlay knows nothing about windows or the operating system. That is what makes it testable - the test
suite drives it with synthetic pointer and keyboard events and checks the pixels that come out.

Why it starts so fast:

- **CPU rendering only.** Frames are drawn straight into the window's pixel buffer
  ([softbuffer](https://github.com/rust-windowing/softbuffer) + [tiny-skia](https://github.com/linebender/tiny-skia)),
  so there is no graphics device to create.
- **The screen is captured while the window system starts.** A worker thread grabs the monitors the instant the
  process starts; the event loop and the overlay windows come up in parallel.
- **Windows are reused.** The background instance keeps hidden overlay windows around, so a snip is just
  capture, draw, show.
- **Nothing is loaded that is not needed.** Icons are vector paths compiled into the binary, the font is parsed
  lazily glyph by glyph, the config is one small TOML file.

## Releases and automatic updates

1. Raise `version` in `Cargo.toml` and commit.
2. Push a tag with the same version: `git tag v0.3.0 && git push origin v0.3.0`.

`.github/workflows/release.yml` refuses tags that do not match `Cargo.toml`, builds all three platforms and
publishes, besides the archives, the bare executables (`krab-windows-x64.exe`, `krab-macos-universal`,
`krab-linux-x86_64`) and a `SHA256SUMS` file.

Installed copies update themselves from those files (`src/update.rs`):

- The background instance checks 20 seconds after it starts and then once a day; without a background instance,
  a snip starts the check in a separate process at most once a day. `krab --update` and the tray menu check
  immediately.
- The newest tag is read from the `releases/latest` redirect; the executable for the platform is downloaded
  next to the running one with the system's `curl`, checked against `SHA256SUMS`, started once with
  `--version`, and only then swapped in. Any failure leaves the installed version untouched.
- On Windows the running `krab.exe` is renamed to `krab.exe.old` (deleted on the next start); on macOS and
  Linux the file is replaced atomically.
- A background instance restarts into the new version once nothing is in progress.

The releases must be publicly downloadable: the updater does not send credentials, so a private repository
answers every request with 404. Set `auto_update = false` in the config or `KRAB_NO_UPDATE=1` in the
environment to turn the automatic check off.
