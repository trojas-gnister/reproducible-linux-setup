# Reproducible Linux Setup

A powerful Rust-based tool for automating Linux desktop environment setup with support for multiple distributions, containerized applications, and dotfiles management.

## 🚀 Features

- **Multi-Distribution Support**: Fedora (dnf) and Debian/Ubuntu (apt)
- **Dotfiles Management**: Safe migration of `.bashrc` and `.config` directories with backups
- **Container Management**: Simplified Podman container deployment with flexible flag support
- **Desktop Environment**: GNOME customization with extensions and themes
- **VPN Integration**: Automated WireGuard setup with NetworkManager
- **Custom Commands**: Execute additional shell commands in sequence
- **Interactive Setup**: User confirmation prompts for safe configuration migration

## 📋 Requirements

- **Supported OS**: Fedora, Debian, or Ubuntu
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
# Run with default configuration
./target/release/repro-setup

# Run with custom config
./target/release/repro-setup --config my-config.toml
```

## 📖 Configuration

The setup is controlled via `config.toml`. Here's the structure:

### Basic Configuration
```toml
# Supported: "fedora", "debian" 
distro = "fedora"

[system]
hostname = "my-desktop"
prefer_dark_theme = true
enable_amd_gpu = false
enable_rpm_fusion = true
system_packages = [
    "git", "curl", "htop", "podman"
]
```

### Dotfiles Management
```toml
[dotfiles]
setup_bashrc = true        # Migrate .bashrc with user confirmation
setup_config_dirs = true   # Migrate .config subdirectories
```

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
commands = [
    "mkdir -p $HOME/.local/bin",
    "git config --global user.name 'Your Name'",
    "systemctl --user enable --now podman.socket"
]
```

## 🎯 What Gets Configured

### System Level
- ✅ Package updates and installations
- ✅ Hostname configuration  
- ✅ Additional repositories (RPM Fusion, contrib/non-free)
- ✅ AMD GPU drivers (optional)
- ✅ Flatpak with Flathub

### Desktop Environment
- ✅ GNOME theme (dark/light)
- ✅ Extensions installation and configuration
- ✅ Custom keybindings and settings
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
- **Error Handling**: Comprehensive error reporting and rollback

## 🔍 Troubleshooting

### Common Issues

**Permission Errors**
```bash
# Ensure your user has sudo access
sudo usermod -aG wheel $USER  # Fedora
sudo usermod -aG sudo $USER   # Debian/Ubuntu
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

### Minimal Setup
```toml
distro = "fedora"
[system]
system_packages = ["git", "curl"]
[dotfiles]
setup_bashrc = true
setup_config_dirs = false
```

### Full Desktop Environment
```toml
distro = "fedora"
[system]
hostname = "workstation"
prefer_dark_theme = true
enable_rpm_fusion = true
system_packages = ["git", "htop", "podman", "gnome-tweaks"]

[gnome]
extensions = [
    { id = 3193, name = "Blur my Shell" }
]

[dotfiles]
setup_bashrc = true
setup_config_dirs = true

[custom_commands]
commands = [
    "git config --global user.name 'Dev User'",
    "mkdir -p $HOME/workspace"
]
```

## 🤝 Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Test on both Fedora and Debian/Ubuntu
5. Submit a pull request

## 📄 License

This project is licensed under the MIT License - see the LICENSE file for details.

## 🙋‍♂️ Support

- Check the [Issues](../../issues) page for known problems
- Review `CLAUDE.md` for development notes
- Ensure your distribution is supported (Fedora/Debian/Ubuntu)

---

**Note**: This tool modifies system configurations. Always review the `config.toml` before running and ensure you have backups of important data.
