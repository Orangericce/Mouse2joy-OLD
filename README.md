# mouse2joy - Rust Implementation

**mouse2joy** rewrites the Python mouse‑to‑joystick converter in Rust for better performance and a modern UI.

## Key Features

- **Fast & Safe**: Pure Rust implementation with efficient physics simulation and lock‑free atomic state.
- **Terminal UI**: Rich Ratatui interface with real‑time monitoring of axes, settings, and audio volume.
- **Audio Feedback**: PulseAudio‑driven sine wave cues (880 Hz grab, 440 Hz ungrab) with smooth fade‑in/fade‑out and adjustable master volume.
- **Persistent Config**: TOML‑based settings that survive application restarts.
- **Auto Detection**: Scans `/dev/input/by‑id/` for a mouse device and falls back to the configured path.
- **Spring‑Back Center**: 60 Hz physics loop; X and Y centering strengths are configurable (default 0.04 / 0.02).
- **Lightweight**: Under 20 MB of RAM and ~2 MB binary (static release build). Requires only common Linux user‑space tools (evdev, PulseAudio).

## Quick Start

```bash
# First time (install dependencies if needed)
cargo build --release

# Run (requires sudo for /dev/uinput and mouse access)
sudo ./target/release/mouse2joy
```

## Configuration

All settings are stored in `~/.config/mouse2joy/config.toml`. When the program first runs, it writes a sensible defaults file if none exists:

```toml
[mouse]
# Path to your mouse (can be auto‑detected if empty)
device = "/dev/input/by-id/usb-Compx_VXE_NordicMouse_1K_Dongle-if02-event-mouse"

# Which mouse button is the grab toggle (key codes match evdev BTN_* constants)
grab_button = "SIDE"      # BTN_SIDE  → mouse button 5

# Mouse sensitivity (joystick units per raw event unit)
sensitivity = 1000.0

# Spring‑back centering strength (0.0 = no pull, 1.0 = instant snap)
center_strength_x = 0.04  # roll
center_strength_y = 0.02  # pitch

# Whether the stick continues to center while you keep the grab button pressed
center_while_grabbed = true

# Audio volume (0.0 = silent, 1.0 = max)
audio_volume = 0.5
```

**You can edit this file at any time while the app is running.**

**Note on grab_button** – Supported values include:
- Human‑readable names: `SIDE`, `MIDDLE`, `LEFT`, `RIGHT` (maps to `BTN_SIDE`, `BTN_MIDDLE`, etc.).
- Numeric codes: `5` (BTN_SIDE), `4` (BTN_MIDDLE), etc.
- `BTN_SIDE` also corresponds to mouse button 5 on most desktop mice.

## UI Features

- **Status Bar** (top) – Shows **GRABBED** in green or **RELEASED** in gray; updates live when you press the grab button.
- **Axis Meters** (middle) – Two vertical gauges with colored bars. The numeric X/Y values under them are the latest joystick positions sent to the virtual stick.
- **Settings Panel** (bottom) – A list of real‑time values you can tweak:
  - **Sensitivity** – How fast the virtual axis moves when you move the mouse.
  - **Center X** – Roll strength (X‑axis spring‑back).
  - **Center Y** – Pitch strength (Y‑axis spring‑back).
  - **Volume** – Audio cue level (percentage).
  - **Center While Grabbed** – Toggle; “ON”/“OFF” line reflects the current boolean.
- **Quit** – Any key (defaults to `q`) closes the UI gracefully and saves the config.

All values are updated via channels and displayed by the Ratatui terminal renderer at ~60 Hz, giving a responsive feel.

## Audio System

- **Grab cue** – Higher‑pitch sine wave (880 Hz, 100 ms, smooth fade‑in).
- **Ungrab cue** – Lower‑pitch sine wave (440 Hz, 100 ms, gentle fade‑out).
- **Volume** – Adjustable from 0 % to 100 % of the PulseAudio master volume.
- **Compatibility** – Works with any PulseAudio server (local ALSA, PipeWire, etc.). No extra dependencies beyond `libpulse‑binding` and `libpulse‑simple‑binding`.

## Controls

- **Mouse side button** (default button 5) – Toggles grab mode. When grabbed, the mouse is captured (hidden from other applications); when released, the OS cursor returns.
- **Keyboard** – `Ctrl+C` during the terminal UI closes the program cleanly (saves config, centers joystick, plays exit cue).
- **UI** – Press `q` in the terminal to quit; the program emits a final "ungrab” sound and restores mouse visibility.

## Performance

- **60 Hz Centering Loop** – Dedicated thread runs at 60 Hz, using atomic fields for X/Y to avoid mutexes.
- **Lock‑Free State** – All state (`x`, `y`, `grabbed`, etc.) is managed via `std::sync::atomic` primitives, ensuring no data races and minimal contention.
- **Streamlined I/O** – `evdev` reads raw events directly (no polling) and only buffers them while grabbed.
- **Low Memory Footprint** – All buffers are small; the whole program stays well under 20 MB even with the Ratatui runtime.

## Files

- **`src/main.rs`** – Core application: mouse input, virtual joystick, centering physics, audio output, terminal UI, and config handling.
- **`Cargo.toml`** – Dependencies and release‑profile optimizations (LTO, static linking friendly).
- **`examples/minimal.rs`** – Example of a minimal config for a custom mouse or alternative key codes.
- **`resources/`** – Placeholder for custom tray or terminal icons (optional).

## Technical Details

- **Stack**: `evdev` (Linux input handling), `ratatui` + `crossterm` (TUI), `serde`/`toml` (config), `dirs` (XDG config directory), `libpulse‑binding` + `libpulse‑simple‑binding` (audio).
- **Architecture**: One `SharedState` struct holds all mutable data as atomic fields; threads communicate via channels (input → state, state → UI, centering → uinput).
- **Safety**: All access is `unsafe`‑free; the code uses Rust’s ownership and atomic APIs to guarantee memory safety and data integrity.
- **Portability**: Runs on any Linux distribution that exposes `/dev/input` and `/dev/uinput` (standard on desktop/embedded). No graphical window system required.

## Testing

```bash
# Unit tests (if any)
cargo test

# Integration test (spawn a dummy mouse device and check output)
# See `examples/integration.rs` for a minimal smoke‑test.
```

## License

MIT