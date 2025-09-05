use anyhow::{Context, Result};
use clap::Parser;
use colored::*;
use serde::Deserialize;
use std::fs;
use std::process::{Command, Output};
use std::env;
use std::io::{self, Write};
use std::path::Path;
use dirs;

#[derive(Parser, Debug)]
#[command(version, about = "Reproducible Desktop Setup System")]
struct Args {
    /// Path to the configuration file (TOML format)
    #[arg(long, default_value = "config.toml")]
    config: String,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Distro {
    Fedora,
    Debian,
}

#[derive(Deserialize, Debug)]
struct Config {
    distro: Distro,
    system: SystemConfig,
    gnome: Option<GnomeConfig>,  // Modular for future desktop environments
    podman: Option<PodmanConfig>,
    wireguard: Option<WireguardConfig>,
    dotfiles: Option<DotfilesConfig>,
    custom_commands: Option<CustomCommandsConfig>,
}

#[derive(Deserialize, Debug)]
struct SystemConfig {
    hostname: Option<String>,
    prefer_dark_theme: bool,
    enable_amd_gpu: bool,
    enable_rpm_fusion: bool,
    system_packages: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct GnomeConfig {
    extensions: Vec<Extension>,
    flatpak_applications: Vec<App>,
    additional_settings: Vec<Setting>,
    extensions_to_enable: Vec<String>,
    dnf_extension_packages: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct Extension {
    id: u32,
    name: String,
    uuid: Option<String>,
}

#[derive(Deserialize, Debug)]
struct App {
    id: String,
    name: Option<String>,
}

#[derive(Deserialize, Debug)]
struct Setting {
    schema: String,
    key: String,
    value: String,
}

#[derive(Deserialize, Debug)]
struct PodmanConfig {
    pre_container_setup: Vec<SetupCommand>,
    containers: Vec<Container>,
}

#[derive(Deserialize, Debug)]
struct SetupCommand {
    description: String,
    command: String,
}

#[derive(Deserialize, Debug)]
struct Container {
    name: String,
    image: String,
    raw_flags: Option<String>,
    auto_start: bool,
}


#[derive(Deserialize, Debug)]
struct WireguardConfig {
    conf_path: String,
}

#[derive(Deserialize, Debug)]
struct DotfilesConfig {
    setup_bashrc: bool,
    setup_config_dirs: bool,
}

#[derive(Deserialize, Debug)]
struct CustomCommandsConfig {
    commands: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    println!("🚀 Starting Desktop environment setup...");

    let config_content = fs::read_to_string(&args.config)
        .context(format!("Failed to read config file: {}", args.config))?;
    let config: Config = toml::from_str(&config_content)
        .context("Failed to parse TOML config")?;

    // Check if running on the correct distro
    let os_release = fs::read_to_string("/etc/os-release")?;
    let detected_distro = detect_distro(&os_release)?;
    
    if detected_distro != config.distro {
        println!("{}", format!("Warning: Configuration is for {:?} but detected {:?}. Continuing...", 
                              config.distro, detected_distro).yellow());
    }

    // Update system
    update_system_packages(&config.distro)?;

    // Set hostname
    if let Some(hostname) = &config.system.hostname {
        let current_hostname = run_command_output(&["hostnamectl", "--static"])?;
        let current = String::from_utf8(current_hostname.stdout)?.trim().to_string();
        if current != *hostname {
            run_command(&["sudo", "hostnamectl", "set-hostname", hostname], &format!("Setting hostname to {}", hostname))?;
            println!("{}", "You may need to reboot for hostname changes.".yellow());
        }
    }

    // Install system packages
    if !config.system.system_packages.is_empty() {
        install_system_packages(&config.distro, &config.system.system_packages)?;
    }

    // Enable additional repositories if configured
    if config.system.enable_rpm_fusion {
        enable_additional_repos(&config.distro)?;
    }

    // AMD GPU setup
    if config.system.enable_amd_gpu {
        setup_amd_gpu(&config.distro)?;
    }

    // Flatpak setup
    setup_flatpak(&config.distro)?;

    // Install Flatpak apps (GNOME specific)
    if let Some(gnome) = &config.gnome {
        for app in &gnome.flatpak_applications {
            run_command(&["flatpak", "install", "-y", "flathub", &app.id], &format!("Installing {}", app.name.as_ref().unwrap_or(&app.id)))?;
        }
    }

    // Podman setup
    if let Some(podman) = &config.podman {
        if config.system.system_packages.contains(&"podman".to_string()) {
            run_command(&["systemctl", "--user", "enable", "--now", "podman.socket"], "Enabling Podman socket")?;

            // Configure registries
            let registries_conf = r#"[registries.search]
registries = ['docker.io', 'registry.fedoraproject.org', 'quay.io', 'registry.redhat.io', 'ghcr.io']"#;
            let config_dir = dirs::home_dir().unwrap().join(".config/containers");
            fs::create_dir_all(&config_dir)?;
            fs::write(config_dir.join("registries.conf"), registries_conf)?;

            // Pre-container setup
            for setup in &podman.pre_container_setup {
                let cmd_parts: Vec<&str> = setup.command.split_whitespace().collect();
                run_command(&cmd_parts, &setup.description)?;
            }

            // Pull and start containers
            for cont in &podman.containers {
                run_command(&["podman", "pull", &cont.image], &format!("Pulling container {}", cont.name))?;

                if cont.auto_start {
                    // Build command: podman run -d --name={name} {raw_flags} {image}
                    let mut command = format!("podman run -d --name={}", cont.name);
                    
                    if let Some(flags) = &cont.raw_flags {
                        command.push(' ');
                        command.push_str(flags);
                    }
                    
                    command.push(' ');
                    command.push_str(&cont.image);

                    // Execute via shell to handle complex flag parsing
                    let output = Command::new("sh")
                        .arg("-c")
                        .arg(&command)
                        .output()
                        .with_context(|| format!("Failed to start container: {}", cont.name))?;

                    if !output.status.success() {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        println!("{} Failed to start container {}: {}", "[ERROR]".red(), cont.name, stderr);
                        anyhow::bail!("Container startup failed: {}", cont.name);
                    }

                    println!("{} Successfully started container {}", "[SUCCESS]".green(), cont.name);
                }
            }
        }
    }

    // WireGuard setup
    if let Some(wg) = &config.wireguard {
        run_command(&["sudo", "dnf", "install", "-y", "--skip-unavailable", "wireguard-tools"], "Installing WireGuard tools")?;

        // Derive interface name from conf_path (e.g., "wg0.conf" -> "wg0")
        use std::path::Path;
        let conf_path = Path::new(&wg.conf_path);
        let interface_name = conf_path.file_stem()
            .context("Invalid conf_path: missing file stem")?
            .to_str()
            .context("Invalid conf_path: non-UTF8 stem")?
            .to_string();

        // Install WireGuard and NetworkManager plugin
        install_wireguard_packages(&config.distro)?;

        // Ensure system-connections dir exists
        run_command(&["sudo", "mkdir", "-p", "/etc/NetworkManager/system-connections"], "Creating NetworkManager system-connections directory")?;

        // Remove existing connection if present (idempotent)
        let _ = run_command(&["nmcli", "connection", "delete", &interface_name], "Removing existing WireGuard connection if present");

        // Import config into NetworkManager
        run_command(&["nmcli", "connection", "import", "type", "wireguard", "file", &wg.conf_path], &format!("Importing WireGuard config for {}", interface_name))?;

        // Copy config to system-connections for persistence
        let nm_connection_file = format!("/etc/NetworkManager/system-connections/wg-{}.nmconnection", interface_name);
        run_command(&["sudo", "nmcli", "connection", "export", &interface_name], &format!("Exporting connection to {}", nm_connection_file))?;
        run_command(&["sudo", "chmod", "600", &nm_connection_file], "Setting secure permissions on NetworkManager connection file")?;

        // Explicitly set autoconnect and save
        run_command(&["nmcli", "connection", "modify", &interface_name, "connection.autoconnect", "yes"], &format!("Enabling autoconnect for {}", interface_name))?;
        run_command(&["nmcli", "connection", "modify", &interface_name, "connection.autoconnect-priority", "10"], &format!("Setting autoconnect priority for {}", interface_name))?;

        // Reload NetworkManager to ensure connection is loaded
        run_command(&["sudo", "systemctl", "reload", "NetworkManager"], "Reloading NetworkManager to apply connection")?;

        // Verify connection exists
        let output = run_command_output(&["nmcli", "connection", "show", &interface_name])?;
        if !output.status.success() {
            println!("{}", format!("Failed to verify {} connection in NetworkManager", interface_name).red());
            anyhow::bail!("Connection not found after import");
        }

        // Activate the connection
        run_command(&["nmcli", "connection", "up", &interface_name], &format!("Activating WireGuard connection {}", interface_name))?;
    }

    // Dotfiles setup
    if let Some(dotfiles) = &config.dotfiles {
        setup_dotfiles(dotfiles)?;
    }

    // GNOME-specific setup
    if let Some(gnome) = &config.gnome {
        // Theme
        if config.system.prefer_dark_theme {
            run_command(&["gsettings", "set", "org.gnome.desktop.interface", "color-scheme", "'prefer-dark'"], "Applying dark theme")?;
            run_command(&["gsettings", "set", "org.gnome.desktop.interface", "gtk-theme", "'Adwaita-dark'"], "Setting GTK theme")?;
        } else {
            run_command(&["gsettings", "set", "org.gnome.desktop.interface", "color-scheme", "'prefer-light'"], "Applying light theme")?;
            run_command(&["gsettings", "set", "org.gnome.desktop.interface", "gtk-theme", "'Adwaita'"], "Setting GTK theme")?;
        }

        // Additional settings
        for setting in &gnome.additional_settings {
            run_command(&["gsettings", "set", &setting.schema, &setting.key, &setting.value], &format!("Setting {} {}", setting.schema, setting.key))?;
        }

        // Install gnome-extensions-cli
        install_gnome_extensions_cli(&config.distro)?;

        // Add to PATH if needed
        let home = dirs::home_dir().unwrap();
        let bin_path = home.join(".local/bin").to_string_lossy().to_string();
        if !env::var("PATH")?.contains(&bin_path) {
            // Note: This doesn't persist; user needs to add to .bashrc
            println!("{}", format!("Add {} to PATH in your shell config.", bin_path).yellow());
        }

        // Install extensions
        for ext in &gnome.extensions {
            run_command(&["gext", "install", &ext.id.to_string()], &format!("Installing extension {}", ext.name))?;
            if let Some(uuid) = &ext.uuid {
                run_command(&["gext", "enable", uuid], &format!("Enabling extension {}", ext.name))?;
            }
        }

        // Install fallback extension packages
        install_extension_packages(&config.distro, &gnome.dnf_extension_packages)?;

        // Enable extensions fallback
        for uuid in &gnome.extensions_to_enable {
            run_command(&["gnome-extensions", "enable", uuid], &format!("Enabling extension {}", uuid))?;
        }
    }

    // Execute custom commands
    if let Some(custom_commands) = &config.custom_commands {
        execute_custom_commands(custom_commands)?;
    }

    // Summary (similar to bash)
    println!("📋 Setup Summary:");
    println!("✅ System updated");
    if let Some(hostname) = config.system.hostname {
        println!("✅ Hostname set to: {}", hostname);
    }
    if config.wireguard.is_some() {
        println!("✅ WireGuard VPN configured with autoconnect");
    }
    // Add more summary items as needed...

    println!("💡 Manual steps: Log out/in or reboot for full effect.");
    println!("{}", "Setup completed successfully!".green());

    Ok(())
}

fn run_command(cmd: &[&str], desc: &str) -> Result<()> {
    println!("{} {}", "[INFO]".blue(), desc);
    let output = Command::new(cmd[0]).args(&cmd[1..]).output()?;
    if !output.status.success() {
        println!("{} {}: {:?}", "[ERROR]".red(), desc, String::from_utf8_lossy(&output.stderr));
        anyhow::bail!("Command failed");
    }
    println!("{} {}", "[SUCCESS]".green(), desc);
    Ok(())
}

fn run_command_output(cmd: &[&str]) -> Result<Output> {
    Command::new(cmd[0]).args(&cmd[1..]).output().context("Command failed")
}

fn setup_dotfiles(config: &DotfilesConfig) -> Result<()> {
    println!("{} Setting up dotfiles...", "[INFO]".blue());
    
    let current_dir = env::current_dir()?;
    let home_dir = dirs::home_dir().context("Could not find home directory")?;
    
    // Setup .bashrc
    if config.setup_bashrc {
        setup_bashrc(&current_dir, &home_dir)?;
    }
    
    // Setup .config directories
    if config.setup_config_dirs {
        setup_config_dirs(&current_dir, &home_dir)?;
    }
    
    println!("{} Dotfiles setup completed!", "[SUCCESS]".green());
    Ok(())
}

fn setup_bashrc(project_dir: &Path, home_dir: &Path) -> Result<()> {
    let project_bashrc = project_dir.join(".bashrc");
    let home_bashrc = home_dir.join(".bashrc");
    
    if !project_bashrc.exists() {
        println!("{} No .bashrc found in project directory, skipping", "[WARN]".yellow());
        return Ok(());
    }
    
    if home_bashrc.exists() {
        println!("{} Found existing .bashrc in home directory", "[INFO]".blue());
        if ask_user_confirmation("Do you want to replace your existing .bashrc with the one from this project?")? {
            // Backup existing .bashrc
            let backup_path = home_dir.join(".bashrc.backup");
            fs::copy(&home_bashrc, &backup_path)
                .context("Failed to backup existing .bashrc")?;
            println!("{} Backed up existing .bashrc to .bashrc.backup", "[INFO]".blue());
            
            // Copy project .bashrc
            fs::copy(&project_bashrc, &home_bashrc)
                .context("Failed to copy project .bashrc")?;
            println!("{} Successfully replaced .bashrc", "[SUCCESS]".green());
        } else {
            println!("{} Skipping .bashrc setup", "[INFO]".blue());
        }
    } else {
        println!("{} No existing .bashrc found, copying from project", "[INFO]".blue());
        fs::copy(&project_bashrc, &home_bashrc)
            .context("Failed to copy .bashrc from project")?;
        println!("{} Successfully installed .bashrc", "[SUCCESS]".green());
    }
    
    Ok(())
}

fn setup_config_dirs(project_dir: &Path, home_dir: &Path) -> Result<()> {
    let project_config = project_dir.join(".config");
    let home_config = home_dir.join(".config");
    
    if !project_config.exists() {
        println!("{} No .config directory found in project, skipping", "[WARN]".yellow());
        return Ok(());
    }
    
    // Create ~/.config if it doesn't exist
    fs::create_dir_all(&home_config)
        .context("Failed to create ~/.config directory")?;
    
    // Process each subdirectory in project .config
    for entry in fs::read_dir(&project_config)? {
        let entry = entry?;
        let path = entry.path();
        
        if path.is_dir() {
            let dir_name = path.file_name()
                .context("Invalid directory name")?
                .to_str()
                .context("Non-UTF8 directory name")?;
            
            let target_dir = home_config.join(dir_name);
            
            println!("{} Processing config directory: {}", "[INFO]".blue(), dir_name);
            
            if target_dir.exists() {
                println!("{} Found existing {} config directory", "[INFO]".blue(), dir_name);
                if ask_user_confirmation(&format!("Do you want to replace your existing {} config with the one from this project?", dir_name))? {
                    // Backup existing config
                    let backup_path = home_config.join(format!("{}.backup", dir_name));
                    if backup_path.exists() {
                        fs::remove_dir_all(&backup_path)?;
                    }
                    fs::rename(&target_dir, &backup_path)
                        .with_context(|| format!("Failed to backup existing {} config", dir_name))?;
                    println!("{} Backed up existing {} config to {}.backup", "[INFO]".blue(), dir_name, dir_name);
                    
                    // Copy project config
                    copy_dir_all(&path, &target_dir)
                        .with_context(|| format!("Failed to copy {} config from project", dir_name))?;
                    println!("{} Successfully replaced {} config", "[SUCCESS]".green(), dir_name);
                } else {
                    if ask_user_confirmation(&format!("Do you want to skip the {} config setup?", dir_name))? {
                        println!("{} Skipping {} config setup", "[INFO]".blue(), dir_name);
                    } else {
                        println!("{} Please choose whether to replace or skip", "[INFO]".blue());
                    }
                }
            } else {
                println!("{} No existing {} config found, copying from project", "[INFO]".blue(), dir_name);
                copy_dir_all(&path, &target_dir)
                    .with_context(|| format!("Failed to copy {} config from project", dir_name))?;
                println!("{} Successfully installed {} config", "[SUCCESS]".green(), dir_name);
            }
        }
    }
    
    Ok(())
}

fn ask_user_confirmation(prompt: &str) -> Result<bool> {
    loop {
        print!("{} (y/n): ", prompt);
        io::stdout().flush()?;
        
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        
        match input.trim().to_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("Please answer yes (y) or no (n)."),
        }
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn detect_distro(os_release: &str) -> Result<Distro> {
    if os_release.contains("Fedora") {
        Ok(Distro::Fedora)
    } else if os_release.contains("Debian") || os_release.contains("Ubuntu") {
        Ok(Distro::Debian)
    } else {
        anyhow::bail!("Unsupported distribution. Only Fedora and Debian/Ubuntu are supported.");
    }
}

fn update_system_packages(distro: &Distro) -> Result<()> {
    match distro {
        Distro::Fedora => {
            run_command(&["sudo", "dnf", "update", "-y"], "Updating system packages")?;
        }
        Distro::Debian => {
            run_command(&["sudo", "apt", "update"], "Updating package lists")?;
            run_command(&["sudo", "apt", "upgrade", "-y"], "Upgrading system packages")?;
        }
    }
    Ok(())
}

fn install_system_packages(distro: &Distro, packages: &[String]) -> Result<()> {
    match distro {
        Distro::Fedora => {
            let mut cmd: Vec<&str> = vec!["sudo", "dnf", "install", "-y", "--skip-unavailable"];
            for pkg in packages {
                cmd.push(pkg);
            }
            run_command(&cmd, "Installing system packages")?;
        }
        Distro::Debian => {
            let mut cmd: Vec<&str> = vec!["sudo", "apt", "install", "-y"];
            for pkg in packages {
                cmd.push(pkg);
            }
            run_command(&cmd, "Installing system packages")?;
        }
    }
    Ok(())
}

fn enable_additional_repos(distro: &Distro) -> Result<()> {
    match distro {
        Distro::Fedora => {
            run_command(&["sudo", "dnf", "install", "-y", "https://download1.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm", "https://download1.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-$(rpm -E %fedora).noarch.rpm"], "Enabling RPM Fusion")?;
        }
        Distro::Debian => {
            // Enable additional repositories for Debian (e.g., non-free)
            run_command(&["sudo", "apt-add-repository", "-y", "contrib"], "Enabling contrib repository")?;
            run_command(&["sudo", "apt-add-repository", "-y", "non-free"], "Enabling non-free repository")?;
            run_command(&["sudo", "apt", "update"], "Updating package lists after adding repositories")?;
        }
    }
    Ok(())
}

fn setup_amd_gpu(distro: &Distro) -> Result<()> {
    match distro {
        Distro::Fedora => {
            run_command(&["sudo", "dnf", "install", "-y", "--skip-unavailable", "rocm-opencl", "rocm-clinfo", "mesa-dri-drivers"], "Installing ROCm and AMD drivers")?;
        }
        Distro::Debian => {
            run_command(&["sudo", "apt", "install", "-y", "mesa-vulkan-drivers", "libvulkan1", "mesa-opencl-icd"], "Installing AMD GPU drivers")?;
        }
    }
    
    // Common GPU setup (distro-agnostic)
    run_command(&["sudo", "usermod", "-aG", "render", &env::var("USER")?], "Adding user to render group")?;
    let udev_content = r#"KERNEL=="kfd", GROUP="render", MODE="0666"
SUBSYSTEM=="drm", GROUP="render", MODE="0666""#;
    let tee_cmd = format!("echo '{}' | sudo tee /etc/udev/rules.d/70-kfd.rules > /dev/null", udev_content);
    run_command(&["sh", "-c", &tee_cmd], "Configuring GPU device permissions")?;
    run_command(&["sudo", "udevadm", "control", "--reload-rules"], "Reloading udev rules")?;
    run_command(&["sudo", "udevadm", "trigger"], "Triggering udev")?;
    println!("{}", "Reboot recommended for AMD GPU.".yellow());
    
    Ok(())
}

fn setup_flatpak(distro: &Distro) -> Result<()> {
    match distro {
        Distro::Fedora => {
            run_command(&["sudo", "dnf", "install", "-y", "--skip-unavailable", "flatpak"], "Installing Flatpak")?;
        }
        Distro::Debian => {
            run_command(&["sudo", "apt", "install", "-y", "flatpak"], "Installing Flatpak")?;
        }
    }
    run_command(&["flatpak", "remote-add", "--if-not-exists", "flathub", "https://flathub.org/repo/flathub.flatpakrepo"], "Adding Flathub")?;
    Ok(())
}

fn install_wireguard_packages(distro: &Distro) -> Result<()> {
    match distro {
        Distro::Fedora => {
            // Enable Copr repo for NM WireGuard plugin (idempotent)
            run_command(&["sudo", "dnf", "copr", "enable", "-y", "timn/NetworkManager-wireguard"], "Enabling Copr repo for NetworkManager WireGuard plugin")?;
            // Install NM WireGuard plugin
            run_command(&["sudo", "dnf", "install", "-y", "NetworkManager-wireguard-gtk"], "Installing NetworkManager WireGuard plugin")?;
        }
        Distro::Debian => {
            run_command(&["sudo", "apt", "install", "-y", "wireguard", "wireguard-tools", "network-manager"], "Installing WireGuard and NetworkManager")?;
        }
    }
    Ok(())
}

fn install_gnome_extensions_cli(distro: &Distro) -> Result<()> {
    if run_command_output(&["command", "-v", "pipx"]).is_ok() {
        run_command(&["pipx", "install", "gnome-extensions-cli", "--system-site-packages"], "Installing gnome-extensions-cli with pipx")?;
    } else if run_command_output(&["command", "-v", "pip3"]).is_ok() {
        run_command(&["pip3", "install", "--user", "gnome-extensions-cli"], "Installing gnome-extensions-cli with pip3")?;
    } else {
        match distro {
            Distro::Fedora => {
                run_command(&["sudo", "dnf", "install", "-y", "--skip-unavailable", "python3-pip"], "Installing python3-pip")?;
            }
            Distro::Debian => {
                run_command(&["sudo", "apt", "install", "-y", "python3-pip"], "Installing python3-pip")?;
            }
        }
        run_command(&["pip3", "install", "--user", "gnome-extensions-cli"], "Installing gnome-extensions-cli")?;
    }
    Ok(())
}

fn install_extension_packages(distro: &Distro, packages: &[String]) -> Result<()> {
    for pkg in packages {
        match distro {
            Distro::Fedora => {
                run_command(&["sudo", "dnf", "install", "-y", "--skip-unavailable", pkg], &format!("Installing fallback package {}", pkg))?;
            }
            Distro::Debian => {
                run_command(&["sudo", "apt", "install", "-y", pkg], &format!("Installing fallback package {}", pkg))?;
            }
        }
    }
    Ok(())
}

fn execute_custom_commands(config: &CustomCommandsConfig) -> Result<()> {
    if config.commands.is_empty() {
        return Ok(());
    }
    
    println!("{} Executing custom commands...", "[INFO]".blue());
    
    for (index, command) in config.commands.iter().enumerate() {
        println!("{} Executing command {} of {}: {}", 
                "[INFO]".blue(), index + 1, config.commands.len(), command);
        
        // Execute command through shell to support environment variables and shell features
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .with_context(|| format!("Failed to execute command: {}", command))?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("{} Command failed: {}", "[ERROR]".red(), command);
            println!("{} Error output: {}", "[ERROR]".red(), stderr);
            anyhow::bail!("Custom command failed: {}", command);
        }
        
        // Show stdout if there's any output
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.trim().is_empty() {
            println!("{} Output: {}", "[INFO]".blue(), stdout.trim());
        }
        
        println!("{} Command completed successfully", "[SUCCESS]".green());
    }
    
    println!("{} All custom commands executed successfully!", "[SUCCESS]".green());
    Ok(())
}
