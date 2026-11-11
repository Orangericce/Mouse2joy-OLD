#!/usr/bin/env bash
# mouse2joy Uninstaller Script
# Removes mouse2joy from your system

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=== mouse2joy Uninstaller ===${NC}"
echo ""
echo "This will remove mouse2joy from your system."
echo ""

# Remove binary
if [ -f /usr/local/bin/mouse2joy ]; then
    echo -e "${YELLOW}Removing /usr/local/bin/mouse2joy...${NC}"
    sudo rm -f /usr/local/bin/mouse2joy
    echo -e "${GREEN}✓ Binary removed${NC}"
else
    echo -e "${YELLOW}Binary not found at /usr/local/bin/mouse2joy${NC}"
fi

# Remove desktop launcher
DESKTOP_DIR="/usr/local/share/applications"

if [ -f "$DESKTOP_DIR/mouse2joy.desktop" ]; then
    echo -e "${YELLOW}Removing desktop launcher...${NC}"
    sudo rm -f "$DESKTOP_DIR/mouse2joy.desktop"
    echo -e "${GREEN}✓ Launcher removed${NC}"
fi

if [ -f "$HOME/.local/share/applications/mouse2joy.desktop" ]; then
    echo -e "${YELLOW}Removing desktop launcher from user directory...${NC}"
    rm -f "$HOME/.local/share/applications/mouse2joy.desktop"
    echo -e "${GREEN}✓ User launcher removed${NC}"
fi

# Update desktop database
sudo update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
sudo gtk-update-icon-cache -f -t /usr/local/share/icons/hicolor 2>/dev/null || true

# Remove icons
ICON_BASE="/usr/local/share/icons/hicolor"

if [ -d "$ICON_BASE" ]; then
    echo -e "${YELLOW}Removing icons...${NC}"
    sudo rm -rf "$ICON_BASE/*/apps/mouse2joy.*"
    echo -e "${GREEN}✓ Icons removed${NC}"
fi

# Optional: Remove config directory
echo ""
echo -e "${BLUE}Do you want to remove the configuration directory?${NC}"
echo -e "${YELLOW}This will delete all your settings and mappings!${NC}"
read -p "Remove config directory (Y/n)? " -n 1 -r
echo ""

if [[ ! $REPLY =~ ^[Nn]$ ]]; then
    if [ -d "$HOME/.config/mouse2joy" ]; then
        echo -e "${YELLOW}Removing $HOME/.config/mouse2joy...${NC}"
        rm -rf "$HOME/.config/mouse2joy"
        echo -e "${GREEN}✓ Config directory removed${NC}"
    else
        echo -e "${YELLOW}Config directory not found${NC}"
    fi
else
    echo -e "${YELLOW}Keeping config directory${NC}"
fi

echo ""
echo -e "${GREEN}=== Uninstallation Complete! ===${NC}"
echo ""
echo "mouse2joy has been removed from your system."
echo ""
echo "If you were a member of the 'input' group and no longer need access to /dev/uinput,"
echo "you can remove the group membership by running:"
echo "  sudo gpasswd -d $USER input"