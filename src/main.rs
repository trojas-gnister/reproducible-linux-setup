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

    /// Enable verbose logging for detailed output
    #[arg(short, long)]
    verbose: bool,

    /// Automatically answer yes to all prompts
    #[arg(short = 'y', long)]
    yes: bool,

    /// Automatically answer no to all prompts
    #[arg(short = 'n', long)]
    no: bool,

    /// Force recreation of all containers
    #[arg(long)]
    force_recreate: bool,

    /// Update container images and recreate if changed
    #[arg(long)]
    update_images: bool,

    /// Never recreate containers (config/systemd only)
    #[arg(long)]
    no_recreate: bool,
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
    drives: Option<Vec<DriveConfig>>,
    desktop: Option<DesktopConfig>,
    flatpak: Option<FlatpakConfig>,
    podman: Option<PodmanConfig>,
    vpn: Option<VpnConfig>,
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
struct FlatpakConfig {
    remotes: Option<Vec<FlatpakRemote>>,
}

#[derive(Deserialize, Debug)]
struct FlatpakRemote {
    name: String,
    url: String,
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
    start_after_creation: bool,
    autostart: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct ContainerState {
    containers: HashMap<String, ContainerInfo>,
}

#[derive(Serialize, Deserialize, Debug)]
struct ContainerInfo {
    config_hash: String,
    image_hash: Option<String>,
    last_updated: u64,
    managed: bool,
}


#[derive(Deserialize, Debug)]
struct VpnConfig {
    #[serde(rename = "type")]
    vpn_type: VpnType,
    conf_path: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
enum VpnType {
    Wireguard,
    Openvpn,
}

#[derive(Deserialize, Debug)]
struct DotfilesConfig {
    setup_bashrc: bool,
    setup_config_dirs: bool,
}

#[derive(Deserialize, Debug)]
struct DriveConfig {
    device: String,
    mount_point: String,
    encrypted: bool,
    filesystem: Option<String>,
    label: Option<String>,
    force_update: Option<bool>,
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

    // Validate flag conflicts
    if args.yes && args.no {
        anyhow::bail!("Cannot specify both --yes and --no flags");
    }

    if args.verbose {
        println!("{} Verbose mode enabled", "[DEBUG]".cyan());
        println!("{} Command line arguments: {:?}", "[DEBUG]".cyan(), args);
    }

    // Handle --initial flag to generate package config files from current system state
    if args.initial {
        println!("{} Generating package configuration from current system state...", "[INFO]".blue());

        if args.verbose {
            println!("{} Creating config directory if it doesn't exist", "[DEBUG]".cyan());
        }
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
    if args.verbose {
        println!("{} Updating system packages...", "[DEBUG]".cyan());
    }
    update_system_packages(&config.distro, args.verbose)?;

    // Set hostname
    if let Some(hostname) = &config.system.hostname {
        let current_hostname = run_command_output(&["hostnamectl", "--static"])?;
        let current = String::from_utf8(current_hostname.stdout)?.trim().to_string();
        if current != *hostname {
            run_command(&["sudo", "hostnamectl", "set-hostname", hostname], &format!("Setting hostname to {}", hostname))?;
            println!("{}", "You may need to reboot for hostname changes.".yellow());
        }
    }

    // Setup drives early as other components may depend on them
    if let Some(drives) = &config.drives {
        setup_drives(drives, args.verbose)?;
    }

    // Synchronize system packages with installed packages
    sync_system_packages(args.yes, args.no, args.verbose)?;

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
    setup_flatpak(&config.distro, config.flatpak.as_ref(), args.verbose)?;

    // Synchronize Flatpak packages with installed applications
    let _flatpak_packages = sync_flatpak_packages(args.yes, args.no, args.verbose)?;

    // Podman setup
    if let Some(podman) = &config.podman {
        // If podman config exists, ensure podman is installed
        if args.verbose {
            println!("{} Podman configuration found, ensuring podman is installed", "[DEBUG]".cyan());
        }

        // Check if podman is installed, install if not
        let podman_check = run_command_output(&["which", "podman"]);
        if podman_check.is_err() {
            if args.verbose {
                println!("{} Podman not found, installing it", "[DEBUG]".cyan());
            }
            install_system_packages(&config.distro, &["podman".to_string()], args.verbose)?;
        } else if args.verbose {
            println!("{} Podman already installed", "[DEBUG]".cyan());
        }

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
                    if ask_user_confirmation(&format!("Container '{}' is managed by this tool but not in the config. Remove it?", container_name), false, false, false)? {
                        run_command(&["podman", "rm", "-f", &container_name], &format!("Removing orphaned container {}", container_name))?;
                    }
                }
            }

            // Smart container lifecycle management
            if let Some(containers) = &podman.containers {
                manage_containers_smart(containers, home_path, &args)?;
            }
    }

    // VPN setup (WireGuard or OpenVPN)
    if let Some(vpn) = &config.vpn {
        match vpn.vpn_type {
            VpnType::Wireguard => setup_wireguard_vpn(vpn)?,
            VpnType::Openvpn => setup_openvpn_vpn(vpn)?,
        }
    }

    // Dotfiles setup
    if let Some(dotfiles) = &config.dotfiles {
        setup_dotfiles(dotfiles, args.yes, args.no, args.verbose)?;
    }

    // Execute custom commands
    if let Some(custom_commands) = &config.custom_commands {
        execute_custom_commands(custom_commands, args.verbose)?;
    }

    // Summary (similar to bash)
    println!("📋 Setup Summary:");
    println!("✅ System updated");
    if let Some(hostname) = config.system.hostname {
        println!("✅ Hostname set to: {}", hostname);
    }
    if config.vpn.is_some() {
        println!("✅ VPN configured with autoconnect");
    }
    // Add more summary items as needed...

    println!("💡 Manual steps: Log out/in or reboot for full effect.");
    println!("{}", "Setup completed successfully!".green());

    Ok(())
}

fn run_command(cmd: &[&str], desc: &str) -> Result<()> {
    println!("{} {}", "[INFO]".blue(), desc);
    // Note: We can't access verbose flag here easily, would need refactoring for full verbose support
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

fn setup_dotfiles(config: &DotfilesConfig, yes: bool, no: bool, verbose: bool) -> Result<()> {
    println!("{} Setting up dotfiles...", "[INFO]".blue());

    let current_dir = env::current_dir()?;
    let home_dir = dirs::home_dir().context("Could not find home directory")?;

    // Setup .bashrc
    if config.setup_bashrc {
        setup_bashrc(&current_dir, &home_dir, yes, no, verbose)?;
    }

    // Setup .config directories
    if config.setup_config_dirs {
        setup_config_dirs(&current_dir, &home_dir, yes, no, verbose)?;
    }

    println!("{} Dotfiles setup completed!", "[SUCCESS]".green());
    Ok(())
}

fn setup_bashrc(project_dir: &Path, home_dir: &Path, yes: bool, no: bool, verbose: bool) -> Result<()> {
    let project_bashrc = project_dir.join(".bashrc");
    let home_bashrc = home_dir.join(".bashrc");

    if !project_bashrc.exists() {
        println!("{} No .bashrc found in project directory, skipping", "[WARN]".yellow());
        return Ok(());
    }

    if home_bashrc.exists() {
        println!("{} Found existing .bashrc in home directory", "[INFO]".blue());
        if ask_user_confirmation("Do you want to replace your existing .bashrc with the one from this project?", yes, no, verbose)? {
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

fn setup_config_dirs(project_dir: &Path, home_dir: &Path, yes: bool, no: bool, verbose: bool) -> Result<()> {
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
                if ask_user_confirmation(&format!("Do you want to replace your existing {} config with the one from this project?", dir_name), yes, no, verbose)? {
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

fn ask_user_confirmation(prompt: &str, yes: bool, no: bool, verbose: bool) -> Result<bool> {
    if yes {
        if verbose {
            println!("{} Auto-answering YES: {}", "[DEBUG]".cyan(), prompt);
        }
        println!("{} (y/n): y", prompt);
        return Ok(true);
    }

    if no {
        if verbose {
            println!("{} Auto-answering NO: {}", "[DEBUG]".cyan(), prompt);
        }
        println!("{} (y/n): n", prompt);
        return Ok(false);
    }

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

fn get_container_state_file_path() -> Result<std::path::PathBuf> {
    let home_dir = dirs::home_dir().context("Could not find home directory")?;
    let config_dir = home_dir.join(".config").join("repro-setup");
    fs::create_dir_all(&config_dir).context("Failed to create config directory")?;
    Ok(config_dir.join("container_state.json"))
}

fn load_container_state() -> Result<ContainerState> {
    let state_file = get_container_state_file_path()?;

    if state_file.exists() {
        let content = fs::read_to_string(&state_file)
            .context("Failed to read container state file")?;
        let state: ContainerState = serde_json::from_str(&content)
            .context("Failed to parse container state file")?;
        Ok(state)
    } else {
        Ok(ContainerState::default())
    }
}

fn save_container_state(state: &ContainerState) -> Result<()> {
    let state_file = get_container_state_file_path()?;
    let content = serde_json::to_string_pretty(state)
        .context("Failed to serialize container state")?;
    fs::write(&state_file, content)
        .context("Failed to write container state file")?;
    Ok(())
}

fn generate_container_config_hash(container: &Container) -> String {
    let mut hasher = Sha256::new();
    hasher.update(container.name.as_bytes());
    hasher.update(container.image.as_bytes());
    hasher.update(container.raw_flags.as_deref().unwrap_or("").as_bytes());
    hasher.update(&[if container.start_after_creation { 1 } else { 0 }]);
    hasher.update(&[if container.autostart.unwrap_or(false) { 1 } else { 0 }]);
    format!("{:x}", hasher.finalize())
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

fn sync_system_packages(yes: bool, no: bool, verbose: bool) -> Result<Vec<String>> {
    println!("{} Synchronizing system packages with installed packages...", "[INFO]".blue());

    // Get currently installed user packages
    let installed_packages = get_user_installed_packages()?;
    if verbose {
        println!("{} Found {} installed packages", "[DEBUG]".cyan(), installed_packages.len());
    }

    // Load packages from config file
    let mut config_packages = load_package_list("config/system-packages.toml")?;
    if verbose {
        println!("{} Loaded {} packages from config", "[DEBUG]".cyan(), config_packages.len());
    }

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
            if ask_user_confirmation(&format!("Do you want to keep '{}' installed?", pkg), yes, no, verbose)? {
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
        install_system_packages(&Distro::Fedora, &packages_to_install, verbose)?;
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

fn sync_flatpak_packages(yes: bool, no: bool, verbose: bool) -> Result<Vec<String>> {
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
            if ask_user_confirmation(&format!("Do you want to keep '{}' installed?", app), yes, no, verbose)? {
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

fn update_system_packages(_distro: &Distro, verbose: bool) -> Result<()> {
    if verbose {
        println!("{} Running: sudo dnf update -y", "[DEBUG]".cyan());
    }
    run_command(&["sudo", "dnf", "update", "-y"], "Updating system packages")?;
    Ok(())
}

fn install_system_packages(_distro: &Distro, packages: &[String], verbose: bool) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    if verbose {
        println!("{} Installing {} system packages: {}", "[DEBUG]".cyan(), packages.len(), packages.join(", "));
    }

    let mut cmd: Vec<&str> = vec!["sudo", "dnf", "install", "-y", "--skip-unavailable"];
    for pkg in packages {
        cmd.push(pkg);
    }
    run_command(&cmd, "Installing system packages")?;
    Ok(())
}

fn enable_additional_repos(_distro: &Distro) -> Result<()> {
    let output = std::process::Command::new("rpm")
        .args(["-E", "%fedora"])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to get Fedora version: {}", e))?;

    let fedora_version = String::from_utf8(output.stdout)
        .map_err(|e| anyhow::anyhow!("Invalid UTF-8 format: {}", e))?
        .trim()
        .to_string();

    let rpmfusion_url = format!("https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-{}.noarch.rpm", fedora_version);

    run_command(&["sudo", "dnf", "install", "-y", &rpmfusion_url], "Enabling RPM Fusion")?;
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

fn setup_flatpak(_distro: &Distro, flatpak_config: Option<&FlatpakConfig>, verbose: bool) -> Result<()> {
    if verbose {
        println!("{} Installing Flatpak and setting up remotes", "[DEBUG]".cyan());
    }

    run_command(&["sudo", "dnf", "install", "-y", "--skip-unavailable", "flatpak"], "Installing Flatpak")?;

    // Add default Flathub if no custom config is provided
    if flatpak_config.is_none() {
        run_command(&["flatpak", "remote-add", "--if-not-exists", "flathub", "https://flathub.org/repo/flathub.flatpakrepo"], "Adding Flathub")?;
        return Ok(());
    }

    // Add configured remotes
    if let Some(config) = flatpak_config {
        if let Some(remotes) = &config.remotes {
            for remote in remotes {
                if verbose {
                    println!("{} Adding Flatpak remote: {} -> {}", "[DEBUG]".cyan(), remote.name, remote.url);
                }
                run_command(&["flatpak", "remote-add", "--if-not-exists", &remote.name, &remote.url],
                          &format!("Adding Flatpak remote {}", remote.name))?;
            }
        }
    }

    Ok(())
}


fn install_flatpak_packages(packages: &[String]) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    println!("{} Installing Flatpak applications...", "[INFO]".blue());

    for package in packages {
        let (remote, app_id) = parse_flatpak_package(package);
        println!("{} Installing Flatpak package: {} from {}", "[INFO]".blue(), app_id, remote);
        run_command(&["flatpak", "install", "-y", remote, app_id], &format!("Installing {} from {}", app_id, remote))?;
    }

    println!("{} All Flatpak packages installed successfully!", "[SUCCESS]".green());
    Ok(())
}

fn parse_flatpak_package(package: &str) -> (&str, &str) {
    if let Some(colon_pos) = package.find(':') {
        let (remote, app_id) = package.split_at(colon_pos);
        (remote, &app_id[1..]) // Skip the colon
    } else {
        ("flathub", package) // Default to flathub
    }
}

fn setup_wireguard_vpn(vpn: &VpnConfig) -> Result<()> {
    // Install WireGuard tools
    run_command(&["sudo", "dnf", "install", "-y", "--skip-unavailable", "wireguard-tools"], "Installing WireGuard tools")?;

    // Try to install NetworkManager WireGuard plugin gracefully
    install_wireguard_packages_graceful()?;

    // Derive interface name from conf_path (e.g., "wg0.conf" -> "wg0")
    use std::path::Path;
    let conf_path = Path::new(&vpn.conf_path);
    let interface_name = conf_path.file_stem()
        .context("Invalid conf_path: missing file stem")?
        .to_str()
        .context("Invalid conf_path: non-UTF8 stem")?
        .to_string();

    // Ensure system-connections dir exists
    run_command(&["sudo", "mkdir", "-p", "/etc/NetworkManager/system-connections"], "Creating NetworkManager system-connections directory")?;

    // Remove existing connection if present (idempotent)
    let _ = run_command(&["nmcli", "connection", "delete", &interface_name], "Removing existing WireGuard connection if present");

    // Import config into NetworkManager
    run_command(&["nmcli", "connection", "import", "type", "wireguard", "file", &vpn.conf_path], &format!("Importing WireGuard config for {}", interface_name))?;

    // Set autoconnect and priority
    run_command(&["nmcli", "connection", "modify", &interface_name, "connection.autoconnect", "yes"], &format!("Enabling autoconnect for {}", interface_name))?;
    run_command(&["nmcli", "connection", "modify", &interface_name, "connection.autoconnect-priority", "10"], &format!("Setting autoconnect priority for {}", interface_name))?;

    // Reload NetworkManager and activate
    run_command(&["sudo", "systemctl", "reload", "NetworkManager"], "Reloading NetworkManager to apply connection")?;
    run_command(&["nmcli", "connection", "up", &interface_name], &format!("Activating WireGuard connection {}", interface_name))?;

    Ok(())
}

fn setup_openvpn_vpn(vpn: &VpnConfig) -> Result<()> {
    // Install OpenVPN and NetworkManager plugin
    run_command(&["sudo", "dnf", "install", "-y", "openvpn", "NetworkManager-openvpn", "NetworkManager-openvpn-gnome"], "Installing OpenVPN and NetworkManager plugin")?;

    // Derive connection name from conf_path
    use std::path::Path;
    let conf_path = Path::new(&vpn.conf_path);
    let connection_name = conf_path.file_stem()
        .context("Invalid conf_path: missing file stem")?
        .to_str()
        .context("Invalid conf_path: non-UTF8 stem")?
        .to_string();

    // Remove existing connection if present
    let _ = run_command(&["nmcli", "connection", "delete", &connection_name], "Removing existing OpenVPN connection if present");

    // Import OpenVPN config
    run_command(&["nmcli", "connection", "import", "type", "openvpn", "file", &vpn.conf_path], &format!("Importing OpenVPN config for {}", connection_name))?;

    // Set autoconnect
    run_command(&["nmcli", "connection", "modify", &connection_name, "connection.autoconnect", "yes"], &format!("Enabling autoconnect for {}", connection_name))?;
    run_command(&["nmcli", "connection", "modify", &connection_name, "connection.autoconnect-priority", "10"], &format!("Setting autoconnect priority for {}", connection_name))?;

    // Activate the connection
    run_command(&["nmcli", "connection", "up", &connection_name], &format!("Activating OpenVPN connection {}", connection_name))?;

    Ok(())
}

fn install_wireguard_packages_graceful() -> Result<()> {
    // Try to enable Copr repo, but don't fail if it's not available for current Fedora version
    let copr_result = run_command(&["sudo", "dnf", "copr", "enable", "-y", "timn/NetworkManager-wireguard"], "Enabling Copr repo for NetworkManager WireGuard plugin");

    if copr_result.is_err() {
        println!("{} NetworkManager WireGuard plugin Copr repo not available for this Fedora version, using basic WireGuard tools only", "[WARNING]".yellow());
        return Ok(());
    }

    // Try to install the plugin, but don't fail if it's not available
    let plugin_result = run_command(&["sudo", "dnf", "install", "-y", "NetworkManager-wireguard-gtk"], "Installing NetworkManager WireGuard plugin");

    if plugin_result.is_err() {
        println!("{} NetworkManager WireGuard plugin not available, using basic WireGuard tools only", "[WARNING]".yellow());
    }

    Ok(())
}


fn execute_custom_commands(config: &CustomCommandsConfig, verbose: bool) -> Result<()> {
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

            if verbose {
                println!("{} Command hash: {} for: {}", "[DEBUG]".cyan(), &command_hash[..8], command);
            }

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

fn setup_drives(drives: &[DriveConfig], verbose: bool) -> Result<()> {
    if drives.is_empty() {
        return Ok(());
    }

    println!("{} Setting up drive mounting...", "[INFO]".blue());

    // Install required packages for drive mounting
    install_drive_packages(verbose)?;

    for drive in drives {
        setup_single_drive(drive, verbose)?;
    }

    println!("{} All drives configured successfully!", "[SUCCESS]".green());
    Ok(())
}

fn install_drive_packages(verbose: bool) -> Result<()> {
    if verbose {
        println!("{} Installing drive mounting packages", "[DEBUG]".cyan());
    }

    // Install cryptsetup for encrypted drives and other utilities
    run_command(&["sudo", "dnf", "install", "-y", "--skip-unavailable", "cryptsetup", "util-linux"], "Installing drive mounting utilities")?;
    Ok(())
}

fn setup_single_drive(drive: &DriveConfig, verbose: bool) -> Result<()> {
    println!("{} Configuring drive {} -> {}", "[INFO]".blue(), drive.device, drive.mount_point);

    // Check if device exists
    if !std::path::Path::new(&drive.device).exists() {
        println!("{} Device {} does not exist, skipping", "[WARN]".yellow(), drive.device);
        return Ok(());
    }

    // Create mount point
    run_command(&["sudo", "mkdir", "-p", &drive.mount_point], &format!("Creating mount point {}", drive.mount_point))?;

    if drive.encrypted {
        setup_encrypted_drive(drive, verbose)?;
    } else {
        setup_unencrypted_drive(drive, verbose)?;
    }

    Ok(())
}

fn setup_unencrypted_drive(drive: &DriveConfig, verbose: bool) -> Result<()> {
    if verbose {
        println!("{} Setting up unencrypted drive {}", "[DEBUG]".cyan(), drive.device);
    }

    // Get filesystem type if not specified
    let filesystem = drive.filesystem.as_deref().unwrap_or("auto");

    // Get UUID of the device
    let uuid_output = run_command_output(&["sudo", "blkid", "-s", "UUID", "-o", "value", &drive.device])?;
    let uuid = String::from_utf8_lossy(&uuid_output.stdout).trim().to_string();

    if uuid.is_empty() {
        println!("{} Could not get UUID for {}, using device path", "[WARN]".yellow(), drive.device);
        add_to_fstab(&drive.device, &drive.mount_point, filesystem, "defaults", drive.force_update.unwrap_or(false), verbose)?;
    } else {
        let uuid_device = format!("UUID={}", uuid);
        add_to_fstab(&uuid_device, &drive.mount_point, filesystem, "defaults", drive.force_update.unwrap_or(false), verbose)?;
    }

    // Mount the drive
    run_command(&["sudo", "mount", &drive.device, &drive.mount_point], &format!("Mounting {} to {}", drive.device, drive.mount_point))?;

    println!("{} Unencrypted drive {} mounted successfully", "[SUCCESS]".green(), drive.device);
    Ok(())
}

fn setup_encrypted_drive(drive: &DriveConfig, verbose: bool) -> Result<()> {
    if verbose {
        println!("{} Setting up encrypted drive {}", "[DEBUG]".cyan(), drive.device);
    }

    // Generate a mapper name based on the label or device name
    let default_name = drive.device.replace("/dev/", "").replace("/", "_");
    let mapper_name = drive.label.as_deref().unwrap_or(&default_name);
    let mapper_path = format!("/dev/mapper/{}", mapper_name);

    // Get UUID of the encrypted device
    let uuid_output = run_command_output(&["sudo", "blkid", "-s", "UUID", "-o", "value", &drive.device])?;
    let uuid = String::from_utf8_lossy(&uuid_output.stdout).trim().to_string();

    if uuid.is_empty() {
        anyhow::bail!("Could not get UUID for encrypted device {}", drive.device);
    }

    // Add to crypttab
    add_to_crypttab(mapper_name, &uuid, drive.force_update.unwrap_or(false), verbose)?;

    // Check if the encrypted device is already opened
    if !std::path::Path::new(&mapper_path).exists() {
        println!("{} Opening encrypted device {} (you may need to enter passphrase)", "[INFO]".blue(), drive.device);
        run_command(&["sudo", "cryptsetup", "open", &drive.device, mapper_name], &format!("Opening encrypted device {}", drive.device))?;
    }

    // Get filesystem type if not specified
    let filesystem = drive.filesystem.as_deref().unwrap_or("auto");

    // Add to fstab using the mapper path
    add_to_fstab(&mapper_path, &drive.mount_point, filesystem, "defaults", drive.force_update.unwrap_or(false), verbose)?;

    // Mount the decrypted drive
    run_command(&["sudo", "mount", &mapper_path, &drive.mount_point], &format!("Mounting decrypted {} to {}", mapper_path, drive.mount_point))?;

    println!("{} Encrypted drive {} mounted successfully", "[SUCCESS]".green(), drive.device);
    Ok(())
}

fn add_to_crypttab(mapper_name: &str, uuid: &str, force_update: bool, verbose: bool) -> Result<()> {
    if verbose {
        println!("{} Adding {} to /etc/crypttab", "[DEBUG]".cyan(), mapper_name);
    }

    let crypttab_entry = format!("{} UUID={} none luks", mapper_name, uuid);

    // Read current crypttab content
    let crypttab_content = std::fs::read_to_string("/etc/crypttab").unwrap_or_default();

    // Check if entry already exists
    let entry_exists = crypttab_content.lines().any(|line| {
        line.trim().starts_with(&format!("{} ", mapper_name)) || line.trim() == mapper_name
    });

    if entry_exists && !force_update {
        println!("{} Entry for {} already exists in /etc/crypttab", "[INFO]".blue(), mapper_name);
        return Ok(());
    }

    if entry_exists && force_update {
        if verbose {
            println!("{} Updating existing {} entry in /etc/crypttab", "[DEBUG]".cyan(), mapper_name);
        }

        // Remove existing entry and add new one
        let updated_content = crypttab_content
            .lines()
            .filter(|line| !line.trim().starts_with(&format!("{} ", mapper_name)) && line.trim() != mapper_name)
            .collect::<Vec<_>>()
            .join("\n");

        let final_content = if updated_content.trim().is_empty() {
            crypttab_entry
        } else {
            format!("{}\n{}", updated_content, crypttab_entry)
        };

        // Write updated content
        let write_cmd = format!("echo '{}' | sudo tee /etc/crypttab > /dev/null", final_content);
        run_command(&["sh", "-c", &write_cmd], &format!("Updating {} in /etc/crypttab", mapper_name))?;
    } else {
        // Append new entry
        let append_cmd = format!("echo '{}' | sudo tee -a /etc/crypttab > /dev/null", crypttab_entry);
        run_command(&["sh", "-c", &append_cmd], &format!("Adding {} to /etc/crypttab", mapper_name))?;
    }

    println!("{} Added {} to /etc/crypttab", "[SUCCESS]".green(), mapper_name);
    Ok(())
}

fn add_to_fstab(device: &str, mount_point: &str, filesystem: &str, options: &str, force_update: bool, verbose: bool) -> Result<()> {
    if verbose {
        println!("{} Adding {} to /etc/fstab", "[DEBUG]".cyan(), device);
    }

    let fstab_entry = format!("{} {} {} {} 0 2", device, mount_point, filesystem, options);

    // Read current fstab content
    let fstab_content = std::fs::read_to_string("/etc/fstab").unwrap_or_default();

    // Check if entry already exists (check for mount point since device might change)
    let entry_exists = fstab_content.lines().any(|line| {
        line.trim().split_whitespace().nth(1) == Some(mount_point)
    });

    if entry_exists && !force_update {
        println!("{} Entry for {} already exists in /etc/fstab", "[INFO]".blue(), mount_point);
        return Ok(());
    }

    // Backup fstab
    run_command(&["sudo", "cp", "/etc/fstab", "/etc/fstab.backup"], "Backing up /etc/fstab")?;

    if entry_exists && force_update {
        if verbose {
            println!("{} Updating existing {} entry in /etc/fstab", "[DEBUG]".cyan(), mount_point);
        }

        // Remove existing entry and add new one
        let updated_content = fstab_content
            .lines()
            .filter(|line| {
                line.trim().split_whitespace().nth(1) != Some(mount_point)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let final_content = if updated_content.trim().is_empty() {
            fstab_entry
        } else {
            format!("{}\n{}", updated_content, fstab_entry)
        };

        // Write updated content
        let write_cmd = format!("echo '{}' | sudo tee /etc/fstab > /dev/null", final_content);
        run_command(&["sh", "-c", &write_cmd], &format!("Updating {} in /etc/fstab", mount_point))?;
    } else {
        // Append new entry
        let append_cmd = format!("echo '{}' | sudo tee -a /etc/fstab > /dev/null", fstab_entry);
        run_command(&["sh", "-c", &append_cmd], &format!("Adding {} to /etc/fstab", mount_point))?;
    }

    println!("{} Added {} to /etc/fstab", "[SUCCESS]".green(), mount_point);
    Ok(())
}

fn manage_containers_smart(containers: &[Container], home_path: &str, args: &Args) -> Result<()> {
    println!("{} Managing containers with smart lifecycle", "[INFO]".blue());

    // Load container state
    let mut state = load_container_state()?;

    // Get existing containers
    let existing_containers = get_existing_containers()?;

    // Analyze what needs to be done
    let mut actions = Vec::new();

    for container in containers {
        let action = determine_container_action(container, &state, &existing_containers, args)?;
        actions.push((container, action));
    }

    // Show summary of actions
    if !actions.is_empty() && !args.yes {
        show_container_action_summary(&actions);
        if !ask_user_confirmation("Proceed with container operations?", args.yes, args.no, args.verbose)? {
            println!("{} Container operations cancelled", "[INFO]".blue());
            return Ok(());
        }
    }

    // Execute actions
    for (container, action) in &actions {
        execute_container_action(container, action, home_path, &mut state, args)?;
    }

    // Save updated state
    save_container_state(&state)?;

    // Setup autostart for containers that need it
    let autostart_containers: Vec<_> = containers.iter()
        .filter(|c| c.autostart.unwrap_or(false))
        .collect();

    if !autostart_containers.is_empty() {
        setup_container_autostart(&autostart_containers, args.verbose)?;
    }

    Ok(())
}

#[derive(Debug, Clone)]
enum ContainerAction {
    Skip,
    Create,
    Update,
    Recreate,
}

fn get_existing_containers() -> Result<HashMap<String, String>> {
    let output = Command::new("podman")
        .args(&["ps", "-a", "--format", "{{.Names}}"])
        .output()
        .context("Failed to list existing containers")?;

    let mut containers = HashMap::new();
    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        let name = line.trim();
        if !name.is_empty() {
            containers.insert(name.to_string(), name.to_string());
        }
    }

    Ok(containers)
}

fn determine_container_action(
    container: &Container,
    state: &ContainerState,
    existing_containers: &HashMap<String, String>,
    args: &Args,
) -> Result<ContainerAction> {
    let current_hash = generate_container_config_hash(container);
    let exists = existing_containers.contains_key(&container.name);

    // Check CLI overrides
    if args.force_recreate {
        return Ok(if exists { ContainerAction::Recreate } else { ContainerAction::Create });
    }

    if args.no_recreate {
        return Ok(ContainerAction::Skip);
    }

    // Check if container exists
    if !exists {
        return Ok(ContainerAction::Create);
    }

    // Check if we have previous state
    if let Some(container_info) = state.containers.get(&container.name) {
        if container_info.config_hash != current_hash {
            return Ok(ContainerAction::Update);
        } else {
            return Ok(ContainerAction::Skip);
        }
    }

    // Container exists but no state - likely first run with existing container
    Ok(ContainerAction::Update)
}

fn show_container_action_summary(actions: &[(&Container, ContainerAction)]) {
    println!("\n{} Container Actions Summary:", "[INFO]".blue());

    for (container, action) in actions {
        match action {
            ContainerAction::Skip => continue,
            ContainerAction::Create => println!("  {} {}: Create new container", "✨".green(), container.name),
            ContainerAction::Update => println!("  {} {}: Update (config changed)", "🔄".yellow(), container.name),
            ContainerAction::Recreate => println!("  {} {}: Force recreate", "🔨".red(), container.name),
        }
    }
    println!();
}

fn execute_container_action(
    container: &Container,
    action: &ContainerAction,
    home_path: &str,
    state: &mut ContainerState,
    args: &Args,
) -> Result<()> {
    match action {
        ContainerAction::Skip => {
            if args.verbose {
                println!("{} Skipping {} (no changes)", "[DEBUG]".cyan(), container.name);
            }
            return Ok(());
        }
        ContainerAction::Create => {
            println!("{} Creating container {}", "[INFO]".blue(), container.name);
        }
        ContainerAction::Update => {
            println!("{} Updating container {} (config changed)", "[INFO]".blue(), container.name);
            // Remove existing container
            run_command(&["podman", "rm", "-f", &container.name], &format!("Removing existing container {}", container.name))?;
        }
        ContainerAction::Recreate => {
            println!("{} Recreating container {}", "[INFO]".blue(), container.name);
            // Remove existing container
            run_command(&["podman", "rm", "-f", &container.name], &format!("Removing existing container {}", container.name))?;
        }
    }

    // Check for conflicting startup configurations
    if container.start_after_creation && container.autostart.unwrap_or(false) {
        println!("{} Container '{}' has both start_after_creation=true and autostart=true", "[WARNING]".yellow(), container.name);
        println!("  This creates a conflict between immediate startup and systemd management.");
        println!("  Consider renaming 'start_after_creation' to 'immediate_start' for clarity.");
        println!("  Using systemd management (autostart=true) and skipping immediate startup.");
    }

    // Only start immediately if not managed by systemd autostart
    if container.start_after_creation && !container.autostart.unwrap_or(false) {
        create_and_start_container(container, home_path)?;
    } else if !container.start_after_creation {
        // Just create the container without starting
        create_container_only(container, home_path)?;
    }

    // Update state
    let container_info = ContainerInfo {
        config_hash: generate_container_config_hash(container),
        image_hash: None, // TODO: Get actual image hash
        last_updated: get_current_timestamp(),
        managed: true,
    };

    state.containers.insert(container.name.clone(), container_info);

    println!("{} Container {} processed successfully", "[SUCCESS]".green(), container.name);
    Ok(())
}

fn create_and_start_container(container: &Container, home_path: &str) -> Result<()> {
    let mut command = format!("podman run -d --name={} --label managed-by=repro-setup", container.name);

    if let Some(flags) = &container.raw_flags {
        let replaced_flags = flags.replace("$HOME", home_path);
        command.push(' ');
        command.push_str(&replaced_flags);
    }

    command.push(' ');
    command.push_str(&container.image);

    let output = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .output()
        .with_context(|| format!("Failed to start container: {}", container.name))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("{} Failed to start container {}: {}", "[ERROR]".red(), container.name, stderr);
        anyhow::bail!("Container startup failed: {}", container.name);
    }

    Ok(())
}

fn create_container_only(container: &Container, home_path: &str) -> Result<()> {
    let mut command = format!("podman create --name={} --label managed-by=repro-setup", container.name);

    if let Some(flags) = &container.raw_flags {
        let replaced_flags = flags.replace("$HOME", home_path);
        command.push(' ');
        command.push_str(&replaced_flags);
    }

    command.push(' ');
    command.push_str(&container.image);

    let output = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .output()
        .with_context(|| format!("Failed to create container: {}", container.name))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("{} Failed to create container {}: {}", "[ERROR]".red(), container.name, stderr);
        anyhow::bail!("Container creation failed: {}", container.name);
    }

    Ok(())
}

fn setup_container_autostart(containers: &[&Container], verbose: bool) -> Result<()> {
    let autostart_containers: Vec<_> = containers.iter()
        .filter(|cont| cont.autostart.unwrap_or(false))
        .collect();

    if autostart_containers.is_empty() {
        if verbose {
            println!("{} No containers configured for autostart", "[DEBUG]".cyan());
        }
        return Ok(());
    }

    println!("{} Setting up autostart for {} containers using Quadlet", "[INFO]".blue(), autostart_containers.len());

    // Create systemd user directory for Quadlet
    let home_dir = dirs::home_dir().context("Could not find home directory")?;
    let quadlet_dir = home_dir.join(".config/containers/systemd");
    std::fs::create_dir_all(&quadlet_dir)
        .context("Failed to create Quadlet directory")?;

    for container in &autostart_containers {
        create_quadlet_file(container, &quadlet_dir, verbose)?;
    }

    // Enable lingering for the user so services start without login
    run_command(&["sudo", "loginctl", "enable-linger", &std::env::var("USER")?], "Enabling user lingering for autostart")?;

    // Reload systemd user daemon to pick up new Quadlet files
    run_command(&["systemctl", "--user", "daemon-reload"], "Reloading systemd user daemon")?;

    // For Quadlet-generated services, we don't need to manually enable them
    // The .container files with WantedBy=default.target will auto-enable
    for container in &autostart_containers {
        if verbose {
            println!("{} Container {} configured for autostart via Quadlet", "[SUCCESS]".green(), container.name);
        }
    }

    println!("{} Quadlet autostart configuration completed!", "[SUCCESS]".green());
    Ok(())
}

fn create_quadlet_file(container: &Container, quadlet_dir: &std::path::Path, verbose: bool) -> Result<()> {
    let quadlet_file = quadlet_dir.join(format!("{}.container", container.name));

    if verbose {
        println!("{} Creating Quadlet file: {}", "[DEBUG]".cyan(), quadlet_file.display());
    }

    // Parse raw_flags to extract individual options
    let mut quadlet_content = String::new();
    quadlet_content.push_str("[Unit]\n");
    quadlet_content.push_str(&format!("Description=Container {}\n", container.name));
    quadlet_content.push_str("Wants=network-online.target\n");
    quadlet_content.push_str("After=network-online.target\n");
    quadlet_content.push_str("RequiresMountsFor=%t/containers\n\n");

    quadlet_content.push_str("[Container]\n");
    quadlet_content.push_str(&format!("Image={}\n", container.image));
    quadlet_content.push_str(&format!("ContainerName={}\n", container.name));

    // Add labels
    quadlet_content.push_str("Label=managed-by=repro-setup\n");

    // Parse raw_flags and convert to Quadlet format
    if let Some(flags) = &container.raw_flags {
        parse_raw_flags_to_quadlet(flags, &mut quadlet_content)?;
    }

    quadlet_content.push_str("\n[Service]\n");
    quadlet_content.push_str("Restart=always\n");
    quadlet_content.push_str("TimeoutStartSec=900\n\n");

    quadlet_content.push_str("[Install]\n");
    quadlet_content.push_str("WantedBy=default.target\n");

    // Write the Quadlet file
    std::fs::write(&quadlet_file, quadlet_content)
        .context(format!("Failed to write Quadlet file for {}", container.name))?;

    println!("{} Created Quadlet file for {}", "[SUCCESS]".green(), container.name);
    Ok(())
}

fn parse_raw_flags_to_quadlet(raw_flags: &str, content: &mut String) -> Result<()> {
    // Get home directory for volume path expansion
    let home_dir = dirs::home_dir().context("Could not find home directory")?;
    let home_path = home_dir.to_str().context("Invalid home directory path")?;

    // Split raw_flags and convert to Quadlet format
    let flags: Vec<&str> = raw_flags.split_whitespace().collect();
    let mut i = 0;

    while i < flags.len() {
        match flags[i] {
            "-p" | "--publish" => {
                if i + 1 < flags.len() {
                    content.push_str(&format!("PublishPort={}\n", flags[i + 1]));
                    i += 2;
                } else {
                    i += 1;
                }
            },
            "-v" | "--volume" => {
                if i + 1 < flags.len() {
                    // Replace $HOME with actual home path for Quadlet
                    let volume_spec = flags[i + 1].replace("$HOME", home_path);
                    content.push_str(&format!("Volume={}\n", volume_spec));
                    i += 2;
                } else {
                    i += 1;
                }
            },
            "-e" | "--env" => {
                if i + 1 < flags.len() {
                    content.push_str(&format!("Environment={}\n", flags[i + 1]));
                    i += 2;
                } else {
                    i += 1;
                }
            },
            "--device" => {
                if i + 1 < flags.len() {
                    // In Quadlet, devices are handled differently - add to PodmanArgs
                    content.push_str(&format!("PodmanArgs=--device={}\n", flags[i + 1]));
                    i += 2;
                } else {
                    i += 1;
                }
            },
            "--security-opt" => {
                if i + 1 < flags.len() {
                    // Only add SecurityLabelDisable for seccomp options
                    if flags[i + 1].contains("seccomp") {
                        content.push_str("SecurityLabelDisable=true\n");
                    }
                    i += 2;
                } else {
                    i += 1;
                }
            },
            "--shm-size" => {
                if i + 1 < flags.len() {
                    content.push_str(&format!("ShmSize={}\n", flags[i + 1]));
                    i += 2;
                } else {
                    i += 1;
                }
            },
            "--restart" => {
                // Skip restart flag as it's handled by systemd
                if i + 1 < flags.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            },
            "--cap-add" => {
                if i + 1 < flags.len() {
                    content.push_str(&format!("AddCapability={}\n", flags[i + 1]));
                    i += 2;
                } else {
                    i += 1;
                }
            },
            _ => {
                // Skip unknown flags for now
                i += 1;
            }
        }
    }

    Ok(())
}
