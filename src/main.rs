use anyhow::{Context, Result};
use clap::Parser;
use colored::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::process::{Command, Output};
use std::env;
use std::io::{self, Write, BufRead};
use std::path::Path;
use std::collections::HashMap;
use sha2::{Sha256, Digest};
use dirs;

#[derive(Parser, Debug)]
#[command(version, about = "Reproducible Desktop Setup System")]
struct Args {
    /// Path to the configuration file (TOML format)
    #[arg(long, default_value = "config/config.toml")]
    config: String,
    
    /// Generate initial configuration files from current system state
    #[arg(long)]
    initial: bool,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Distro {
    Fedora,
}

#[derive(Deserialize, Debug)]
struct Config {
    distro: Distro,
    system: SystemConfig,
    desktop: Option<DesktopConfig>,
    podman: Option<PodmanConfig>,
    wireguard: Option<WireguardConfig>,
    dotfiles: Option<DotfilesConfig>,
    custom_commands: Option<CustomCommandsConfig>,
}

#[derive(Deserialize, Debug)]
struct SystemConfig {
    hostname: Option<String>,
    enable_amd_gpu: bool,
    enable_rpm_fusion: bool,
}

#[derive(Deserialize, Debug)]
struct DesktopConfig {
    environment: Option<String>,
    packages: Option<Vec<String>>,
    display_manager: Option<String>,
}

#[derive(Deserialize, Debug)]
struct PodmanConfig {
    pre_container_setup: Option<Vec<SetupCommand>>,
    containers: Option<Vec<Container>>,
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
    run_once: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct ExecutedCommandsState {
    executed_once_commands: HashMap<String, CommandExecutionRecord>,
}

#[derive(Serialize, Deserialize, Debug)]
struct CommandExecutionRecord {
    command_hash: String,
    original_command: String,
    executed_at: u64, // Unix timestamp
}

#[derive(Serialize, Deserialize, Debug)]
struct PackageList {
    packages: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    // Handle --initial flag to generate package config files from current system state
    if args.initial {
        println!("{} Generating package configuration from current system state...", "[INFO]".blue());
        
        // Create config directory if it doesn't exist
        fs::create_dir_all("config")
            .with_context(|| "Failed to create config directory")?;
        
        // Generate system packages config
        let system_packages = get_user_installed_packages()?;
        update_system_packages_file(&system_packages)?;
        
        // Generate flatpak packages config  
        let flatpak_packages = get_installed_flatpaks().unwrap_or_else(|_| {
            println!("{} Flatpak not available or no applications installed", "[WARN]".yellow());
            Vec::new()
        });
        update_flatpak_packages_file(&flatpak_packages)?;
        
        println!("{} Package configuration files generated successfully!", "[SUCCESS]".green());
        println!("Now create your main config/config.toml file and run again without --initial");
        return Ok(());
    }
    
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

    // Synchronize system packages with installed packages
    let system_packages = sync_system_packages()?;

    // Desktop Environment Setup
    if let Some(desktop_config) = &config.desktop {
        setup_desktop_environment(&config.distro, desktop_config)?;
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
    
    // Synchronize Flatpak packages with installed applications
    let _flatpak_packages = sync_flatpak_packages()?;

    // Podman setup
    if let Some(podman) = &config.podman {
        if system_packages.contains(&"podman".to_string()) {
            run_command(&["systemctl", "--user", "enable", "--now", "podman.socket"], "Enabling Podman socket")?;

            // Configure registries
            let registries_conf = r#"[registries.search]
registries = ['docker.io', 'registry.fedoraproject.org', 'quay.io', 'registry.redhat.io', 'ghcr.io']"#;
            let config_dir = dirs::home_dir().unwrap().join(".config/containers");
            fs::create_dir_all(&config_dir)?;
            fs::write(config_dir.join("registries.conf"), registries_conf)?;

            let home_dir = dirs::home_dir().context("Could not find home directory")?;
            let home_path = home_dir.to_str().context("Invalid home directory path")?;

            if let Some(setups) = &podman.pre_container_setup {
                for setup in setups {
                    let command = setup.command.replace("$HOME", home_path);
                    let cmd_parts: Vec<&str> = command.split_whitespace().collect();
                    run_command(&cmd_parts, &setup.description)?;
                }
            }

            // Reconciliation of managed containers
            let managed_containers_output = Command::new("podman").args(&["ps", "-a", "--filter", "label=managed-by=repro-setup", "--format", "{{.Names}}"]).output()?;
            let managed_containers = std::io::Cursor::new(managed_containers_output.stdout).lines().collect::<Result<Vec<_>, _>>()?;
            let configured_containers: Vec<String> = podman.containers.as_ref().unwrap_or(&Vec::new()).iter().map(|c| c.name.clone()).collect();

            for container_name in managed_containers {
                if !configured_containers.contains(&container_name) {
                    if ask_user_confirmation(&format!("Container '{}' is managed by this tool but not in the config. Remove it?", container_name))? {
                        run_command(&["podman", "rm", "-f", &container_name], &format!("Removing orphaned container {}", container_name))?;
                    }
                }
            }

            // Pull and start containers
            if let Some(containers) = &podman.containers {
                for cont in containers {
                let container_exists = std::io::Cursor::new(Command::new("podman").args(&["ps", "-a", "--format", "{{.Names}}"]).output()?.stdout).lines().any(|line| line.unwrap_or_default() == cont.name);

                if container_exists {
                    if ask_user_confirmation(&format!("Container '{}' already exists. Do you want to replace it?", cont.name))? {
                        run_command(&["podman", "rm", "-f", &cont.name], &format!("Removing existing container {}", cont.name))?;
                    } else {
                        println!("{} Skipping container {}", "[INFO]".blue(), cont.name);
                        continue;
                    }
                }

                run_command(&["podman", "pull", &cont.image], &format!("Pulling container {}", cont.name))?;

                if cont.auto_start {
                    let mut command = format!("podman run -d --name={} --label managed-by=repro-setup", cont.name);
                    
                    if let Some(flags) = &cont.raw_flags {
                        let replaced_flags = flags.replace("$HOME", home_path);
                        command.push(' ');
                        command.push_str(&replaced_flags);
                    }
                    
                    command.push(' ');
                    command.push_str(&cont.image);

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

    io::stdout().write_all(&output.stdout)?;
    io::stderr().write_all(&output.stderr)?;

    if !output.status.success() {
        println!("{} {}: Command failed", "[ERROR]".red(), desc);
        anyhow::bail!("Command failed");
    }
    println!("{} {}", "[SUCCESS]".green(), desc);
    Ok(())
}

fn run_command_output(cmd: &[&str]) -> Result<Output> {
    let output = Command::new(cmd[0]).args(&cmd[1..]).output().context("Command failed")?;
    io::stdout().write_all(&output.stdout)?;
    io::stderr().write_all(&output.stderr)?;
    Ok(output)
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
                    println!("{} Skipping {} config setup", "[INFO]".blue(), dir_name);
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

fn generate_command_hash(command: &str) -> String {
    let normalized_command = command.trim();
    let mut hasher = Sha256::new();
    hasher.update(normalized_command.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn get_current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn get_state_file_path() -> Result<std::path::PathBuf> {
    let home_dir = dirs::home_dir().context("Could not find home directory")?;
    let config_dir = home_dir.join(".config").join("repro-setup");
    fs::create_dir_all(&config_dir).context("Failed to create config directory")?;
    Ok(config_dir.join("executed_commands.json"))
}

fn load_executed_commands_state() -> Result<ExecutedCommandsState> {
    let state_file = get_state_file_path()?;
    
    if state_file.exists() {
        let content = fs::read_to_string(&state_file)
            .context("Failed to read executed commands state file")?;
        let state: ExecutedCommandsState = serde_json::from_str(&content)
            .context("Failed to parse executed commands state file")?;
        Ok(state)
    } else {
        Ok(ExecutedCommandsState::default())
    }
}

fn save_executed_commands_state(state: &ExecutedCommandsState) -> Result<()> {
    let state_file = get_state_file_path()?;
    let content = serde_json::to_string_pretty(state)
        .context("Failed to serialize executed commands state")?;
    fs::write(&state_file, content)
        .context("Failed to write executed commands state file")?;
    Ok(())
}

fn load_package_list(file_path: &str) -> Result<Vec<String>> {
    if !std::path::Path::new(file_path).exists() {
        println!("{} Package file {} not found, skipping", "[WARN]".yellow(), file_path);
        return Ok(Vec::new());
    }
    
    let content = fs::read_to_string(file_path)
        .context(format!("Failed to read package file: {}", file_path))?;
    let package_list: PackageList = toml::from_str(&content)
        .context("Failed to parse package TOML file")?;
    Ok(package_list.packages)
}

fn get_user_installed_packages() -> Result<Vec<String>> {
    println!("{} Getting list of user-installed packages...", "[INFO]".blue());
    
    let output = Command::new("dnf")
        .args(&["repoquery", "--leaves", "--userinstalled", "--qf", "%{name}\\n"])
        .output()
        .context("Failed to run dnf repoquery command")?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("dnf repoquery failed: {}", stderr);
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut packages: Vec<String> = stdout
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    
    packages.sort();
    packages.dedup();
    
    println!("{} Found {} user-installed packages", "[INFO]".blue(), packages.len());
    Ok(packages)
}

fn get_installed_flatpaks() -> Result<Vec<String>> {
    println!("{} Getting list of installed Flatpak applications...", "[INFO]".blue());
    
    let output = Command::new("flatpak")
        .args(&["list", "--app", "--columns=application"])
        .output()
        .context("Failed to run flatpak list command")?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("flatpak list failed: {}", stderr);
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut apps: Vec<String> = stdout
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty() && line != "Application ID") // Skip header
        .collect();
    
    apps.sort();
    apps.dedup();
    
    println!("{} Found {} installed Flatpak applications", "[INFO]".blue(), apps.len());
    Ok(apps)
}

fn update_system_packages_file(packages: &[String]) -> Result<()> {
    let package_list = PackageList {
        packages: packages.to_vec(),
    };
    
    let content = format!("# System packages to install via dnf\n{}", 
        toml::to_string_pretty(&package_list)
        .context("Failed to serialize package list to TOML")?);
    
    fs::write("config/system-packages.toml", content)
        .context("Failed to write system-packages.toml file")?;
    
    println!("{} Updated config/system-packages.toml with {} packages", "[SUCCESS]".green(), packages.len());
    Ok(())
}

fn update_flatpak_packages_file(packages: &[String]) -> Result<()> {
    let package_list = PackageList {
        packages: packages.to_vec(),
    };
    
    let content = format!("# Flatpak applications to install from Flathub\n{}", 
        toml::to_string_pretty(&package_list)
        .context("Failed to serialize flatpak list to TOML")?);
    
    fs::write("config/flatpak-packages.toml", content)
        .context("Failed to write flatpak-packages.toml file")?;
    
    println!("{} Updated config/flatpak-packages.toml with {} applications", "[SUCCESS]".green(), packages.len());
    Ok(())
}

fn sync_system_packages() -> Result<Vec<String>> {
    println!("{} Synchronizing system packages with installed packages...", "[INFO]".blue());
    
    // Get currently installed user packages
    let installed_packages = get_user_installed_packages()?;
    
    // Load packages from config file
    let mut config_packages = load_package_list("config/system-packages.toml")?;
    
    // Find packages to install (in config but not installed)
    let mut packages_to_install = Vec::new();
    for pkg in &config_packages {
        if !installed_packages.contains(pkg) {
            packages_to_install.push(pkg.clone());
        }
    }
    
    // Find packages to potentially remove (installed but not in config)
    let mut packages_to_keep = Vec::new();
    let mut packages_to_remove = Vec::new();
    
    for pkg in &installed_packages {
        if !config_packages.contains(pkg) {
            println!("\n{} Package '{}' is installed but not in system-packages.toml", "[INFO]".yellow(), pkg);
            if ask_user_confirmation(&format!("Do you want to keep '{}' installed?", pkg))? {
                packages_to_keep.push(pkg.clone());
                config_packages.push(pkg.clone());
            } else {
                packages_to_remove.push(pkg.clone());
            }
        }
    }
    
    // Install missing packages
    if !packages_to_install.is_empty() {
        println!("{} Installing {} packages from config...", "[INFO]".blue(), packages_to_install.len());
        install_system_packages(&Distro::Fedora, &packages_to_install)?;
    }
    
    // Remove unwanted packages
    if !packages_to_remove.is_empty() {
        println!("{} Removing {} unwanted packages...", "[INFO]".blue(), packages_to_remove.len());
        for pkg in &packages_to_remove {
            run_command(&["sudo", "dnf", "remove", "-y", pkg], &format!("Removing package {}", pkg))?;
        }
    }
    
    // Update config file if there were changes
    if !packages_to_keep.is_empty() || !packages_to_remove.is_empty() {
        config_packages.sort();
        config_packages.dedup();
        update_system_packages_file(&config_packages)?;
    }
    
    println!("{} Package synchronization completed", "[SUCCESS]".green());
    println!("  - Installed: {} packages", packages_to_install.len());
    println!("  - Kept: {} packages", packages_to_keep.len()); 
    println!("  - Removed: {} packages", packages_to_remove.len());
    
    Ok(config_packages)
}

fn sync_flatpak_packages() -> Result<Vec<String>> {
    println!("{} Synchronizing Flatpak packages with installed applications...", "[INFO]".blue());
    
    // Get currently installed Flatpak applications
    let installed_flatpaks = get_installed_flatpaks()?;
    
    // Load packages from config file
    let mut config_flatpaks = load_package_list("config/flatpak-packages.toml")?;
    
    // Find packages to install (in config but not installed)
    let mut flatpaks_to_install = Vec::new();
    for app in &config_flatpaks {
        if !installed_flatpaks.contains(app) {
            flatpaks_to_install.push(app.clone());
        }
    }
    
    // Find packages to potentially remove (installed but not in config)
    let mut flatpaks_to_keep = Vec::new();
    let mut flatpaks_to_remove = Vec::new();
    
    for app in &installed_flatpaks {
        if !config_flatpaks.contains(app) {
            println!("\n{} Flatpak application '{}' is installed but not in flatpak-packages.toml", "[INFO]".yellow(), app);
            if ask_user_confirmation(&format!("Do you want to keep '{}' installed?", app))? {
                flatpaks_to_keep.push(app.clone());
                config_flatpaks.push(app.clone());
            } else {
                flatpaks_to_remove.push(app.clone());
            }
        }
    }
    
    // Install missing Flatpak applications
    if !flatpaks_to_install.is_empty() {
        println!("{} Installing {} Flatpak applications from config...", "[INFO]".blue(), flatpaks_to_install.len());
        install_flatpak_packages(&flatpaks_to_install)?;
    }
    
    // Remove unwanted Flatpak applications
    if !flatpaks_to_remove.is_empty() {
        println!("{} Removing {} unwanted Flatpak applications...", "[INFO]".blue(), flatpaks_to_remove.len());
        for app in &flatpaks_to_remove {
            run_command(&["flatpak", "uninstall", "-y", app], &format!("Removing Flatpak application {}", app))?;
        }
    }
    
    // Update config file if there were changes
    if !flatpaks_to_keep.is_empty() || !flatpaks_to_remove.is_empty() {
        config_flatpaks.sort();
        config_flatpaks.dedup();
        update_flatpak_packages_file(&config_flatpaks)?;
    }
    
    println!("{} Flatpak synchronization completed", "[SUCCESS]".green());
    println!("  - Installed: {} applications", flatpaks_to_install.len());
    println!("  - Kept: {} applications", flatpaks_to_keep.len()); 
    println!("  - Removed: {} applications", flatpaks_to_remove.len());
    
    Ok(config_flatpaks)
}

fn detect_distro(os_release: &str) -> Result<Distro> {
    if os_release.contains("Fedora") {
        Ok(Distro::Fedora)
    } else {
        anyhow::bail!("Unsupported distribution. Only Fedora is supported.");
    }
}

fn update_system_packages(_distro: &Distro) -> Result<()> {
    run_command(&["sudo", "dnf", "update", "-y"], "Updating system packages")?;
    Ok(())
}

fn install_system_packages(_distro: &Distro, packages: &[String]) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }
    let mut cmd: Vec<&str> = vec!["sudo", "dnf", "install", "-y", "--skip-unavailable"];
    for pkg in packages {
        cmd.push(pkg);
    }
    run_command(&cmd, "Installing system packages")?;
    Ok(())
}

fn enable_additional_repos(_distro: &Distro) -> Result<()> {
    run_command(&["sudo", "dnf", "install", "-y", "https://download1.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm", "https://download1.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-$(rpm -E %fedora).noarch.rpm"], "Enabling RPM Fusion")?;
    Ok(())
}

fn setup_amd_gpu(_distro: &Distro) -> Result<()> {
    run_command(&["sudo", "dnf", "install", "-y", "--skip-unavailable", "rocm-opencl", "rocm-clinfo", "mesa-dri-drivers"], "Installing ROCm and AMD drivers")?;
    
    // Common GPU setup
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

fn setup_flatpak(_distro: &Distro) -> Result<()> {
    run_command(&["sudo", "dnf", "install", "-y", "--skip-unavailable", "flatpak"], "Installing Flatpak")?;
    run_command(&["flatpak", "remote-add", "--if-not-exists", "flathub", "https://flathub.org/repo/flathub.flatpakrepo"], "Adding Flathub")?;
    Ok(())
}

fn install_flatpak_packages(packages: &[String]) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }
    
    println!("{} Installing Flatpak applications...", "[INFO]".blue());
    
    for package in packages {
        println!("{} Installing Flatpak package: {}", "[INFO]".blue(), package);
        run_command(&["flatpak", "install", "-y", "flathub", package], &format!("Installing {}", package))?;
    }
    
    println!("{} All Flatpak packages installed successfully!", "[SUCCESS]".green());
    Ok(())
}

fn install_wireguard_packages(_distro: &Distro) -> Result<()> {
    // Enable Copr repo for NM WireGuard plugin (idempotent)
    run_command(&["sudo", "dnf", "copr", "enable", "-y", "timn/NetworkManager-wireguard"], "Enabling Copr repo for NetworkManager WireGuard plugin")?;
    // Install NM WireGuard plugin
    run_command(&["sudo", "dnf", "install", "-y", "NetworkManager-wireguard-gtk"], "Installing NetworkManager WireGuard plugin")?;
    Ok(())
}


fn execute_custom_commands(config: &CustomCommandsConfig) -> Result<()> {
    if config.commands.is_empty() && config.run_once.as_ref().map_or(true, |v| v.is_empty()) {
        return Ok(());
    }
    
    println!("{} Executing custom commands...", "[INFO]".blue());
    
    // Load state for run_once commands
    let mut state = load_executed_commands_state()?;
    let mut state_changed = false;
    
    // Execute regular commands
    for (index, command) in config.commands.iter().enumerate() {
        println!("{} Executing command {} of {}: {}", 
                "[INFO]".blue(), index + 1, config.commands.len(), command);
        
        execute_single_command(command)?;
        println!("{} Command completed successfully", "[SUCCESS]".green());
    }
    
    // Execute run_once commands
    if let Some(run_once_commands) = &config.run_once {
        for (index, command) in run_once_commands.iter().enumerate() {
            let command_hash = generate_command_hash(command);
            
            if state.executed_once_commands.contains_key(&command_hash) {
                println!("{} Skipping run-once command {} of {} (already executed): {}", 
                        "[INFO]".blue(), index + 1, run_once_commands.len(), command);
                continue;
            }
            
            println!("{} Executing run-once command {} of {}: {}", 
                    "[INFO]".blue(), index + 1, run_once_commands.len(), command);
            
            execute_single_command(command)?;
            
            // Mark command as executed with metadata
            let execution_record = CommandExecutionRecord {
                command_hash: command_hash.clone(),
                original_command: command.clone(),
                executed_at: get_current_timestamp(),
            };
            state.executed_once_commands.insert(command_hash, execution_record);
            state_changed = true;
            
            println!("{} Run-once command completed successfully", "[SUCCESS]".green());
        }
    }
    
    // Save state if it changed
    if state_changed {
        save_executed_commands_state(&state)?;
    }
    
    println!("{} All custom commands executed successfully!", "[SUCCESS]".green());
    Ok(())
}

fn execute_single_command(command: &str) -> Result<()> {
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
    
    Ok(())
}

fn setup_desktop_environment(distro: &Distro, config: &DesktopConfig) -> Result<()> {
    let de_env = config.environment.as_deref().unwrap_or("cosmic-desktop");
    println!("{} Setting up desktop environment: {}", "[INFO]".blue(), de_env);

    validate_desktop_environment(distro, de_env)?;

    let mut packages_to_install = vec![de_env.to_string()];
    if let Some(additional_packages) = &config.packages {
        packages_to_install.extend_from_slice(additional_packages);
    }

    install_desktop_packages(distro, &packages_to_install)?;

    set_default_desktop_environment(distro, de_env)?;

    // Setup display manager if specified
    if let Some(display_manager) = &config.display_manager {
        setup_display_manager(distro, display_manager)?;
    }

    Ok(())
}

fn validate_desktop_environment(distro: &Distro, de: &str) -> Result<()> {
    println!("{} Validating desktop environment: {}", "[INFO]".blue(), de);
    let available_des = get_available_des(distro)?;
    if !available_des.contains(&de.to_lowercase()) {
        anyhow::bail!("Desktop environment '{}' is not valid. Available options: {:?}", de, available_des);
    }
    Ok(())
}

fn get_available_des(_distro: &Distro) -> Result<Vec<String>> {
    let output = run_command_output(&["dnf", "group", "list", "--available"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let des = stdout.lines()
        .skip_while(|line| !line.trim().starts_with("ID"))
        .skip(1) // Skip the header line itself
        .filter_map(|line| {
            line.split_whitespace().next().map(|s| s.to_lowercase())
        })
        .collect();
    Ok(des)
}

fn install_desktop_packages(_distro: &Distro, packages: &[String]) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }
    println!("{} Installing desktop packages...", "[INFO]".blue());
    let mut cmd: Vec<&str> = vec!["sudo", "dnf", "group", "install", "-y"];
    for pkg in packages {
        cmd.push(pkg);
    }
    run_command(&cmd, "Installing desktop environment group")?;
    Ok(())
}


fn set_default_desktop_environment(_distro: &Distro, de_env: &str) -> Result<()> {
    println!("{} Setting default desktop environment to {}", "[INFO]".blue(), de_env);
    let session_name = de_env.split('-').next().unwrap_or(de_env);
    let desktop_file_content = format!("DESKTOP={}", session_name);
    let cmd = format!("echo '{}' | sudo tee /etc/sysconfig/desktop", desktop_file_content);
    run_command(&["sh", "-c", &cmd], "Setting default desktop session")?;
    Ok(())
}

fn setup_display_manager(_distro: &Distro, display_manager: &str) -> Result<()> {
    println!("{} Setting up display manager: {}", "[INFO]".blue(), display_manager);
    
    // Install the display manager package
    let dm_package = match display_manager {
        "gdm" => "gdm",
        "lightdm" => "lightdm",
        "sddm" => "sddm",
        "cosmic-greeter" => "cosmic-greeter",
        _ => {
            println!("{} Unsupported display manager: {}. Supported: gdm, lightdm, sddm, cosmic-greeter", "[WARN]".yellow(), display_manager);
            return Ok(());
        }
    };
    
    // Install the display manager
    run_command(&["sudo", "dnf", "install", "-y", "--skip-unavailable", dm_package], &format!("Installing {}", dm_package))?;
    
    // Disable current display manager
    let _ = run_command(&["sudo", "systemctl", "disable", "gdm"], "Disabling GDM");
    let _ = run_command(&["sudo", "systemctl", "disable", "lightdm"], "Disabling LightDM");
    let _ = run_command(&["sudo", "systemctl", "disable", "sddm"], "Disabling SDDM");
    let _ = run_command(&["sudo", "systemctl", "disable", "cosmic-greeter"], "Disabling COSMIC Greeter");
    
    // Enable the selected display manager
    let service_name = match display_manager {
        "cosmic-greeter" => "cosmic-greeter",
        _ => display_manager,
    };
    
    run_command(&["sudo", "systemctl", "enable", service_name], &format!("Enabling {}", service_name))?;
    
    // Set as default display manager
    run_command(&["sudo", "systemctl", "set-default", "graphical.target"], "Setting graphical target as default")?;
    
    println!("{} Display manager {} configured successfully", "[SUCCESS]".green(), display_manager);
    println!("{} Reboot required for display manager changes to take effect", "[INFO]".yellow());
    
    Ok(())
}