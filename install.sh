#!/usr/bin/env bash
# mouse2joy Installer Script
# One-command installation & desktop integration for mouse2joy

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=== mouse2joy Installer ===${NC}"
echo ""

# Detect distribution
detect_distro() {
    if [ -f /etc/os-release ]; then
        . /etc/os-release
        DISTRO=$ID
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
        echo -e "${GREEN}✓ Rust found: $RUST_VERSION${NC}"
    else
        RUST_AVAILABLE=false
    fi
}

# Install Rust if missing
install_rust() {
    echo -e "${YELLOW}Rust not found. Installing via rustup...${NC}"
    if [ "$DISTRO" = "arch" ] || [ "$DISTRO" = "manjaro" ]; then
        sudo pacman -S --needed --noconfirm rustup
        rustup default stable
    elif [ "$DISTRO" = "fedora" ]; then
        sudo dnf install -y rust cargo
    elif [ "$DISTRO" = "opensuse" ]; then
        sudo zypper install -y rust cargo
    else
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        export PATH="$HOME/.cargo/bin:$PATH"
        rustup default stable
    fi
}

# Install PulseAudio and GTK4 development libraries if missing
install_deps() {
    echo "Checking dependencies..."
    # Check for GTK4
    if ! pkg-config --exists gtk4 2>/dev/null; then
        echo -e "${YELLOW}GTK4 development headers missing, installing...${NC}"
        if [ "$DISTRO" = "arch" ] || [ "$DISTRO" = "manjaro" ]; then
            sudo pacman -S --needed --noconfirm gtk4
        elif [ "$DISTRO" = "ubuntu" ] || [ "$DISTRO" = "debian" ]; then
            sudo apt-get update && sudo apt-get install -y libgtk-4-dev
        elif [ "$DISTRO" = "fedora" ]; then
            sudo dnf install -y gtk4-devel
        elif [ "$DISTRO" = "opensuse" ]; then
            sudo zypper install -y gtk4-devel
        fi
    fi

    # Check for libevdev
    if ! pkg-config --exists libevdev 2>/dev/null; then
        echo -e "${YELLOW}libevdev development headers missing, installing...${NC}"
        if [ "$DISTRO" = "arch" ] || [ "$DISTRO" = "manjaro" ]; then
            sudo pacman -S --needed --noconfirm libevdev
        elif [ "$DISTRO" = "ubuntu" ] || [ "$DISTRO" = "debian" ]; then
            sudo apt-get update && sudo apt-get install -y libevdev-dev
        elif [ "$DISTRO" = "fedora" ]; then
            sudo dnf install -y libevdev-devel
        elif [ "$DISTRO" = "opensuse" ]; then
            sudo zypper install -y libevdev-devel
        fi
    fi

    # Check for PulseAudio
    if ! pkg-config --exists libpulse-simple 2>/dev/null; then
        echo -e "${YELLOW}PulseAudio development headers missing, installing...${NC}"
        if [ "$DISTRO" = "arch" ] || [ "$DISTRO" = "manjaro" ]; then
            sudo pacman -S --needed --noconfirm libpulse
        elif [ "$DISTRO" = "ubuntu" ] || [ "$DISTRO" = "debian" ]; then
            sudo apt-get update && sudo apt-get install -y libpulse-dev
        elif [ "$DISTRO" = "fedora" ]; then
            sudo dnf install -y pulseaudio-libs-devel
        elif [ "$DISTRO" = "opensuse" ]; then
            sudo zypper install -y pulseaudio-devel
        fi
    fi
}

# Build the release binary
build_app() {
    echo -e "${BLUE}Building mouse2joy (release build)...${NC}"
    if [ -f "./Cargo.toml" ]; then
        cargo build --release
        BIN_SRC="./target/release/mouse2joy"
    else
        echo "Fetching source code..."
        BUILD_DIR=$(mktemp -d)
        git clone https://github.com/Orangericce/mouse2joy.git "$BUILD_DIR"
        cd "$BUILD_DIR"
        cargo build --release
        BIN_SRC="$BUILD_DIR/target/release/mouse2joy"
    fi

    if [ ! -f "$BIN_SRC" ]; then
        echo -e "${RED}Error: Build failed, binary not created.${NC}"
        exit 1
    fi
}

# Install binary to system
install_binary() {
    echo "Installing binary to /usr/local/bin..."
    BIN_DEST="/usr/local/bin/mouse2joy"
    sudo cp -f "$BIN_SRC" "$BIN_DEST"
    sudo chmod +x "$BIN_DEST"
    echo -e "${GREEN}✓ Binary installed: $BIN_DEST${NC}"
}

# Install Desktop Entry and Icons (for app menu launcher)
install_desktop_entry() {
    echo "Installing desktop application entry and icons..."

    # Determine resources path
    RES_DIR="resources"
    if [ ! -d "$RES_DIR" ] && [ -n "$BUILD_DIR" ]; then
        RES_DIR="$BUILD_DIR/resources"
    fi

    # 1. Desktop Entry
    DESKTOP_DIR="/usr/local/share/applications"
    sudo mkdir -p "$DESKTOP_DIR"
    if [ -f "$RES_DIR/mouse2joy.desktop" ]; then
        sudo cp -f "$RES_DIR/mouse2joy.desktop" "$DESKTOP_DIR/"
    else
        sudo bash -c "cat > $DESKTOP_DIR/mouse2joy.desktop" << 'EOF'
[Desktop Entry]
Version=1.0
Type=Application
Name=mouse2joy
GenericName=Mouse to Joystick Converter
Comment=Convert mouse input into a virtual joystick with a modern desktop GUI
Exec=mouse2joy
Icon=mouse2joy
Terminal=false
Categories=Utility;
Keywords=mouse;joystick;uinput;evdev;gamepad;accessibility;X-InputDevice;
StartupNotify=false
EOF
    fi
    sudo chmod 644 "$DESKTOP_DIR/mouse2joy.desktop"

    # Also place in user applications folder so desktop environments pick it up instantly
    USER_DESKTOP="$HOME/.local/share/applications"
    mkdir -p "$USER_DESKTOP"
    cp -f "$DESKTOP_DIR/mouse2joy.desktop" "$USER_DESKTOP/"

    # 2. Icons
    ICON_BASE="/usr/local/share/icons/hicolor"
    for sz in 16 22 24 32 48 64 128 256; do
        if [ -f "$RES_DIR/mouse2joy-$sz.png" ]; then
            sudo mkdir -p "$ICON_BASE/${sz}x${sz}/apps"
            sudo cp -f "$RES_DIR/mouse2joy-$sz.png" "$ICON_BASE/${sz}x${sz}/apps/mouse2joy.png"
        fi
    done

    # SVG Scalable Icon
    if [ -f "$RES_DIR/mouse2joy.svg" ]; then
        sudo mkdir -p "$ICON_BASE/scalable/apps"
        sudo cp -f "$RES_DIR/mouse2joy.svg" "$ICON_BASE/scalable/apps/mouse2joy.svg"
    fi

    # Update caches
    sudo update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
    update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
    sudo gtk-update-icon-cache -f -t "$ICON_BASE" 2>/dev/null || true

    echo -e "${GREEN}✓ App launcher installed (icon & menu entry)${NC}"
}

# Configure uinput permissions and rules
setup_permissions() {
    echo "Setting up /dev/uinput permissions..."

    # Install udev rule for persistent uinput access without root
    UDEV_RULE="/etc/udev/rules.d/99-mouse2joy-uinput.rules"
    sudo bash -c "cat > $UDEV_RULE" << 'EOF'
KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
EOF

    sudo udevadm control --reload-rules 2>/dev/null || true
    sudo udevadm trigger 2>/dev/null || true

    # Add current user to input group if needed
    if ! groups "$USER" | grep -q "\binput\b"; then
        echo -e "${YELLOW}Adding $USER to 'input' group for /dev/uinput access...${NC}"
        sudo usermod -aG input "$USER" 2>/dev/null || true
    fi

    # Immediate fix for current session
    if [ -c /dev/uinput ]; then
        sudo chmod 660 /dev/uinput 2>/dev/null || true
        sudo chgrp input /dev/uinput 2>/dev/null || true
    fi

    echo -e "${GREEN}✓ Uinput permissions configured${NC}"
}

# Default config creation
setup_config() {
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

# Spring-back centering strength (0.0 = no pull, 1.0 = instant snap)
center_strength_x = 0.04  # roll
center_strength_y = 0.02  # pitch

# Whether the stick continues to center while you keep the grab button pressed
center_while_grabbed = true

# Audio volume (0.0 = silent, 1.0 = max)
audio_volume = 0.5
EOF
        echo -e "${GREEN}✓ Default config created at $CONFIG_FILE${NC}"
    fi
}

# Run install flow
check_rust
if [ "$RUST_AVAILABLE" = false ]; then
    install_rust
fi

install_deps
build_app
install_binary
install_desktop_entry
setup_permissions
setup_config

echo ""
echo -e "${GREEN}=== Installation Complete! ===${NC}"
echo ""
echo "mouse2joy is now installed as a modern GTK4 desktop application."
echo ""
echo "How to launch:"
echo "  1. Search for 'mouse2joy' in your Linux application menu"
echo "  2. Or run: mouse2joy  (or: sudo mouse2joy)"
echo ""
echo "Configuration file:"
echo "  ~/.config/mouse2joy/config.toml"
echo ""
echo "Note: If running without sudo fails, log out and back in once for the 'input' group permissions to take effect."
