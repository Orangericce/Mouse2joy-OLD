# mouse2joy - Rust Implementation

**mouse2joy** rewrites the Python mouse-to-joystick converter in Rust for better performance and a modern UI.

## Key Features

- **Fast & Safe**: Pure Rust implementation with efficient physics simulation
- **Terminal UI**: Rich terminal interface with real-time monitoring
- **Audio Feedback**: Pulsed sound on grab/ungrab with adjustable volume
- **Persistent Config**: TOML-based settings that survive restarts
- **Auto Detection**: Automatically find your mouse device
- **Spring-Back Center**: 60Hz physics with customizable centering strength

## Quick Start

```bash
cargo build --release
sudo ./target/release/mouse2joy
```

## Configuration

Settings are saved to `~/.config/mouse2joy/config.toml`:

```toml
[mouse]
device = "/dev/input/by-id/usb-Compx_VXE_NordicMouse_1K_Dongle-if02-event-mouse"
grab_button = "SIDE"  # mouse button 5
sensitivity = 1000.0
center_strength_x = 0.04
center_strength_y = 0.02
center_while_grabbed = true
audio_volume = 0.5  # 0.0 = silent, 1.0 = max
```

## UI Features

- **Status Display**: Grabbed/ungrabbed state with color coding
- **Axis Meters**: Live X/Y axis visualization
- **Setting Sliders**: Adjust sensitivity and centering in real-time
- **Sound Volume**: Control audio cue levels
- **Toggle Options**: Enable/disable centering while grabbed

## Audio System

- **Grab Sound**: Higher pitch (880Hz) with smooth fade-in
- **Ungrab Sound**: Lower pitch (440Hz) with gentle fade-out
- **Adjustable Volume**: From 0% to 100% of master volume
- **Background Compatible**: Works with any PulseAudio setup

## Controls

- **Mouse Side Button**: Toggle grab mode (exclusive capture)
- **Keyboard**: `Ctrl+C` to quit cleanly
- **UI**: Real-time updates without blocking

## Performance

- **60Hz Centering**: Smooth spring-back physics
- **Atomic Operations**: Lock-free state management
- **Streamlined I/O**: Efficient event processing
- **Low Memory**: Under 20MB total footprint

## Files

- **src/main.rs**: Core application logic
- **Cargo.toml**: Dependencies and build configuration
- **examples/minimal.rs**: Minimal configuration example
- **resources/icons/mouse2joy.svg**: Custom UI icons (optional)

## Technical Details

- **Dependencies**: evdev, ratatui, crossterm, serde, toml, dirs, PulseAudio
- **Architecture**: Modular design with input, joystick, centering, audio, and UI components
- **Safety**: Memory-safe Rust with proper error handling
- **Portability**: Works on any Linux system with evdev support

## Building

```bash
cargo build        # Debug build
cargo build --release  # Optimized release build
```

## License

MIT