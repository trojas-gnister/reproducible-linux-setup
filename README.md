# FedoraForge

**Forge your perfect Fedora system with declarative configuration**

FedoraForge is a powerful declarative configuration system for Fedora Linux that enables reproducible, version-controlled system management through simple TOML configuration files. Define your entire system state as code - packages, containers, desktop environment, VPN, and custom commands - then apply changes idempotently.

## 🚀 Declarative Features

- **Pure Configuration**: Define your entire system state in version-controlled TOML files
- **Idempotent Operations**: Run multiple times safely - only changes what needs changing
- **Atomic Updates**: Complete system configuration or rollback on failure
- **Reproducible Builds**: Generate identical systems from the same configuration
- **Modular Design**: Separate concerns with dedicated files for packages, containers, and system settings
- **State Tracking**: Intelligent synchronization between declared and actual system state
- **Drift Detection**: Identify and correct configuration drift from your declared state
- **Backup Integration**: Automatic backups before any destructive operations

## 📋 Requirements

- **Supported OS**: Fedora Linux
- **Dependencies**: `sudo` access for system modifications
- **Optional**: Podman for container support

### Essential Dependencies

Before building FedoraForge, ensure you have these packages installed:

```bash
# Install Rust toolchain and Git
sudo dnf install -y rust cargo git

# Optional: Install development tools group for a complete build environment
sudo dnf groupinstall -y "Development Tools"
```

**Alternative Rust Installation:**
```bash
# Install Rust via rustup (recommended for latest version)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

## 🛠️ Installation

### From Source
```bash
git clone <repository-url>
cd fedoraforge
cargo build --release
```

### Declarative Workflow
```bash
# 1. Capture current system state as declarative configuration
./target/release/fedoraforge --initial

# 2. Edit configuration files to declare desired state
vim config/config.toml

# 3. Apply configuration (converge system to declared state)
./target/release/fedoraforge

# 4. Verify system matches declared configuration
./target/release/fedoraforge --verbose

# Advanced usage
./target/release/fedoraforge --yes          # Unattended deployment
./target/release/fedoraforge --config prod.toml  # Environment-specific configs
```

## 🎛️ CLI Options

| Flag | Description |
|------|-------------|
| `--initial` | Generate initial configuration files from current system state |
| `--config <path>` | Use custom configuration file (default: `config/config.toml`) |
| `--verbose, -v` | Enable verbose logging for detailed output |
| `--yes, -y` | Automatically answer yes to all prompts (unattended mode) |
| `--no, -n` | Automatically answer no to all prompts (safe mode) |
| `--force-recreate` | Force recreation of all containers |
| `--update-images` | Update container images and recreate if changed |
| `--no-recreate` | Never recreate containers (config/systemd only) |
| `--help, -h` | Show help information |
| `--version` | Show version information |

**Note**: `--yes` and `--no` flags cannot be used together.

## 📖 Declarative Configuration

Your entire system is defined through configuration files in the `config/` directory:

- `config/config.toml` - System state declaration (hostname, desktop, containers, VPN, drives)
- `config/system-packages.toml` - Declared package state (dnf packages)
- `config/flatpak-packages.toml` - Declared application state (Flatpak apps)
- `config/system-services.toml` - Declared system services state (systemd services as root)
- `config/user-services.toml` - Declared user services state (systemd user services)

### Bootstrapping Your Configuration

Capture your current system state as a declarative configuration:

```bash
./target/release/fedoraforge --initial
```

This introspects your system and generates:
- **Package declarations** from `dnf repoquery --leaves --userinstalled`
- **Application declarations** from `flatpak list --app`
- **Service declarations** from `systemctl list-unit-files` (system and user)
- **Base configuration structure** in `config/`

Once generated, your system state becomes code - edit the TOML files to declare your desired state, then apply changes with a simple `./fedoraforge` command.

Here's the structure:

### System State Declaration (config/config.toml)
```toml
# Declare target distribution
distro = "fedora"

# Declare system configuration
[system]
hostname = "my-workstation"      # Desired hostname
enable_amd_gpu = false           # GPU driver state
enable_rpm_fusion = true        # Repository state

# Declare desktop environment state
[desktop]
environment = "cosmic-desktop"   # Desired desktop environment
packages = ["cosmic-desktop-apps"]  # Required desktop packages
display_manager = "gdm"          # Login manager

# Declare application repositories
[flatpak]
[[flatpak.remotes]]
name = "flathub"
url = "https://flathub.org/repo/flathub.flatpakrepo"
```

### Package State Declaration (config/system-packages.toml)
```toml
# Declare desired system packages (managed via dnf)
packages = [
    "podman",     # Container runtime
    "git",        # Version control
    "curl",       # HTTP client
    "htop",       # Process monitor
    "vim",        # Text editor
    "neovim"      # Modern vim
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

#### Application State Declaration (config/flatpak-packages.toml)
```toml
# Declare desired Flatpak applications
packages = [
    "io.gitlab.librewolf-community",          # Privacy-focused browser
    "flathub-beta:com.valvesoftware.Steam"    # Gaming platform (beta)
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

### Container State Declaration
```toml
[podman]
# Commands to run before container setup
pre_container_setup = [
    { description = "Create config directory", command = "mkdir -p $HOME/.config/librewolf" }
]

# Declare desired container state
[[podman.containers]]
name = "librewolf"
image = "lscr.io/linuxserver/librewolf:latest"
raw_flags = "--security-opt seccomp=unconfined -e PUID=1000 -p 3000:3000 -v $HOME/.config/librewolf:/config --restart unless-stopped"
autostart = true      # Declare autostart behavior via systemd
start_after_creation = false
```

### Drive Configuration
```toml
[[drives]]
device = "/dev/sdb1"
mount_point = "/mnt/data"
encrypted = false
filesystem = "ext4"
label = "data-drive"
force_update = false
```

### Services Configuration

#### System Services (config/system-services.toml)
```toml
# System services (run as root)
[services]
sshd = { enabled = true, started = true }
NetworkManager = { enabled = true, started = true }
firewalld = { enabled = true, started = true }
cups = { enabled = false, started = false }

# Custom system services (defined declaratively)
[[custom_services]]
name = "backup-service"
enabled = true
started = false  # Only run when triggered by timer
service_definition = """
[Unit]
Description=System Backup Service
After=network.target

[Service]
Type=oneshot
ExecStart=/usr/local/bin/backup.sh
User=backup

[Install]
WantedBy=multi-user.target
"""
# Optional timer for scheduled execution
timer_definition = """
[Unit]
Description=Daily Backup Timer
Requires=backup-service.service

[Timer]
OnCalendar=daily
Persistent=true

[Install]
WantedBy=timers.target
"""
```

#### User Services (config/user-services.toml)
```toml
# User services (run as current user)
[services]
"podman.socket" = { enabled = true, started = true }
"pipewire.service" = { enabled = true, started = true }

# Application autostart (automatically creates systemd user services)
[applications]
cosmic-term = { enabled = true, restart_policy = "never", delay = 2 }
firefox = { enabled = true, restart_policy = "never", delay = 5 }
discord = { enabled = false, restart_policy = "on-failure" }

# Custom user services
[[custom_services]]
name = "dev-server"
enabled = true
started = true
service_definition = """
[Unit]
Description=Development Server
After=graphical-session.target

[Service]
Type=simple
ExecStart=%h/bin/dev-server
Restart=always
Environment=NODE_ENV=development

[Install]
WantedBy=default.target
"""
```

### VPN Configuration (WireGuard or OpenVPN)
```toml
[vpn]
# VPN type: "wireguard" or "openvpn"
type = "wireguard"
# Path to the VPN configuration file
conf_path = "/home/user/vpn/wg0.conf"
```

**Supported VPN Types**:
- `wireguard` - WireGuard VPN configuration (.conf files)
- `openvpn` - OpenVPN configuration (.ovpn files)

The tool will automatically:
- Install the necessary VPN tools and NetworkManager plugins
- Import the configuration into NetworkManager
- Enable autoconnect for the VPN connection
- Handle Fedora version compatibility issues gracefully

### Command State Declaration
```toml
[custom_commands]
# Commands that enforce state every run
commands = [
    "mkdir -p $HOME/.local/bin",                    # Ensure directory exists
    "git config --global user.name 'Your Name'",   # Ensure git config
    "systemctl --user enable --now podman.socket"  # Ensure service state
]

# One-time initialization commands (idempotent via hash tracking)
run_once = [
    "curl -o ~/.local/bin/my-script https://example.com/script.sh && chmod +x ~/.local/bin/my-script",
    "git clone https://github.com/user/dotfiles ~/.dotfiles"
]
```

## 🎯 What Gets Configured

### System Level
- ✅ Package updates and installations from `config/system-packages.toml`
- ✅ Hostname configuration
- ✅ Additional repositories (RPM Fusion)
- ✅ AMD GPU drivers (optional)
- ✅ Flatpak with Flathub and package installation from `config/flatpak-packages.toml`
- ✅ System and user services management from `config/system-services.toml` and `config/user-services.toml`
- ✅ Custom service definition and deployment (systemd services defined declaratively)
- ✅ Application autostart management (automatically creates systemd user services for applications)

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
./fedoraforge
```

### Container-Only Setup
```bash
# Disable other features, focus on containers
# Edit config.toml to set start_after_creation = true for immediate start
# Set autostart = true for boot-time autostart via Quadlet
./fedoraforge
```

### Dotfiles Migration
```bash
# The tool will prompt before overwriting existing configs
# Creates backups like .bashrc.backup, nvim.backup, etc.
./fedoraforge
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
./fedoraforge --config config.toml --help
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

**Full System Services (config/system-services.toml):**
```toml
[services]
sshd = { enabled = true, started = true }
NetworkManager = { enabled = true, started = true }
firewalld = { enabled = true, started = true }
```

**Full User Services (config/user-services.toml):**
```toml
[services]
"podman.socket" = { enabled = true, started = true }
"pipewire.service" = { enabled = true, started = true }

[applications]
cosmic-term = { enabled = true, restart_policy = "never", delay = 2 }
firefox = { enabled = true, restart_policy = "never", delay = 5 }
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

**Note**: FedoraForge modifies system configurations. Always review the `config.toml` before running and ensure you have backups of important data.

---

**FedoraForge - Forge your perfect Fedora system** 🔥

