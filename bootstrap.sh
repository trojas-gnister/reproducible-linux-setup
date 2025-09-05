#!/bin/bash

# Bootstrap script to install dependencies needed to build and run the Rust application on a fresh Fedora install.
# This includes Rust toolchain, Git for cloning the repo, and specified system packages.
# Run this as a regular user; it will use sudo where needed.

set -e

echo "🚀 Starting bootstrap setup for Reproducible Desktop Setup System..."

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

print_status() { echo -e "${BLUE}[INFO]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
print_warning() { echo -e "${YELLOW}[WARNING]${NC} $1"; }
print_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Check if running on Fedora
if ! grep -q "Fedora" /etc/os-release; then
    print_warning "This script is designed for Fedora. Continue anyway? (y/N)"
    read -r response
    if [[ ! "$response" =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Update system
print_status "Updating system packages..."
sudo dnf update -y
print_success "System updated"

# Install dnf-plugins-core for copr
print_status "Installing dnf-plugins-core..."
sudo dnf install -y dnf-plugins-core
print_success "dnf-plugins-core installed"

# Enable COPR repo for COSMIC
print_status "Enabling COPR repo for COSMIC..."
sudo dnf copr enable -y ryanabx/cosmic-epoch
print_success "COPR repo enabled"

# Install required dependencies
print_status "Installing dependencies: git, cargo, rust, neovim, cosmic-term, kitty, tmux..."
sudo dnf install -y git cargo rust neovim cosmic-term kitty tmux
print_success "Dependencies installed"

print_success "Bootstrap completed! Now you can:"
echo "1. git clone <repo-url>"
echo "2. cd <repo-dir>"
echo "3. cargo build --release"
echo "4. ./target/release/repro-setup --config config.toml"
