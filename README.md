# Reproducible Linux Setup

A powerful Rust-based tool for automating Fedora Linux desktop environment setup with containerized applications and dotfiles management.

## 🚀 Features

- **Fedora Support**: Optimized for Fedora Linux with dnf package manager
- **Dotfiles Management**: Safe migration of `.bashrc` and `.config` directories with backups
- **Container Management**: Simplified Podman container deployment with flexible flag support
- **Desktop Environment**: Desktop environment installation and configuration with display manager setup
- **VPN Integration**: Automated WireGuard setup with NetworkManager
- **Package Management**: Modular system and Flatpak package installation from separate TOML files
- **Custom Commands**: Execute additional shell commands in sequence with run-once support
- **Interactive Setup**: User confirmation prompts for safe configuration migration

## 📋 Requirements

- **Supported OS**: Fedora Linux
- **Dependencies**: `sudo` access for system modifications
- **Optional**: Podman for container support

## 🛠️ Installation

### From Source
```bash
git clone <repository-url>
cd repro-setup
cargo build --release
```

### Quick Setup
```bash
# Generate initial configuration from current system state
./target/release/repro-setup --initial

# Run with default configuration (uses config/config.toml)
./target/release/repro-setup

# Run with verbose logging for detailed output
./target/release/repro-setup --verbose

# Auto-answer yes to all prompts (unattended mode)
./target/release/repro-setup --yes

# Auto-answer no to all prompts (safe mode)
./target/release/repro-setup --no

# Run with custom config file
./target/release/repro-setup --config my-config/config.toml
```

## 🎛️ CLI Options

| Flag | Description |
|------|-------------|
| `--initial` | Generate initial configuration files from current system state |
| `--config <path>` | Use custom configuration file (default: `config/config.toml`) |
| `--verbose, -v` | Enable verbose logging for detailed output |
| `--yes, -y` | Automatically answer yes to all prompts (unattended mode) |
| `--no, -n` | Automatically answer no to all prompts (safe mode) |
| `--help, -h` | Show help information |
| `--version` | Show version information |

**Note**: `--yes` and `--no` flags cannot be used together.

## 📖 Configuration

The setup is controlled via configuration files in the `config/` directory:

- `config/config.toml` - Main configuration file
- `config/system-packages.toml` - System packages to install via dnf
- `config/flatpak-packages.toml` - Flatpak applications to install

### Initial Setup

For first-time setup, use the `--initial` flag to generate package configuration files from your current system:

```bash
./target/release/repro-setup --initial
```

This will:
- Scan your system for user-installed packages using `dnf repoquery --leaves --userinstalled`
- Scan for installed Flatpak applications using `flatpak list --app`
- Generate `config/system-packages.toml` and `config/flatpak-packages.toml`
- Create the `config/` directory structure

After running `--initial`, create your main `config/config.toml` file and run the tool again without the flag.

Here's the structure:

### Main Configuration (config/config.toml)
```toml
distro = "fedora"

[system]
hostname = "my-desktop" 
enable_amd_gpu = false
enable_rpm_fusion = true

[desktop]
environment = "cosmic-desktop"
packages = ["cosmic-desktop-apps"]
display_manager = "gdm"

[flatpak]
[[flatpak.remotes]]
name = "flathub"
url = "https://flathub.org/repo/flathub.flatpakrepo"

[[flatpak.remotes]]
name = "flathub-beta"
url = "https://flathub.org/beta-repo/flathub-beta.flatpakrepo"
```

### System Packages (config/system-packages.toml)
```toml
# System packages to install via dnf
packages = [
    "podman",
    "git",
    "curl", 
    "htop",
    "vim",
    "btop"
]
```

### Flatpak Configuration

#### Main Config (config/config.toml)
```toml
[flatpak]
[[flatpak.remotes]]
name = "flathub"
url = "https://flathub.org/repo/flathub.flatpakrepo"

[[flatpak.remotes]]
name = "flathub-beta"
url = "https://flathub.org/beta-repo/flathub-beta.flatpakrepo"
```

#### Packages (config/flatpak-packages.toml)
```toml
# Flatpak applications to install
packages = [
    "io.gitlab.librewolf-community",          # From flathub (default)
    "flathub-beta:com.valvesoftware.Steam"    # From flathub-beta (specify remote)
]
```

### Dotfiles Management
```toml
[dotfiles]
setup_bashrc = true        # Migrate .bashrc with user confirmation
setup_config_dirs = true   # Migrate .config subdirectories
```

### Desktop Environment Configuration
```toml
[desktop]
# Desktop environment to install and configure
environment = "cosmic-desktop"  # Options: cosmic-desktop, gnome, kde-plasma, xfce, etc.

# Additional desktop packages to install
packages = ["cosmic-desktop-apps"]

# Display manager configuration (login screen)
display_manager = "gdm"  # Options: gdm, lightdm, sddm, cosmic-greeter
```

**Display Manager Options**:
- `gdm` - GNOME Display Manager (recommended for COSMIC)
- `lightdm` - Lightweight display manager
- `sddm` - Simple Desktop Display Manager (KDE's default)
- `cosmic-greeter` - Native COSMIC display manager (in development)

### Container Configuration
```toml
[podman]
containers = [
    # Brave Browser
    { name = "brave", 
      image = "lscr.io/linuxserver/brave:latest", 
      raw_flags = "--security-opt seccomp=unconfined -e PUID=1000 -e PGID=1000 -p 3100:3000 -p 3101:3001 -v $HOME/.config/brave:/config --shm-size=1gb --restart unless-stopped",
      auto_start = true },
    
    # Librewolf Browser  
    { name = "librewolf",
      image = "lscr.io/linuxserver/librewolf:latest",
      raw_flags = "--security-opt seccomp=unconfined -e PUID=1000 -e PGID=1000 -p 3000:3000 -p 3001:3001 -v $HOME/.config/librewolf:/config --shm-size=1gb --restart unless-stopped",
      auto_start = true }
]
```

### Custom Commands
```toml
[custom_commands]
# Regular commands that run every time
commands = [
    "mkdir -p $HOME/.local/bin",
    "git config --global user.name 'Your Name'",
    "systemctl --user enable --now podman.socket"
]

# Commands that only run once (tracked via SHA-256 hash in ~/.config/repro-setup/executed_commands.json)
run_once = [
    "curl -o ~/.local/bin/my-script https://example.com/script.sh && chmod +x ~/.local/bin/my-script",
    "git clone https://github.com/user/dotfiles ~/.dotfiles",
    "pip install --user some-package"
]
```

## 🎯 What Gets Configured

### System Level
- ✅ Package updates and installations from `config/system-packages.toml`
- ✅ Hostname configuration  
- ✅ Additional repositories (RPM Fusion)
- ✅ AMD GPU drivers (optional)
- ✅ Flatpak with Flathub and package installation from `config/flatpak-packages.toml`

### Desktop Environment
- ✅ Desktop environment package installation (COSMIC, GNOME, KDE, etc.)
- ✅ Display manager configuration (GDM, LightDM, SDDM, COSMIC Greeter)
- ✅ Default session configuration
- ✅ Additional desktop packages
- ✅ Flatpak applications

### Containers
- ✅ Podman socket activation
- ✅ Container registry configuration
- ✅ Automated container deployment
- ✅ Volume and network setup

### Dotfiles
- ✅ `.bashrc` migration with backup
- ✅ `.config` directory synchronization
- ✅ User confirmation prompts
- ✅ Preserves existing configurations

## 🔧 Usage Examples

### Basic Desktop Setup
```bash
# Configure distro and basic packages
./repro-setup
```

### Container-Only Setup
```bash
# Disable other features, focus on containers
# Edit config.toml to set auto_start = true for desired containers
./repro-setup
```

### Dotfiles Migration
```bash
# The tool will prompt before overwriting existing configs
# Creates backups like .bashrc.backup, nvim.backup, etc.
./repro-setup
```

## 🌐 Container Access

Once containers are running:

- **Brave Browser**: http://localhost:3100 or https://localhost:3101
- **Librewolf Browser**: http://localhost:3000 or https://localhost:3001  
- **Ollama API**: http://localhost:11434 (if enabled)

## 🛡️ Safety Features

- **Backup Creation**: Automatically backs up existing configurations
- **User Confirmation**: Prompts before overwriting files
- **Distribution Detection**: Warns if config doesn't match detected OS
- **Hash-based Command Tracking**: Run-once commands tracked via SHA-256 hash to prevent duplicate execution
- **Error Handling**: Comprehensive error reporting and rollback

## 🔍 Troubleshooting

### Common Issues

**Permission Errors**
```bash
# Ensure your user has sudo access
sudo usermod -aG wheel $USER
```

**Container Issues**
```bash
# Check Podman status
systemctl --user status podman.socket
podman ps -a

# View logs
podman logs <container-name>
```

**Config Validation**
```bash
# Test config parsing
./repro-setup --config config.toml --help
```

## 📝 Configuration Templates

### Minimal Setup (config/config.toml)
```toml
distro = "fedora"
[system]
hostname = "minimal-setup"
enable_amd_gpu = false
enable_rpm_fusion = false
[dotfiles]
setup_bashrc = true
setup_config_dirs = false
```

**Minimal System Packages (config/system-packages.toml):**
```toml
packages = ["git", "curl"]
```

**Minimal Flatpak Packages (config/flatpak-packages.toml):**
```toml
packages = []
```

### Full Desktop Environment (config/config.toml)
```toml
distro = "fedora"
[system]
hostname = "workstation"
enable_amd_gpu = false
enable_rpm_fusion = true

[desktop]
environment = "cosmic-desktop"
packages = ["cosmic-desktop-apps"]
display_manager = "gdm"  # Recommended for COSMIC

[dotfiles]
setup_bashrc = true
setup_config_dirs = true

[custom_commands]
commands = [
    "git config --global user.name 'Dev User'",
    "mkdir -p $HOME/workspace"
]
run_once = [
    "curl -fsSL https://get.docker.com | sh",
    "pip install --user pipx"
]
```

**Full System Packages (config/system-packages.toml):**
```toml
packages = [
    "git", "htop", "podman", "vim", "curl", "wget",
    "gnome-tweaks", "dconf-editor", "virt-manager"
]
```

**Full Flatpak Packages (config/flatpak-packages.toml):**
```toml
packages = [
    "io.gitlab.librewolf-community",
    "org.mozilla.firefox", 
    "org.libreoffice.LibreOffice"
]
```

## 🤝 Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Test on Fedora
5. Submit a pull request

## 📄 License

This project is licensed under the MIT License - see the LICENSE file for details.

## 🙋‍♂️ Support

- Check the [Issues](../../issues) page for known problems
- Review `CLAUDE.md` for development notes
- Ensure you're running Fedora Linux

---

**Note**: This tool modifies system configurations. Always review the `config.toml` before running and ensure you have backups of important data.
