#!/usr/bin/env bash
# mouse2joy Installer Script
# Single-command installation for the Rust mouse-to-joystick converter

set -e

echo "=== mouse2joy Installer ==="
echo ""

# Detect distribution
detect_distro() {
    if [ -f /etc/os-release ]; then
        . /etc/os-release
        DISTRO=$ID
        VERSION_ID=$VERSION_ID
    else
        DISTRO="unknown"
    fi
}

detect_distro

# Check if cargo/rustc is available
check_rust() {
    if command -v cargo &>/dev/null && command -v rustc &>/dev/null; then
        RUST_AVAILABLE=true
        RUST_VERSION=$(rustc --version | awk '{print $2}')
        echo "Rust found: $RUST_VERSION"
    else
        RUST_AVAILABLE=false
    fi
}

# Install Rust if not available
install_rust() {
    echo "Rust not found. Installing via rustup..."
    if [ "$DISTRO" = "arch" ] || [ "$DISTRO" = "manjaro" ]; then
        sudo pacman -S --needed rustup
        rustup install stable
        rustup default stable
    elif [ "$DISTRO" = "ubuntu" ] || [ "$DISTRO" = "debian" ]; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        export PATH="$HOME/.cargo/bin:$PATH"
        rustup install stable
        rustup default stable
    elif [ "$DISTRO" = "fedora" ]; then
        sudo dnf install -y rust cargo
    elif [ "$DISTRO" = "opensuse" ]; then
        sudo zypper install -y rust cargo
    else
        echo "Unknown distribution. Trying rustup..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        export PATH="$HOME/.cargo/bin:$PATH"
        rustup install stable
        rustup default stable
    fi
}

# Build the release binary
build_app() {
    echo "Building mouse2joy..."
    cd /tmp/mouse2joy_build 2>/dev/null || mkdir -p /tmp/mouse2joy_build

    # Try cargo install first
    if [ "$RUST_AVAILABLE" = true ]; then
        cargo build --release 2>&1 | tail -1
    else
        cargo build --release 2>&1 | tail -1
    fi

    if [ $? -ne 0 ]; then
        echo "Build failed. Trying cargo install..."
        cargo install --path . --locked 2>&1 | tail -5
    fi
}

# Set up configuration
setup_config() {
    echo "Setting up configuration..."
    CONFIG_DIR="$HOME/.config/mouse2joy"
    mkdir -p "$CONFIG_DIR"

    CONFIG_FILE="$CONFIG_DIR/config.toml"
    if [ ! -f "$CONFIG_FILE" ]; then
        cat > "$CONFIG_FILE" << 'EOF'
[mouse]
# Path to your mouse (can be auto-detected if empty)
device = ""

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
EOF
        echo "Default config created at $CONFIG_FILE"
    fi

    # Set uinput permissions
    if [ -c /dev/uinput ]; then
        sudo chmod 666 /dev/uinput 2>/dev/null || echo "Warning: Could not set /dev/uinput permissions"
    fi
}

# Create desktop/launcher shortcut (optional)
create_shortcut() {
    echo "Creating launcher..."
    BIN_PATH="/usr/local/bin/mouse2joy"

    if [ ! -f "$BIN_PATH" ]; then
        # Copy the binary
        if [ -f "./target/release/mouse2joy" ]; then
            sudo cp ./target/release/mouse2joy "$BIN_PATH"
            sudo chmod +x "$BIN_PATH"
        elif command -v mouse2joy &>/dev/null; then
            # Already installed
            :
        else
            echo "Warning: Could not find binary to install"
        fi
    fi

    # Create a systemd service if available
    if [ -d /etc/systemd/system ]; then
        sudo bash -c 'cat > /etc/systemd/system/mouse2joy.service << EOF
[Unit]
Description=mouse2joy - Mouse to Joystick Converter
After=network.target

[Service]
Type=simple
ExecStart=mouse2joy
Restart=no

[Install]
WantedBy=multi-user.target
EOF'
        sudo systemctl daemon-reload 2>/dev/null || true
    fi
}

# Main installation flow
echo "Starting mouse2joy installation..."

# Check for rust
check_rust

# Install rust if needed
if [ "$RUST_AVAILABLE" = false ]; then
    install_rust
    check_rust
fi

# Build the application
build_app

# Set up configuration
setup_config

# Install binary
create_shortcut

echo ""
echo "=== Installation Complete ==="
echo ""
echo "mouse2joy has been installed successfully."
echo ""
echo "To run:"
echo "  sudo mouse2joy"
echo ""
echo "Configuration:"
echo "  ~/.config/mouse2joy/config.toml"
echo ""
echo "Enjoy your virtual joystick!"