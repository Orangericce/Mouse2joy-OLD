# mouse2joy - Rust Implementation

**mouse2joy** converts mouse input into a virtual joystick using a modern GTK4 desktop GUI. Lightweight, Linux-native, with a blue-accented interface inspired by oneclient.

---

## What Has to Be Done Next (Roadmap)

> **Note for Contributors & Maintainers:** This section is kept up to date every time changes are made.

- [x] **Desktop GUI Application**: Complete rewrite from Ratatui TUI to GTK4 desktop GUI with native file dialogs, sliders with input boxes, toggle switches, and trigger button binding
- [x] **Desktop Integration**: Add `.desktop` launcher and multi-resolution app icons so `mouse2joy` appears as a native application in Linux launcher menus
- [x] **Improved Installer**: Smart `install.sh` that configures `/dev/uinput` permissions (udev rule + `input` group), installs PulseAudio headers if missing, compiles the release binary, and sets up desktop launchers
- [x] **Hot-Plug Detection**: App can launch without a mouse connected and detects/reconnects when a mouse is plugged in
- [ ] **Unit & Integration Tests**: Add automated unit test coverage for config parsing/saving, mouse auto-detection, and virtual joystick setup
- [ ] **Missing Example Files**: Provide `examples/minimal.rs` and `examples/integration.rs` referenced in docs
- [ ] **Restore & Fix GitHub Actions CI**: Re-add `.github/workflows/rust.yml` with proper system dependencies (`libpulse-dev`, `libevdev-dev`, `libgtk-4-dev`) to ensure clean builds on PRs
- [ ] **Systemd User Service (Optional Background Mode)**: Add a `systemd --user` service unit for users who prefer running `mouse2joy` continuously in the background
- [ ] **Cross-Platform Support**: Extend beyond Linux to Windows/macOS (future goal)

---

## Installation - Desktop Application

### Option 1: One-Command Installer (Recommended)

Run the automated installer script:

```bash
# Download and install mouse2joy (installs binary, uinput udev rules, desktop launcher & icons)
curl -fsSL https://raw.githubusercontent.com/Orangericce/mouse2joy/main/install.sh | bash
```

Or run locally from the source directory:
```bash
./install.sh
```

**What the installer does:**
1. Checks for Rust / `cargo` (installs via `rustup` if missing).
2. Installs required development libraries (`libpulse-dev` / `libpulse`, `libgtk-4-dev`, `libevdev-dev`).
3. Compiles the release binary (`target/release/mouse2joy`).
4. Installs the binary to `/usr/local/bin/mouse2joy`.
5. Installs the **desktop launcher** (`mouse2joy.desktop`) and application icons (`16x16` up to `256x256` + SVG) so `mouse2joy` appears in your Linux Application Menu.
6. Configures `/dev/uinput` permissions with a `/etc/udev/rules.d/99-mouse2joy-uinput.rules` udev rule and adds your user to the `input` group so you don't need `sudo` every time.
7. Initializes `~/.config/mouse2joy/config.toml` with default settings.

---

### Option 2: Manual Build (For Rust Developers)

If you already have Rust, GTK4, and PulseAudio libraries installed:

```bash
# Clone the repository
git clone https://github.com/Orangericce/mouse2joy.git
cd mouse2joy

# Build and install
cargo install --path . --locked

# Copy desktop launcher (optional)
sudo cp resources/mouse2joy.desktop /usr/local/share/applications/
```

---

## Running the Application

### 1. From Desktop Application Launcher
Search for **mouse2joy** in your Linux application launcher / GNOME Activities / KDE Application Menu / Rofi and click to open! It launches as a native GTK4 desktop application.

### 2. From Command Line
```bash
mouse2joy
```

*If you encounter permission issues with `/dev/uinput`, run with `sudo mouse2joy` or log out and log back in for the `input` group permissions to take effect.*

---

## Configuration

Settings are saved at `~/.config/mouse2joy/config.toml`. When the app first runs, it generates standard default values:

```toml
[mouse]
# Path to your mouse (auto-detected if empty)
device = ""

# Mouse button used to toggle grab mode (SIDE = Mouse Button 5)
grab_button = "SIDE"

# Mouse sensitivity (joystick movement multiplier)
sensitivity = 1000.0

# Spring-back centering strength (0.0 = float, 1.0 = instant center snap)
center_strength_x = 0.04  # Roll
center_strength_y = 0.02  # Pitch

# Centering active while grab is held
center_while_grabbed = true

# Audio volume (0.0 = silent, 1.0 = max)
audio_volume = 0.5
```

---

## Key Features

- **Modern Desktop GUI**: Native GTK4 application with blue accent theme (similar to oneclient), not a terminal interface
- **Lightweight & Fast**: Pure Rust with efficient physics simulation and lock-free atomic state (`std::sync::atomic`)
- **Live Settings**: Sliders with numeric input boxes (validation + clamping), toggle switches for boolean options
- **Trigger Button Binding**: Click to enter binding mode and press any mouse button on any detected mouse device to assign (no matter which device is selected for grab/movement)
- **Readable Button Names**: Displays "Left Click", "Right Click", "Side (Btn 4)", etc. instead of numeric codes
- **Native File Dialogs**: Export/Import configuration using GTK4 native file choosers
- **Device Selection**: Clear selection and display of the active mouse device used for grab/movement
- **Live Movement Display**: Status section always shows recent mouse movement (even when not grabbed) so you can verify input is working
- **Hot-Plug Support**: Can start without a mouse connected and automatically detects/connects when a mouse is plugged in
- **Audio Feedback**: Sine wave audio cues (880 Hz grab, 440 Hz ungrab) powered by PulseAudio
- **Auto-save**: Configuration auto-saves every 30 seconds and on window close
- **Zero Sudo Needed**: Automated udev rule setup so normal desktop users can capture `/dev/uinput`

---

## Controls

- **Configured Trigger Button** (default: Mouse Side Button / BTN_SIDE): Toggles mouse grab on/off
- **GUI Controls**: All settings adjustable via sliders, input boxes, toggles, and buttons
- **Window Close**: Gracefully quits the application, saves config, and restores normal cursor operation

---

## Uninstallation

### Option 1: Automated Uninstaller (Recommended)

Run the uninstaller script:
```bash
./uninstall.sh
```

This will remove:
- Binary (`/usr/local/bin/mouse2joy`)
- Desktop launcher and icons
- Udev rules are kept (they don't conflict)
- Optionally: configuration directory (`~/.config/mouse2joy`)

### Option 2: Manual Removal

```bash
# Remove binary & launchers
sudo rm -f /usr/local/bin/mouse2joy
sudo rm -f /usr/local/share/applications/mouse2joy.desktop
rm -f ~/.local/share/applications/mouse2joy.desktop
sudo rm -rf /usr/local/share/icons/hicolor/*/apps/mouse2joy.*

# Optional: Remove config directory
rm -rf ~/.config/mouse2joy
```

If you no longer need access to `/dev/uinput` and want to remove the `input` group membership:
```bash
sudo gpasswd -d $USER input
```

---

## License

MIT