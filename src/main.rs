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
#[command(version, about = "FedoraForge - Forge your perfect Fedora system with declarative configuration")]
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
    users_groups: Option<UsersGroupsConfig>,
}

#[derive(Deserialize, Debug)]
struct SystemConfig {
    hostname: Option<String>,
    enable_amd_gpu: bool,
    enable_rpm_fusion: bool,
    enable_winapps: bool,
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

#[derive(Deserialize, Debug)]
struct WinAppsConfig {
    rdp_user: String,
    rdp_pass: String,
    rdp_domain: Option<String>,
    rdp_ip: String,
    vm_name: Option<String>,
    waflavor: String,
    rdp_scale: Option<String>,
    removable_media: Option<String>,
    debug: Option<bool>,
    multimon: Option<bool>,
    rdp_flags: Option<String>,
    rdp_env: Option<String>,
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

#[derive(Serialize, Deserialize, Debug, Default)]
struct DotfilesState {
    bashrc_hash: Option<String>,
    config_dirs: HashMap<String, String>, // dir_name -> hash
}

#[derive(Serialize, Deserialize, Debug)]
struct PackageList {
    packages: Vec<String>,
}

// Services configuration structures
#[derive(Deserialize, Debug)]
struct SystemServicesConfig {
    services: Option<HashMap<String, ServiceState>>,
    custom_services: Option<Vec<CustomService>>,
}

#[derive(Deserialize, Debug)]
struct UserServicesConfig {
    services: Option<HashMap<String, ServiceState>>,
    custom_services: Option<Vec<CustomService>>,
    applications: Option<HashMap<String, ApplicationAutostart>>,
}

#[derive(Deserialize, Debug)]
struct ServiceState {
    enabled: bool,
    started: bool,
}

#[derive(Deserialize, Debug)]
struct ApplicationAutostart {
    enabled: bool,
    restart_policy: Option<String>, // "never", "always", "on-failure"
    delay: Option<u64>,             // seconds delay after login
    args: Option<Vec<String>>,      // command line arguments
    environment: Option<HashMap<String, String>>, // environment variables
}

#[derive(Deserialize, Debug)]
struct CustomService {
    name: String,
    enabled: bool,
    started: bool,
    service_definition: String,
    timer_definition: Option<String>,
}

#[derive(Debug)]
struct CurrentServiceInfo {
    enabled: bool,
    active: bool,
    exists: bool,
    is_custom: bool,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct CustomServicesState {
    system_services: HashMap<String, CustomServiceInfo>,
    user_services: HashMap<String, CustomServiceInfo>,
}

#[derive(Serialize, Deserialize, Debug)]
struct CustomServiceInfo {
    content_hash: String,
    installed_at: u64,
}

#[derive(Debug, Clone)]
enum ServiceScope {
    System,
    User,
}

// Users and Groups configuration structures
#[derive(Deserialize, Serialize, Debug)]
struct UsersGroupsConfig {
    users: Option<HashMap<String, UserConfig>>,
    groups: Option<HashMap<String, GroupConfig>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct UserConfig {
    uid: Option<u32>,              // User ID (auto-assign if None)
    gid: Option<u32>,              // Primary group ID
    groups: Option<Vec<String>>,   // Supplementary groups
    home: Option<String>,          // Home directory
    shell: Option<String>,         // Login shell
    comment: Option<String>,       // GECOS field (full name, etc.)
    create_home: Option<bool>,     // Create home directory (default: true)
    system: Option<bool>,          // Is system user (default: false)
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct GroupConfig {
    gid: Option<u32>,              // Group ID (auto-assign if None)
    members: Option<Vec<String>>,  // Group members
    system: Option<bool>,          // Is system group (default: false)
}

#[derive(Debug, Clone)]
struct CurrentUserInfo {
    uid: u32,
    gid: u32,
    groups: Vec<String>,
    home: String,
    shell: String,
    comment: String,
}

#[derive(Debug, Clone)]
struct CurrentGroupInfo {
    gid: u32,
    members: Vec<String>,
}

// State tracking for managed users and groups
#[derive(Serialize, Deserialize, Debug, Default)]
struct UsersGroupsState {
    managed_users: HashMap<String, ManagedUserInfo>,
    managed_groups: HashMap<String, ManagedGroupInfo>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ManagedUserInfo {
    uid: u32,
    managed_at: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ManagedGroupInfo {
    gid: u32,
    managed_at: u64,
}

// Constants for user/group filtering
const MIN_USER_UID: u32 = 1000;
const MIN_GROUP_GID: u32 = 1000;
const MAX_USER_UID: u32 = 60000;
const MAX_GROUP_GID: u32 = 60000;

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

        // Generate pip packages config
        let pip_packages = get_installed_pip_packages().unwrap_or_else(|_| {
            println!("{} pip not available or no packages installed", "[WARN]".yellow());
            Vec::new()
        });
        update_pip_packages_file(&pip_packages)?;

        // Generate npm packages config
        let npm_packages = get_installed_npm_packages().unwrap_or_else(|_| {
            println!("{} npm not available or no packages installed", "[WARN]".yellow());
            Vec::new()
        });
        update_npm_packages_file(&npm_packages)?;

        // Generate cargo packages config
        let cargo_packages = get_installed_cargo_packages().unwrap_or_else(|_| {
            println!("{} cargo not available or no packages installed", "[WARN]".yellow());
            Vec::new()
        });
        update_cargo_packages_file(&cargo_packages)?;

        // Generate services config
        generate_initial_services_configs()?;

        // Generate users and groups config
        generate_initial_users_groups_config()?;

        println!("{} Package, services, and users/groups configuration files generated successfully!", "[SUCCESS]".green());
        println!("Now create your main config/config.toml file and run again without --initial");
        return Ok(());
    }

    println!("🔥 FedoraForge: Forging your perfect Fedora system...");

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
    let _flatpak_packages = sync_flatpak_packages(args.yes, args.no, args.verbose).unwrap_or_else(|e| {
        println!("{} Flatpak synchronization failed: {}", "[WARNING]".yellow(), e);
        Vec::new()
    });

    // Synchronize pip packages with installed packages
    let _pip_packages = sync_pip_packages(args.yes, args.no, args.verbose).unwrap_or_else(|e| {
        println!("{} pip synchronization skipped: {}", "[WARNING]".yellow(), e);
        Vec::new()
    });

    // Synchronize npm packages with installed packages
    let _npm_packages = sync_npm_packages(args.yes, args.no, args.verbose).unwrap_or_else(|e| {
        println!("{} npm synchronization skipped: {}", "[WARNING]".yellow(), e);
        Vec::new()
    });

    // Synchronize cargo packages with installed binaries
    let _cargo_packages = sync_cargo_packages(args.yes, args.no, args.verbose).unwrap_or_else(|e| {
        println!("{} cargo synchronization skipped: {}", "[WARNING]".yellow(), e);
        Vec::new()
    });

    // Synchronize services with system state
    sync_services(args.yes, args.no, args.verbose)?;

    // Synchronize users and groups with system state
    sync_users_and_groups(args.yes, args.no, args.verbose)?;

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
            let managed_output = Command::new("podman").args(&["ps", "-a", "--filter", "label=managed-by=fedoraforge", "--format", "{{.Names}}"]).output()?;
            let managed_containers = std::io::Cursor::new(managed_output.stdout).lines().collect::<Result<Vec<_>, _>>()?;

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

    // WinApps setup
    setup_winapps(config.system.enable_winapps, &args)?;

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

    // Load dotfiles state
    let mut state = load_dotfiles_state()?;

    // Setup .bashrc
    if config.setup_bashrc {
        setup_bashrc(&current_dir, &home_dir, &mut state, yes, no, verbose)?;
    }

    // Setup .config directories
    if config.setup_config_dirs {
        setup_config_dirs(&current_dir, &home_dir, &mut state, yes, no, verbose)?;
    }

    // Save updated state
    save_dotfiles_state(&state)?;

    println!("{} Dotfiles setup completed!", "[SUCCESS]".green());
    Ok(())
}

fn setup_bashrc(project_dir: &Path, home_dir: &Path, state: &mut DotfilesState, yes: bool, no: bool, verbose: bool) -> Result<()> {
    let project_bashrc = project_dir.join(".bashrc");
    let home_bashrc = home_dir.join(".bashrc");

    if !project_bashrc.exists() {
        if verbose {
            println!("{} No .bashrc found in project directory, skipping", "[DEBUG]".cyan());
        }
        return Ok(());
    }

    // Generate hash of project .bashrc
    let project_hash = generate_file_hash(&project_bashrc)?;

    // Check if home .bashrc exists and matches
    if home_bashrc.exists() {
        let home_hash = generate_file_hash(&home_bashrc)?;

        // Compare with stored state
        if let Some(stored_hash) = &state.bashrc_hash {
            if stored_hash == &project_hash && &home_hash == &project_hash {
                if verbose {
                    println!("{} .bashrc is up to date, skipping", "[DEBUG]".cyan());
                }
                return Ok(());
            }
        }

        // Files differ - ask to update
        println!("{} .bashrc has changed since last sync", "[INFO]".blue());
        if ask_user_confirmation("Do you want to update your .bashrc with the version from this project?", yes, no, verbose)? {
            // Backup existing .bashrc
            let backup_path = home_dir.join(".bashrc.backup");
            fs::copy(&home_bashrc, &backup_path)
                .context("Failed to backup existing .bashrc")?;
            println!("{} Backed up existing .bashrc to .bashrc.backup", "[INFO]".blue());

            // Copy project .bashrc
            fs::copy(&project_bashrc, &home_bashrc)
                .context("Failed to copy project .bashrc")?;
            state.bashrc_hash = Some(project_hash);
            println!("{} Successfully updated .bashrc", "[SUCCESS]".green());
        } else {
            println!("{} Skipping .bashrc update", "[INFO]".blue());
        }
    } else {
        println!("{} No existing .bashrc found, copying from project", "[INFO]".blue());
        fs::copy(&project_bashrc, &home_bashrc)
            .context("Failed to copy .bashrc from project")?;
        state.bashrc_hash = Some(project_hash);
        println!("{} Successfully installed .bashrc", "[SUCCESS]".green());
    }

    Ok(())
}

fn setup_config_dirs(project_dir: &Path, home_dir: &Path, state: &mut DotfilesState, yes: bool, no: bool, verbose: bool) -> Result<()> {
    let project_config = project_dir.join(".config");
    let home_config = home_dir.join(".config");

    if !project_config.exists() {
        if verbose {
            println!("{} No .config directory found in project, skipping", "[DEBUG]".cyan());
        }
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
                .context("Non-UTF8 directory name")?
                .to_string();

            let target_dir = home_config.join(&dir_name);

            // Generate hash of project config directory
            let project_hash = generate_directory_hash(&path)?;

            // Check if target dir exists and compare hashes
            if target_dir.exists() {
                let home_hash = generate_directory_hash(&target_dir)?;

                // Compare with stored state
                if let Some(stored_hash) = state.config_dirs.get(&dir_name) {
                    if stored_hash == &project_hash && &home_hash == &project_hash {
                        if verbose {
                            println!("{} {} config is up to date, skipping", "[DEBUG]".cyan(), dir_name);
                        }
                        continue;
                    }
                }

                // Configs differ - ask to update
                println!("{} {} config has changed since last sync", "[INFO]".blue(), dir_name);
                if ask_user_confirmation(&format!("Do you want to update your {} config with the version from this project?", dir_name), yes, no, verbose)? {
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
                    state.config_dirs.insert(dir_name.clone(), project_hash);
                    println!("{} Successfully updated {} config", "[SUCCESS]".green(), dir_name);
                } else {
                    println!("{} Skipping {} config update", "[INFO]".blue(), dir_name);
                }
            } else {
                println!("{} No existing {} config found, copying from project", "[INFO]".blue(), dir_name);
                copy_dir_all(&path, &target_dir)
                    .with_context(|| format!("Failed to copy {} config from project", dir_name))?;
                state.config_dirs.insert(dir_name.clone(), project_hash);
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
    let config_dir = home_dir.join(".config").join("fedoraforge");
    fs::create_dir_all(&config_dir).context("Failed to create config directory")?;
    Ok(config_dir.join("executed_commands.json"))
}

fn get_container_state_file_path() -> Result<std::path::PathBuf> {
    let home_dir = dirs::home_dir().context("Could not find home directory")?;
    let config_dir = home_dir.join(".config").join("fedoraforge");
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

fn get_dotfiles_state_path() -> Result<std::path::PathBuf> {
    let config_dir = dirs::home_dir()
        .context("Failed to get home directory")?
        .join(".config")
        .join("fedoraforge");
    fs::create_dir_all(&config_dir)?;
    Ok(config_dir.join("dotfiles_state.json"))
}

fn load_dotfiles_state() -> Result<DotfilesState> {
    let state_file = get_dotfiles_state_path()?;

    if state_file.exists() {
        let content = fs::read_to_string(&state_file)
            .context("Failed to read dotfiles state file")?;
        let state: DotfilesState = serde_json::from_str(&content)
            .context("Failed to parse dotfiles state file")?;
        Ok(state)
    } else {
        Ok(DotfilesState::default())
    }
}

fn save_dotfiles_state(state: &DotfilesState) -> Result<()> {
    let state_file = get_dotfiles_state_path()?;
    let content = serde_json::to_string_pretty(state)
        .context("Failed to serialize dotfiles state")?;
    fs::write(&state_file, content)
        .context("Failed to write dotfiles state file")?;
    Ok(())
}

fn generate_file_hash(file_path: &Path) -> Result<String> {
    let content = fs::read(file_path)
        .with_context(|| format!("Failed to read file {:?}", file_path))?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    Ok(format!("{:x}", hasher.finalize()))
}

fn generate_directory_hash(dir_path: &Path) -> Result<String> {
    let mut hasher = Sha256::new();

    // Walk directory and hash all files in sorted order for consistency
    let mut files: Vec<_> = Vec::new();
    for entry in walkdir::WalkDir::new(dir_path).sort_by_file_name() {
        let entry = entry?;
        if entry.file_type().is_file() {
            files.push(entry.path().to_path_buf());
        }
    }

    for file in files {
        let content = fs::read(&file)?;
        hasher.update(&content);
    }

    Ok(format!("{:x}", hasher.finalize()))
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

// ========== Pip Package Management ==========

fn get_installed_pip_packages() -> Result<Vec<String>> {
    println!("{} Getting list of installed pip packages...", "[INFO]".blue());

    let output = Command::new("pip")
        .args(&["list", "--format=freeze", "--user"])
        .output()
        .context("pip is not installed or not in PATH")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("pip list failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut packages: Vec<String> = stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            // Parse "package==version" format and extract package name
            line.split("==").next().map(|s| s.to_string())
        })
        .collect();

    packages.sort();
    packages.dedup();

    println!("{} Found {} installed pip packages", "[INFO]".blue(), packages.len());
    Ok(packages)
}

fn install_pip_packages(packages: &[String]) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    println!("{} Installing {} pip packages...", "[INFO]".blue(), packages.len());
    for pkg in packages {
        run_command(&["pip", "install", "--user", pkg], &format!("Installing pip package {}", pkg))?;
    }

    Ok(())
}

fn update_pip_packages_file(packages: &[String]) -> Result<()> {
    let package_list = PackageList {
        packages: packages.to_vec(),
    };

    let content = format!("# Python packages to install via pip\n# List user-installed packages here\n{}",
        toml::to_string_pretty(&package_list)
        .context("Failed to serialize pip package list to TOML")?);

    fs::write("config/pip-packages.toml", content)
        .context("Failed to write pip-packages.toml file")?;

    println!("{} Updated config/pip-packages.toml with {} packages", "[SUCCESS]".green(), packages.len());
    Ok(())
}

fn sync_pip_packages(yes: bool, no: bool, verbose: bool) -> Result<Vec<String>> {
    println!("{} Synchronizing pip packages with installed packages...", "[INFO]".blue());

    // Get currently installed pip packages
    let installed_packages = get_installed_pip_packages()?;

    // Load packages from config file
    let mut config_packages = load_package_list("config/pip-packages.toml")?;

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
            println!("\n{} Pip package '{}' is installed but not in pip-packages.toml", "[INFO]".yellow(), pkg);
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
        println!("{} Installing {} pip packages from config...", "[INFO]".blue(), packages_to_install.len());
        install_pip_packages(&packages_to_install)?;
    }

    // Remove unwanted packages
    if !packages_to_remove.is_empty() {
        println!("{} Removing {} unwanted pip packages...", "[INFO]".blue(), packages_to_remove.len());
        for pkg in &packages_to_remove {
            run_command(&["pip", "uninstall", "-y", pkg], &format!("Removing pip package {}", pkg))?;
        }
    }

    // Update config file if there were changes
    if !packages_to_keep.is_empty() || !packages_to_remove.is_empty() {
        config_packages.sort();
        config_packages.dedup();
        update_pip_packages_file(&config_packages)?;
    }

    println!("{} Pip synchronization completed", "[SUCCESS]".green());
    println!("  - Installed: {} packages", packages_to_install.len());
    println!("  - Kept: {} packages", packages_to_keep.len());
    println!("  - Removed: {} packages", packages_to_remove.len());

    Ok(config_packages)
}

// ========== NPM Package Management ==========

fn get_installed_npm_packages() -> Result<Vec<String>> {
    println!("{} Getting list of globally installed npm packages...", "[INFO]".blue());

    let output = Command::new("npm")
        .args(&["list", "-g", "--depth=0", "--json"])
        .output()
        .context("npm is not installed or not in PATH")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("npm list failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON output
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .context("Failed to parse npm list JSON output")?;

    let mut packages = Vec::new();
    if let Some(deps) = json.get("dependencies").and_then(|d| d.as_object()) {
        for (pkg_name, _) in deps {
            // Skip npm itself
            if pkg_name != "npm" {
                packages.push(pkg_name.clone());
            }
        }
    }

    packages.sort();
    packages.dedup();

    println!("{} Found {} globally installed npm packages", "[INFO]".blue(), packages.len());
    Ok(packages)
}

fn install_npm_packages(packages: &[String]) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    println!("{} Installing {} npm packages globally...", "[INFO]".blue(), packages.len());
    for pkg in packages {
        run_command(&["npm", "install", "-g", pkg], &format!("Installing npm package {}", pkg))?;
    }

    Ok(())
}

fn update_npm_packages_file(packages: &[String]) -> Result<()> {
    let package_list = PackageList {
        packages: packages.to_vec(),
    };

    let content = format!("# Node.js global packages to install via npm\n# List globally installed packages here\n{}",
        toml::to_string_pretty(&package_list)
        .context("Failed to serialize npm package list to TOML")?);

    fs::write("config/npm-packages.toml", content)
        .context("Failed to write npm-packages.toml file")?;

    println!("{} Updated config/npm-packages.toml with {} packages", "[SUCCESS]".green(), packages.len());
    Ok(())
}

fn sync_npm_packages(yes: bool, no: bool, verbose: bool) -> Result<Vec<String>> {
    println!("{} Synchronizing npm packages with installed packages...", "[INFO]".blue());

    // Get currently installed npm packages
    let installed_packages = get_installed_npm_packages()?;

    // Load packages from config file
    let mut config_packages = load_package_list("config/npm-packages.toml")?;

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
            println!("\n{} npm package '{}' is installed but not in npm-packages.toml", "[INFO]".yellow(), pkg);
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
        println!("{} Installing {} npm packages from config...", "[INFO]".blue(), packages_to_install.len());
        install_npm_packages(&packages_to_install)?;
    }

    // Remove unwanted packages
    if !packages_to_remove.is_empty() {
        println!("{} Removing {} unwanted npm packages...", "[INFO]".blue(), packages_to_remove.len());
        for pkg in &packages_to_remove {
            run_command(&["npm", "uninstall", "-g", pkg], &format!("Removing npm package {}", pkg))?;
        }
    }

    // Update config file if there were changes
    if !packages_to_keep.is_empty() || !packages_to_remove.is_empty() {
        config_packages.sort();
        config_packages.dedup();
        update_npm_packages_file(&config_packages)?;
    }

    println!("{} npm synchronization completed", "[SUCCESS]".green());
    println!("  - Installed: {} packages", packages_to_install.len());
    println!("  - Kept: {} packages", packages_to_keep.len());
    println!("  - Removed: {} packages", packages_to_remove.len());

    Ok(config_packages)
}

// ========== Cargo Package Management ==========

fn get_installed_cargo_packages() -> Result<Vec<String>> {
    println!("{} Getting list of cargo-installed binaries...", "[INFO]".blue());

    let output = Command::new("cargo")
        .args(&["install", "--list"])
        .output()
        .context("cargo is not installed or not in PATH")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("cargo install --list failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut packages = Vec::new();

    // Parse cargo install --list output
    // Format: "package_name v0.1.0:"
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(' ') {
            continue;
        }

        // Extract package name (before the version)
        if let Some(pkg_name) = line.split_whitespace().next() {
            packages.push(pkg_name.to_string());
        }
    }

    packages.sort();
    packages.dedup();

    println!("{} Found {} cargo-installed binaries", "[INFO]".blue(), packages.len());
    Ok(packages)
}

fn install_cargo_packages(packages: &[String]) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    println!("{} Installing {} cargo packages...", "[INFO]".blue(), packages.len());
    for pkg in packages {
        run_command(&["cargo", "install", pkg], &format!("Installing cargo package {}", pkg))?;
    }

    Ok(())
}

fn update_cargo_packages_file(packages: &[String]) -> Result<()> {
    let package_list = PackageList {
        packages: packages.to_vec(),
    };

    let content = format!("# Rust binaries to install via cargo\n# List cargo-installed binaries here\n{}",
        toml::to_string_pretty(&package_list)
        .context("Failed to serialize cargo package list to TOML")?);

    fs::write("config/cargo-packages.toml", content)
        .context("Failed to write cargo-packages.toml file")?;

    println!("{} Updated config/cargo-packages.toml with {} packages", "[SUCCESS]".green(), packages.len());
    Ok(())
}

fn sync_cargo_packages(yes: bool, no: bool, verbose: bool) -> Result<Vec<String>> {
    println!("{} Synchronizing cargo packages with installed binaries...", "[INFO]".blue());

    // Get currently installed cargo packages
    let installed_packages = get_installed_cargo_packages()?;

    // Load packages from config file
    let mut config_packages = load_package_list("config/cargo-packages.toml")?;

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
            println!("\n{} Cargo package '{}' is installed but not in cargo-packages.toml", "[INFO]".yellow(), pkg);
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
        println!("{} Installing {} cargo packages from config...", "[INFO]".blue(), packages_to_install.len());
        install_cargo_packages(&packages_to_install)?;
    }

    // Remove unwanted packages
    if !packages_to_remove.is_empty() {
        println!("{} Removing {} unwanted cargo packages...", "[INFO]".blue(), packages_to_remove.len());
        for pkg in &packages_to_remove {
            run_command(&["cargo", "uninstall", pkg], &format!("Removing cargo package {}", pkg))?;
        }
    }

    // Update config file if there were changes
    if !packages_to_keep.is_empty() || !packages_to_remove.is_empty() {
        config_packages.sort();
        config_packages.dedup();
        update_cargo_packages_file(&config_packages)?;
    }

    println!("{} Cargo synchronization completed", "[SUCCESS]".green());
    println!("  - Installed: {} packages", packages_to_install.len());
    println!("  - Kept: {} packages", packages_to_keep.len());
    println!("  - Removed: {} packages", packages_to_remove.len());

    Ok(config_packages)
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

fn setup_winapps(enable_winapps: bool, args: &Args) -> Result<()> {
    if !enable_winapps {
        if args.verbose {
            println!("{} WinApps is disabled, skipping setup", "[DEBUG]".cyan());
        }
        return Ok(());
    }

    println!("{} Setting up WinApps...", "[INFO]".blue());

    // Check if winapps-config.toml exists
    let winapps_config_path = "config/winapps-config.toml";
    if !std::path::Path::new(winapps_config_path).exists() {
        anyhow::bail!("WinApps is enabled but config file not found at {}. Please create the config file or set enable_winapps = false", winapps_config_path);
    }

    // Install WinApps dependencies for Fedora
    println!("{} Installing WinApps dependencies...", "[INFO]".blue());
    run_command(
        &["sudo", "dnf", "install", "-y", "curl", "dialog", "freerdp", "git", "iproute", "libnotify", "nmap-ncat"],
        "Installing WinApps dependencies"
    )?;

    // Load WinApps configuration
    let winapps_config_content = fs::read_to_string(winapps_config_path)
        .with_context(|| format!("Failed to read WinApps config from {}", winapps_config_path))?;

    let winapps_config: WinAppsConfig = toml::from_str(&winapps_config_content)
        .with_context(|| format!("Failed to parse WinApps config from {}", winapps_config_path))?;

    // Verify that backend is set to podman
    if winapps_config.waflavor != "podman" {
        println!("{} WinApps backend is set to '{}', but only 'podman' is supported. Please set waflavor = \"podman\" in {}",
                 "[WARNING]".yellow(), winapps_config.waflavor, winapps_config_path);
        anyhow::bail!("Unsupported WinApps backend: {}. Only 'podman' is supported.", winapps_config.waflavor);
    }

    // Create WinApps config directory
    let home_dir = dirs::home_dir().context("Failed to get home directory")?;
    let winapps_dir = home_dir.join(".config").join("winapps");

    println!("{} Creating WinApps config directory at {:?}", "[INFO]".blue(), winapps_dir);
    fs::create_dir_all(&winapps_dir)
        .with_context(|| format!("Failed to create WinApps config directory at {:?}", winapps_dir))?;

    // Write winapps.conf file
    let winapps_conf_path = winapps_dir.join("winapps.conf");
    println!("{} Writing WinApps configuration to {:?}", "[INFO]".blue(), winapps_conf_path);

    let mut conf_content = String::new();
    conf_content.push_str(&format!("RDP_USER=\"{}\"\n", winapps_config.rdp_user));
    conf_content.push_str(&format!("RDP_PASS=\"{}\"\n", winapps_config.rdp_pass));
    conf_content.push_str(&format!("RDP_DOMAIN=\"{}\"\n", winapps_config.rdp_domain.as_deref().unwrap_or("")));
    conf_content.push_str(&format!("RDP_IP=\"{}\"\n", winapps_config.rdp_ip));
    conf_content.push_str(&format!("VM_NAME=\"{}\"\n", winapps_config.vm_name.as_deref().unwrap_or("RDPWindows")));
    conf_content.push_str(&format!("WAFLAVOR=\"{}\"\n", winapps_config.waflavor));
    conf_content.push_str(&format!("RDP_SCALE=\"{}\"\n", winapps_config.rdp_scale.as_deref().unwrap_or("100")));
    conf_content.push_str(&format!("REMOVABLE_MEDIA=\"{}\"\n", winapps_config.removable_media.as_deref().unwrap_or("/run/media")));
    conf_content.push_str(&format!("DEBUG=\"{}\"\n", if winapps_config.debug.unwrap_or(false) { "true" } else { "false" }));
    conf_content.push_str(&format!("MULTIMON=\"{}\"\n", if winapps_config.multimon.unwrap_or(false) { "true" } else { "false" }));

    if let Some(rdp_flags) = &winapps_config.rdp_flags {
        conf_content.push_str(&format!("RDP_FLAGS=\"{}\"\n", rdp_flags));
    }

    // Detect Wayland and set RDP_ENV for compatibility
    let rdp_env = if let Some(custom_env) = &winapps_config.rdp_env {
        // Use custom environment from config if specified
        custom_env.clone()
    } else {
        // Auto-detect Wayland session
        if let Ok(session_type) = std::env::var("XDG_SESSION_TYPE") {
            if session_type.to_lowercase() == "wayland" {
                println!("{} Detected Wayland session, setting GDK_BACKEND=x11 for FreeRDP compatibility", "[INFO]".blue());
                "GDK_BACKEND=x11".to_string()
            } else {
                String::new()
            }
        } else if std::env::var("WAYLAND_DISPLAY").is_ok() {
            // Fallback check for Wayland
            println!("{} Detected Wayland display, setting GDK_BACKEND=x11 for FreeRDP compatibility", "[INFO]".blue());
            "GDK_BACKEND=x11".to_string()
        } else {
            String::new()
        }
    };

    if !rdp_env.is_empty() {
        conf_content.push_str(&format!("RDP_ENV=\"{}\"\n", rdp_env));
    }

    fs::write(&winapps_conf_path, conf_content)
        .with_context(|| format!("Failed to write WinApps config to {:?}", winapps_conf_path))?;

    // Set secure permissions on config file (600)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&winapps_conf_path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&winapps_conf_path, perms)?;
        println!("{} Set secure permissions (600) on {:?}", "[INFO]".blue(), winapps_conf_path);
    }

    // Clone WinApps repository
    let winapps_repo_dir = home_dir.join(".local").join("share").join("winapps");

    if winapps_repo_dir.exists() {
        println!("{} WinApps repository already exists at {:?}, pulling latest changes...", "[INFO]".blue(), winapps_repo_dir);
        run_command(
            &["git", "-C", winapps_repo_dir.to_str().unwrap(), "pull"],
            "Updating WinApps repository"
        )?;
    } else {
        println!("{} Cloning WinApps repository to {:?}...", "[INFO]".blue(), winapps_repo_dir);
        fs::create_dir_all(winapps_repo_dir.parent().unwrap())?;
        run_command(
            &["git", "clone", "https://github.com/winapps-org/winapps.git", winapps_repo_dir.to_str().unwrap()],
            "Cloning WinApps repository"
        )?;
    }

    // Copy compose.yaml to winapps config directory
    println!("{} Copying compose.yaml to WinApps config directory...", "[INFO]".blue());
    let compose_src = winapps_repo_dir.join("compose.yaml");
    let compose_dest = winapps_dir.join("compose.yaml");

    if compose_src.exists() {
        fs::copy(&compose_src, &compose_dest)
            .with_context(|| format!("Failed to copy compose.yaml from {:?} to {:?}", compose_src, compose_dest))?;
        println!("{} Copied compose.yaml successfully", "[SUCCESS]".green());
    } else {
        println!("{} compose.yaml not found in repository, skipping", "[WARNING]".yellow());
    }

    println!("{} WinApps dependencies and configuration prepared!", "[SUCCESS]".green());

    println!("\n{} ═══════════════════════════════════════════════════════════════", "📋".blue());
    println!("{} WinApps Setup Instructions", "[INFO]".blue().bold());
    println!("{} ═══════════════════════════════════════════════════════════════", "📋".blue());

    println!("\n{} STEP 1: Start the Windows Container", "1️⃣".green());
    println!("     cd {:?}", winapps_dir);
    println!("     podman-compose --file compose.yaml up -d");

    println!("\n{} IMPORTANT: First-time setup takes 15-30 minutes:", "⏱️".yellow());
    println!("  • Windows will download (~4-6 GB)");
    println!("  • Windows will install automatically");
    println!("  • Container will restart once installation completes");

    println!("\n{} Monitor Progress:", "👀".blue());
    println!("  • View logs:     podman logs -f WinApps");
    println!("  • Web console:   http://127.0.0.1:8006");
    println!("  • Check status:  podman ps | grep WinApps");

    println!("\n{} RAM Configuration (in compose.yaml):", "⚙️".yellow());
    println!("  • Default: 4GB RAM (may be too high for some systems)");
    println!("  • If container crashes, edit RAM_SIZE in {:?}", compose_dest);
    println!("  • Recommended: 2GB minimum, 4GB optimal");

    println!("\n{} STEP 2: Run the WinApps Installer (after Windows boots)", "2️⃣".green());
    println!("     bash {:?}", winapps_repo_dir.join("setup.sh"));
    println!("\n  The installer will:");
    println!("  • Install the winapps binary");
    println!("  • Let you select which Windows applications to expose");
    println!("  • Create desktop shortcuts for selected apps");

    if !rdp_env.is_empty() {
        println!("\n{} Wayland Compatibility Configured:", "🖥️".blue());
        println!("  • Detected Wayland session");
        println!("  • Automatically configured: {}", rdp_env);
        println!("  • This fixes FreeRDP X11 compatibility issues");
    }

    println!("\n{} Configuration saved to: {:?}", "✅".green(), winapps_conf_path);
    println!("{} ═══════════════════════════════════════════════════════════════\n", "📋".blue());

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

    // Show summary of actions (only if there are non-Skip actions)
    let has_actions = actions.iter().any(|(_, action)| !matches!(action, ContainerAction::Skip));
    if has_actions && !args.yes {
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
    let mut command = format!("podman run -d --name={} --label managed-by=fedoraforge", container.name);

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
    let mut command = format!("podman create --name={} --label managed-by=fedoraforge", container.name);

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
    quadlet_content.push_str("Label=managed-by=fedoraforge\n");

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

// ========================= SERVICES MANAGEMENT =========================

fn sync_services(yes: bool, no: bool, verbose: bool) -> Result<()> {
    if verbose {
        println!("{} Starting services synchronization", "[DEBUG]".cyan());
    }

    sync_system_services(yes, no, verbose)?;
    sync_user_services(yes, no, verbose)?;

    Ok(())
}

fn sync_system_services(yes: bool, no: bool, verbose: bool) -> Result<()> {
    let config_path = "config/system-services.toml";
    let config = load_system_services_config().unwrap_or_else(|_| SystemServicesConfig {
        services: None,
        custom_services: None,
    });

    let declared = config.services.unwrap_or_default();
    let current = get_current_system_services(verbose)?;

    sync_services_bidirectional(&declared, &current, ServiceScope::System, config_path, yes, no, verbose)?;

    // Handle custom services
    if let Some(custom_services) = config.custom_services {
        sync_custom_services(&custom_services, ServiceScope::System, yes, no, verbose)?;
    }

    Ok(())
}

fn sync_user_services(yes: bool, no: bool, verbose: bool) -> Result<()> {
    let config_path = "config/user-services.toml";
    let config = load_user_services_config().unwrap_or_else(|_| UserServicesConfig {
        services: None,
        custom_services: None,
        applications: None,
    });

    let declared = config.services.unwrap_or_default();
    let current = get_current_user_services(verbose)?;

    sync_services_bidirectional(&declared, &current, ServiceScope::User, config_path, yes, no, verbose)?;

    // Handle custom services
    if let Some(custom_services) = config.custom_services {
        sync_custom_services(&custom_services, ServiceScope::User, yes, no, verbose)?;
    }

    // Handle application autostart
    if let Some(applications) = config.applications {
        sync_application_autostart(&applications, yes, no, verbose)?;
    }

    Ok(())
}

fn sync_services_bidirectional(
    declared: &HashMap<String, ServiceState>,
    current: &HashMap<String, CurrentServiceInfo>,
    scope: ServiceScope,
    config_path: &str,
    yes: bool,
    no: bool,
    verbose: bool,
) -> Result<()> {
    let scope_str = match scope {
        ServiceScope::System => "system",
        ServiceScope::User => "user",
    };

    if verbose {
        println!("{} Syncing {} services bidirectionally", "[DEBUG]".cyan(), scope_str);
    }

    // Find services in system but not in config (add to config)
    let undeclared: Vec<_> = current.iter()
        .filter(|(name, info)| {
            info.exists && !info.is_custom && !declared.contains_key(*name) &&
            // Only include enabled services or services that are currently running
            (info.enabled || info.active)
        })
        .collect();

    if !undeclared.is_empty() {
        println!("{} Found {} {} services not in config:", "[INFO]".blue(), undeclared.len(), scope_str);
        for (name, info) in &undeclared {
            let status = match (info.enabled, info.active) {
                (true, true) => "enabled and running",
                (true, false) => "enabled but not running",
                (false, true) => "disabled but running",
                (false, false) => "disabled and not running",
            };
            println!("  - {} ({})", name, status);
        }

        if ask_user_confirmation(&format!("Add these {} services to config?", scope_str), yes, no, verbose)? {
            update_services_config_with_discovered(&undeclared, config_path, scope.clone())?;
            println!("{} Added {} services to {}", "[SUCCESS]".green(), undeclared.len(), config_path);
        }
    }

    // Find services in config but with different states (apply changes)
    let to_change: Vec<_> = declared.iter()
        .filter_map(|(name, desired)| {
            current.get(name).and_then(|current_info| {
                if !current_info.exists {
                    println!("{} Service '{}' declared in config but not found on system", "[WARN]".yellow(), name);
                    None
                } else if current_info.enabled != desired.enabled || current_info.active != desired.started {
                    Some((name, desired, current_info))
                } else {
                    None
                }
            })
        })
        .collect();

    if !to_change.is_empty() {
        println!("{} Found {} {} services with different states:", "[INFO]".blue(), to_change.len(), scope_str);
        for (name, desired, current) in &to_change {
            println!("  - {}: current(enabled={}, active={}) -> desired(enabled={}, started={})",
                name, current.enabled, current.active, desired.enabled, desired.started);
        }

        if ask_user_confirmation(&format!("Apply these {} service changes?", scope_str), yes, no, verbose)? {
            apply_service_changes(&to_change, scope)?;
        }
    }

    Ok(())
}

fn get_current_system_services(verbose: bool) -> Result<HashMap<String, CurrentServiceInfo>> {
    if verbose {
        println!("{} Discovering system services", "[DEBUG]".cyan());
    }

    let mut services = HashMap::new();

    // Get enabled/disabled state
    let output = run_command_output(&["systemctl", "list-unit-files", "--type=service", "--no-pager", "--plain"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        if line.trim().is_empty() || line.starts_with("UNIT FILE") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let name = parts[0].trim_end_matches(".service");
            let state = parts[1];

            // Skip non-manageable service states
            if matches!(state, "static" | "transient" | "generated" | "indirect" | "alias" | "masked" | "enabled-runtime") {
                continue;
            }

            // Skip runtime dbus services and generated autostart services
            if name.starts_with("dbus-:") || (name.starts_with("app-") && name.contains("@autostart")) {
                continue;
            }

            // Skip system services that are auto-managed (keep consistent with user services filtering)
            if name == "uresourced" {
                if verbose {
                    println!("{} Skipping auto-managed system service: {}", "[DEBUG]".cyan(), name);
                }
                continue;
            }

            let enabled = state == "enabled";
            services.insert(name.to_string(), CurrentServiceInfo {
                enabled,
                active: false, // Will be updated below
                exists: true,
                is_custom: false, // Will be updated if we find it's custom
            });
        }
    }

    // Get active/inactive state
    let output = run_command_output(&["systemctl", "list-units", "--type=service", "--no-pager", "--plain"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        if line.trim().is_empty() || line.starts_with("UNIT") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let name = parts[0].trim_end_matches(".service");
            let active = parts[2] == "active";
            if let Some(service) = services.get_mut(name) {
                service.active = active;
            }
        }
    }

    // Mark custom services
    let custom_state = load_custom_services_state().unwrap_or_default();
    for service_name in custom_state.system_services.keys() {
        if let Some(service) = services.get_mut(service_name) {
            service.is_custom = true;
        }
    }

    if verbose {
        println!("{} Found {} system services", "[DEBUG]".cyan(), services.len());
    }

    Ok(services)
}

fn get_current_user_services(verbose: bool) -> Result<HashMap<String, CurrentServiceInfo>> {
    if verbose {
        println!("{} Discovering user services", "[DEBUG]".cyan());
    }

    let mut services = HashMap::new();

    // Load container state to exclude Quadlet-managed containers
    let container_state = load_container_state().unwrap_or_default();
    let managed_containers: Vec<String> = container_state.containers.keys().cloned().collect();

    // Get enabled/disabled state
    let output = run_command_output(&["systemctl", "--user", "list-unit-files", "--type=service", "--no-pager", "--plain"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        if line.trim().is_empty() || line.starts_with("UNIT FILE") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let name = parts[0].trim_end_matches(".service");
            let state = parts[1];

            // Skip non-manageable service states
            if matches!(state, "static" | "transient" | "generated" | "indirect" | "alias" | "masked" | "enabled-runtime") {
                continue;
            }

            // Skip runtime dbus services and generated autostart services
            if name.starts_with("dbus-:") || (name.starts_with("app-") && name.contains("@autostart")) {
                continue;
            }

            // Skip container services managed by Quadlet
            if managed_containers.contains(&name.to_string()) {
                if verbose {
                    println!("{} Skipping Quadlet-managed container service: {}", "[DEBUG]".cyan(), name);
                }
                continue;
            }

            // Skip desktop session services that shouldn't be managed
            if matches!(name,
                "pipewire" | "pipewire-pulse" |
                "dconf" | "uresourced" | "podman-user-wait-network-online" |
                "at-spi-dbus-bus"
            ) || name.starts_with("gvfs-")
              || name.starts_with("evolution-")
              || (name.starts_with("xdg-") && name != "xdg-user-dirs") {
                if verbose {
                    println!("{} Skipping desktop session service: {}", "[DEBUG]".cyan(), name);
                }
                continue;
            }

            let enabled = state == "enabled";
            services.insert(name.to_string(), CurrentServiceInfo {
                enabled,
                active: false, // Will be updated below
                exists: true,
                is_custom: false, // Will be updated if we find it's custom
            });
        }
    }

    // Get active/inactive state
    let output = run_command_output(&["systemctl", "--user", "list-units", "--type=service", "--no-pager", "--plain"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        if line.trim().is_empty() || line.starts_with("UNIT") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let name = parts[0].trim_end_matches(".service");
            let active = parts[2] == "active";
            if let Some(service) = services.get_mut(name) {
                service.active = active;
            }
        }
    }

    // Mark custom services
    let custom_state = load_custom_services_state().unwrap_or_default();
    for service_name in custom_state.user_services.keys() {
        if let Some(service) = services.get_mut(service_name) {
            service.is_custom = true;
        }
    }

    if verbose {
        println!("{} Found {} user services", "[DEBUG]".cyan(), services.len());
    }

    Ok(services)
}

fn apply_service_changes(
    changes: &[(&String, &ServiceState, &CurrentServiceInfo)],
    scope: ServiceScope,
) -> Result<()> {
    for (name, desired, current) in changes {
        // Handle enabled state
        if current.enabled != desired.enabled {
            if desired.enabled {
                enable_service(name, &scope)?;
            } else {
                disable_service(name, &scope)?;
            }
        }

        // Handle started state
        if current.active != desired.started {
            if desired.started {
                start_service(name, &scope)?;
            } else {
                stop_service(name, &scope)?;
            }
        }
    }
    Ok(())
}

fn enable_service(name: &str, scope: &ServiceScope) -> Result<()> {
    match scope {
        ServiceScope::System => {
            run_command(&["sudo", "systemctl", "enable", name], &format!("Enabling system service {}", name))?;
        }
        ServiceScope::User => {
            run_command(&["systemctl", "--user", "enable", name], &format!("Enabling user service {}", name))?;
        }
    }
    Ok(())
}

fn disable_service(name: &str, scope: &ServiceScope) -> Result<()> {
    match scope {
        ServiceScope::System => {
            run_command(&["sudo", "systemctl", "disable", name], &format!("Disabling system service {}", name))?;
        }
        ServiceScope::User => {
            run_command(&["systemctl", "--user", "disable", name], &format!("Disabling user service {}", name))?;
        }
    }
    Ok(())
}

fn start_service(name: &str, scope: &ServiceScope) -> Result<()> {
    match scope {
        ServiceScope::System => {
            run_command(&["sudo", "systemctl", "start", name], &format!("Starting system service {}", name))?;
        }
        ServiceScope::User => {
            run_command(&["systemctl", "--user", "start", name], &format!("Starting user service {}", name))?;
        }
    }
    Ok(())
}

fn stop_service(name: &str, scope: &ServiceScope) -> Result<()> {
    match scope {
        ServiceScope::System => {
            run_command(&["sudo", "systemctl", "stop", name], &format!("Stopping system service {}", name))?;
        }
        ServiceScope::User => {
            run_command(&["systemctl", "--user", "stop", name], &format!("Stopping user service {}", name))?;
        }
    }
    Ok(())
}

// ========================= CUSTOM SERVICES MANAGEMENT =========================

fn sync_custom_services(
    custom_services: &[CustomService],
    scope: ServiceScope,
    yes: bool,
    no: bool,
    verbose: bool,
) -> Result<()> {
    let mut state = load_custom_services_state()?;
    let scope_str = match scope {
        ServiceScope::System => "system",
        ServiceScope::User => "user",
    };

    if verbose {
        println!("{} Syncing {} custom services", "[DEBUG]".cyan(), scope_str);
    }

    for service in custom_services {
        let service_hash = generate_service_hash(&service.service_definition, &service.timer_definition)?;
        let state_map = match scope {
            ServiceScope::System => &mut state.system_services,
            ServiceScope::User => &mut state.user_services,
        };

        let needs_install = match state_map.get(&service.name) {
            Some(info) => info.content_hash != service_hash,
            None => true,
        };

        if needs_install {
            if ask_user_confirmation(&format!("Install/update custom {} service '{}'?", scope_str, service.name), yes, no, verbose)? {
                install_custom_service(service, &service_hash, &scope, state_map)?;
            }
        }

        // Sync enabled/started state
        sync_custom_service_state(service, &scope, verbose)?;
    }

    // Remove orphaned custom services
    cleanup_orphaned_custom_services(custom_services, &mut state, &scope, yes, no, verbose)?;

    save_custom_services_state(&state)?;
    Ok(())
}

fn install_custom_service(
    service: &CustomService,
    content_hash: &str,
    scope: &ServiceScope,
    state_map: &mut HashMap<String, CustomServiceInfo>,
) -> Result<()> {
    let service_dir = match scope {
        ServiceScope::System => "/etc/systemd/system".to_string(),
        ServiceScope::User => {
            let home = dirs::home_dir().context("Could not find home directory")?;
            let user_dir = home.join(".config/systemd/user");
            fs::create_dir_all(&user_dir)?;
            user_dir.to_str().unwrap().to_string()
        }
    };

    // Write service file
    let service_file = format!("{}/{}.service", service_dir, service.name);

    match scope {
        ServiceScope::System => {
            // For system services, write to temp file first then sudo move it
            let temp_file = format!("/tmp/{}.service", service.name);
            fs::write(&temp_file, &service.service_definition)?;
            run_command(&["sudo", "mv", &temp_file, &service_file], &format!("Installing system service {}", service.name))?;
        }
        ServiceScope::User => {
            fs::write(&service_file, &service.service_definition)?;
        }
    }

    // Write timer file if present
    if let Some(timer_def) = &service.timer_definition {
        let timer_file = format!("{}/{}.timer", service_dir, service.name);

        match scope {
            ServiceScope::System => {
                let temp_file = format!("/tmp/{}.timer", service.name);
                fs::write(&temp_file, timer_def)?;
                run_command(&["sudo", "mv", &temp_file, &timer_file], &format!("Installing system timer {}", service.name))?;
            }
            ServiceScope::User => {
                fs::write(&timer_file, timer_def)?;
            }
        }
    }

    // Reload systemd
    match scope {
        ServiceScope::System => run_command(&["sudo", "systemctl", "daemon-reload"], "Reloading system daemon")?,
        ServiceScope::User => run_command(&["systemctl", "--user", "daemon-reload"], "Reloading user daemon")?,
    }

    // Update state
    state_map.insert(service.name.clone(), CustomServiceInfo {
        content_hash: content_hash.to_string(),
        installed_at: get_current_timestamp(),
    });

    println!("{} Installed custom service: {}", "[SUCCESS]".green(), service.name);
    Ok(())
}

fn sync_custom_service_state(service: &CustomService, scope: &ServiceScope, _verbose: bool) -> Result<()> {
    // Check current service state
    let is_enabled = check_service_enabled(&service.name, scope)?;
    let is_active = check_service_active(&service.name, scope)?;

    // Apply desired enabled state
    if is_enabled != service.enabled {
        if service.enabled {
            enable_service(&service.name, scope)?;
        } else {
            disable_service(&service.name, scope)?;
        }
    }

    // Apply desired started state
    if is_active != service.started {
        if service.started {
            start_service(&service.name, scope)?;
        } else {
            stop_service(&service.name, scope)?;
        }
    }

    Ok(())
}

fn check_service_enabled(name: &str, scope: &ServiceScope) -> Result<bool> {
    let output = match scope {
        ServiceScope::System => run_command_output(&["systemctl", "is-enabled", name]),
        ServiceScope::User => run_command_output(&["systemctl", "--user", "is-enabled", name]),
    };

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(stdout.trim() == "enabled")
        }
        Err(_) => Ok(false), // Service doesn't exist or is disabled
    }
}

fn check_service_active(name: &str, scope: &ServiceScope) -> Result<bool> {
    let output = match scope {
        ServiceScope::System => run_command_output(&["systemctl", "is-active", name]),
        ServiceScope::User => run_command_output(&["systemctl", "--user", "is-active", name]),
    };

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(stdout.trim() == "active")
        }
        Err(_) => Ok(false), // Service doesn't exist or is inactive
    }
}

fn cleanup_orphaned_custom_services(
    current_services: &[CustomService],
    state: &mut CustomServicesState,
    scope: &ServiceScope,
    yes: bool,
    no: bool,
    verbose: bool,
) -> Result<()> {
    let state_map = match scope {
        ServiceScope::System => &mut state.system_services,
        ServiceScope::User => &mut state.user_services,
    };

    let current_names: std::collections::HashSet<_> = current_services.iter().map(|s| &s.name).collect();
    let orphaned: Vec<_> = state_map.keys().filter(|name| !current_names.contains(name)).cloned().collect();

    if !orphaned.is_empty() {
        let scope_str = match scope {
            ServiceScope::System => "system",
            ServiceScope::User => "user",
        };

        println!("{} Found {} orphaned custom {} services:", "[INFO]".blue(), orphaned.len(), scope_str);
        for name in &orphaned {
            println!("  - {}", name);
        }

        if ask_user_confirmation(&format!("Remove orphaned custom {} services?", scope_str), yes, no, verbose)? {
            for name in &orphaned {
                remove_custom_service(name, scope)?;
                state_map.remove(name);
            }
            println!("{} Removed {} orphaned services", "[SUCCESS]".green(), orphaned.len());
        }
    }

    Ok(())
}

fn remove_custom_service(name: &str, scope: &ServiceScope) -> Result<()> {
    let service_dir = match scope {
        ServiceScope::System => "/etc/systemd/system".to_string(),
        ServiceScope::User => {
            let home = dirs::home_dir().context("Could not find home directory")?;
            home.join(".config/systemd/user").to_string_lossy().to_string()
        }
    };

    let service_file = format!("{}/{}.service", service_dir, name);
    let timer_file = format!("{}/{}.timer", service_dir, name);

    // Stop and disable service first
    let _ = match scope {
        ServiceScope::System => {
            let _ = run_command(&["sudo", "systemctl", "stop", name], &format!("Stopping service {}", name));
            run_command(&["sudo", "systemctl", "disable", name], &format!("Disabling service {}", name))
        }
        ServiceScope::User => {
            let _ = run_command(&["systemctl", "--user", "stop", name], &format!("Stopping service {}", name));
            run_command(&["systemctl", "--user", "disable", name], &format!("Disabling service {}", name))
        }
    };

    // Remove files
    match scope {
        ServiceScope::System => {
            let _ = run_command(&["sudo", "rm", "-f", &service_file], &format!("Removing service file {}", service_file));
            let _ = run_command(&["sudo", "rm", "-f", &timer_file], &format!("Removing timer file {}", timer_file));
        }
        ServiceScope::User => {
            let _ = fs::remove_file(&service_file);
            let _ = fs::remove_file(&timer_file);
        }
    }

    // Reload systemd
    match scope {
        ServiceScope::System => run_command(&["sudo", "systemctl", "daemon-reload"], "Reloading system daemon")?,
        ServiceScope::User => run_command(&["systemctl", "--user", "daemon-reload"], "Reloading user daemon")?,
    }

    Ok(())
}

fn generate_service_hash(service_def: &str, timer_def: &Option<String>) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(service_def.trim().as_bytes());
    if let Some(timer) = timer_def {
        hasher.update(timer.trim().as_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

// ========================= APPLICATION AUTOSTART =========================

fn sync_application_autostart(
    applications: &HashMap<String, ApplicationAutostart>,
    yes: bool,
    no: bool,
    verbose: bool,
) -> Result<()> {
    if verbose {
        println!("{} Syncing application autostart", "[DEBUG]".cyan());
    }

    // Convert applications to custom services
    let mut app_services = Vec::new();
    for (app_name, app_config) in applications {
        if let Some(service) = generate_application_service(app_name, app_config)? {
            app_services.push(service);
        }
    }

    if !app_services.is_empty() {
        sync_custom_services(&app_services, ServiceScope::User, yes, no, verbose)?;
    }

    Ok(())
}

fn generate_application_service(app_name: &str, config: &ApplicationAutostart) -> Result<Option<CustomService>> {
    if !config.enabled {
        return Ok(None);
    }

    // Try to find the application executable
    let executable = find_application_executable(app_name)?;

    // Build command with arguments
    let mut exec_start = executable.clone();
    if let Some(args) = &config.args {
        for arg in args {
            exec_start.push(' ');
            exec_start.push_str(arg);
        }
    }

    // Build environment variables section
    let mut environment_section = String::new();
    if let Some(env_vars) = &config.environment {
        for (key, value) in env_vars {
            environment_section.push_str(&format!("Environment={}={}\n", key, value));
        }
    }

    // Set restart policy
    let restart_policy = config.restart_policy.as_deref().unwrap_or("never");

    // Build delay configuration
    let delay_section = if let Some(delay) = config.delay {
        format!("ExecStartPre=/bin/sleep {}\n", delay)
    } else {
        String::new()
    };

    let service_definition = format!(
        r#"[Unit]
Description={} Autostart
After=graphical-session.target
Wants=graphical-session.target

[Service]
Type=simple
{}ExecStart={}
Restart={}
{}Environment=DISPLAY=:0
Environment=WAYLAND_DISPLAY=wayland-0

[Install]
WantedBy=default.target"#,
        app_name,
        delay_section,
        exec_start,
        restart_policy,
        environment_section
    );

    Ok(Some(CustomService {
        name: format!("{}-autostart", app_name),
        enabled: true,
        started: true,
        service_definition,
        timer_definition: None,
    }))
}

fn find_application_executable(app_name: &str) -> Result<String> {
    // First try common application paths
    let common_paths = [
        format!("/usr/bin/{}", app_name),
        format!("/usr/local/bin/{}", app_name),
        format!("/bin/{}", app_name),
        format!("/opt/{}/bin/{}", app_name, app_name),
    ];

    for path in &common_paths {
        if std::path::Path::new(path).exists() {
            return Ok(path.clone());
        }
    }

    // Try using 'which' command
    let output = run_command_output(&["which", app_name]);
    if let Ok(output) = output {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() && std::path::Path::new(&path).exists() {
            return Ok(path);
        }
    }

    // Try desktop file lookup for Flatpak and other applications
    let desktop_file = format!("{}.desktop", app_name);
    let desktop_paths = [
        format!("/usr/share/applications/{}", desktop_file),
        format!("/var/lib/flatpak/exports/share/applications/{}", desktop_file),
        format!("{}/.local/share/applications/{}",
                dirs::home_dir().unwrap_or_default().to_string_lossy(), desktop_file),
    ];

    for desktop_path in &desktop_paths {
        if std::path::Path::new(desktop_path).exists() {
            if let Ok(exec_line) = extract_exec_from_desktop_file(desktop_path) {
                return Ok(exec_line);
            }
        }
    }

    // Fallback: assume it's in PATH
    Ok(app_name.to_string())
}

fn extract_exec_from_desktop_file(desktop_file: &str) -> Result<String> {
    let content = fs::read_to_string(desktop_file)?;
    for line in content.lines() {
        if line.starts_with("Exec=") {
            let exec_line = line.strip_prefix("Exec=").unwrap_or(line);
            // Remove %u, %f and other desktop file placeholders
            let cleaned = exec_line
                .replace("%u", "")
                .replace("%f", "")
                .replace("%F", "")
                .replace("%U", "")
                .trim()
                .to_string();
            return Ok(cleaned);
        }
    }
    anyhow::bail!("No Exec line found in desktop file: {}", desktop_file)
}

// ========================= CONFIG LOADING AND SAVING =========================

fn load_system_services_config() -> Result<SystemServicesConfig> {
    let config_content = fs::read_to_string("config/system-services.toml")
        .with_context(|| "Failed to read config/system-services.toml")?;
    let config: SystemServicesConfig = toml::from_str(&config_content)
        .with_context(|| "Failed to parse system-services.toml")?;
    Ok(config)
}

fn load_user_services_config() -> Result<UserServicesConfig> {
    let config_content = fs::read_to_string("config/user-services.toml")
        .with_context(|| "Failed to read config/user-services.toml")?;
    let config: UserServicesConfig = toml::from_str(&config_content)
        .with_context(|| "Failed to parse user-services.toml")?;
    Ok(config)
}

fn load_custom_services_state() -> Result<CustomServicesState> {
    let home_dir = dirs::home_dir().context("Could not find home directory")?;
    let state_dir = home_dir.join(".config").join("repro-setup");
    fs::create_dir_all(&state_dir)?;
    let state_file = state_dir.join("custom_services.json");

    if state_file.exists() {
        let content = fs::read_to_string(&state_file)?;
        let state: CustomServicesState = serde_json::from_str(&content)?;
        Ok(state)
    } else {
        Ok(CustomServicesState::default())
    }
}

fn save_custom_services_state(state: &CustomServicesState) -> Result<()> {
    let home_dir = dirs::home_dir().context("Could not find home directory")?;
    let state_dir = home_dir.join(".config").join("repro-setup");
    fs::create_dir_all(&state_dir)?;
    let state_file = state_dir.join("custom_services.json");

    let content = serde_json::to_string_pretty(state)?;
    fs::write(&state_file, content)?;
    Ok(())
}

fn update_services_config_with_discovered(
    discovered: &[(&String, &CurrentServiceInfo)],
    config_path: &str,
    scope: ServiceScope,
) -> Result<()> {
    // Create new services map from discovered services
    let mut new_services = HashMap::new();
    for (name, info) in discovered {
        new_services.insert((*name).clone(), ServiceState {
            enabled: info.enabled,
            started: info.active,
        });
    }

    // Try to read existing config
    let existing_content = fs::read_to_string(config_path).unwrap_or_else(|_| String::new());

    if existing_content.trim().is_empty() {
        // Create new config file
        let config_content = match scope {
            ServiceScope::System => format!(
                "# System services configuration\n[services]\n{}",
                new_services.iter()
                    .map(|(name, state)| format!(
                        r#""{}" = {{ enabled = {}, started = {} }}"#,
                        name, state.enabled, state.started
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            ServiceScope::User => format!(
                "# User services configuration\n[services]\n{}",
                new_services.iter()
                    .map(|(name, state)| format!(
                        r#""{}" = {{ enabled = {}, started = {} }}"#,
                        name, state.enabled, state.started
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        };
        fs::write(config_path, config_content)?;
    } else {
        // Append to existing config
        let mut content = existing_content;
        if !content.ends_with('\n') {
            content.push('\n');
        }

        for (name, state) in new_services {
            content.push_str(&format!(
                r#""{}" = {{ enabled = {}, started = {} }}"#,
                name, state.enabled, state.started
            ));
            content.push('\n');
        }

        fs::write(config_path, content)?;
    }

    Ok(())
}

// ========================= INITIAL SETUP SUPPORT =========================

fn generate_initial_services_configs() -> Result<()> {
    println!("{} Generating services configuration from current state...", "[INFO]".blue());

    // Create config directory if it doesn't exist
    fs::create_dir_all("config")?;

    // Generate system services config
    let system_services = get_current_system_services(false)?;
    let system_enabled: Vec<_> = system_services.iter()
        .filter(|(_, info)| info.enabled && !info.is_custom)
        .collect();

    if !system_enabled.is_empty() {
        update_services_config_with_discovered(&system_enabled, "config/system-services.toml", ServiceScope::System)?;
        println!("{} Generated config/system-services.toml with {} services", "[SUCCESS]".green(), system_enabled.len());
    }

    // Generate user services config
    let user_services = get_current_user_services(false)?;
    let user_enabled: Vec<_> = user_services.iter()
        .filter(|(_, info)| info.enabled && !info.is_custom)
        .collect();

    if !user_enabled.is_empty() {
        update_services_config_with_discovered(&user_enabled, "config/user-services.toml", ServiceScope::User)?;
        println!("{} Generated config/user-services.toml with {} services", "[SUCCESS]".green(), user_enabled.len());
    }

    Ok(())
}

// ============================================================================
// Users and Groups Management Functions
// ============================================================================

// Validation functions
fn validate_username(username: &str) -> Result<()> {
    // Username regex: must start with lowercase letter or underscore
    // Can contain lowercase letters, numbers, underscores, and hyphens
    // May end with a dollar sign (for machine accounts)
    use regex::Regex;
    let re = Regex::new(r"^[a-z_][a-z0-9_-]*[$]?$").context("Failed to compile username regex")?;

    if !re.is_match(username) {
        anyhow::bail!("Invalid username '{}': must start with a lowercase letter or underscore, and contain only lowercase letters, numbers, underscores, hyphens, and optionally end with $", username);
    }

    if username.len() > 32 {
        anyhow::bail!("Username '{}' is too long (max 32 characters)", username);
    }

    Ok(())
}

fn validate_groupname(groupname: &str) -> Result<()> {
    // Same rules as username
    validate_username(groupname).context(format!("Invalid groupname '{}'", groupname))
}

fn validate_uid(uid: u32) -> Result<()> {
    if uid < MIN_USER_UID {
        anyhow::bail!("UID {} is below minimum {} (system UID range)", uid, MIN_USER_UID);
    }
    if uid > MAX_USER_UID {
        anyhow::bail!("UID {} exceeds maximum {}", uid, MAX_USER_UID);
    }
    Ok(())
}

fn validate_gid(gid: u32) -> Result<()> {
    if gid < MIN_GROUP_GID {
        anyhow::bail!("GID {} is below minimum {} (system GID range)", gid, MIN_GROUP_GID);
    }
    if gid > MAX_GROUP_GID {
        anyhow::bail!("GID {} exceeds maximum {}", gid, MAX_GROUP_GID);
    }
    Ok(())
}

fn validate_shell(shell: &str) -> Result<()> {
    // Check if shell exists in /etc/shells
    let shells_content = fs::read_to_string("/etc/shells")
        .context("Failed to read /etc/shells")?;

    let valid_shells: Vec<&str> = shells_content
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
        .map(|line| line.trim())
        .collect();

    if !valid_shells.contains(&shell) {
        anyhow::bail!("Shell '{}' is not listed in /etc/shells. Valid shells: {:?}", shell, valid_shells);
    }

    Ok(())
}

// Backup function
fn backup_user_files(verbose: bool) -> Result<()> {
    if verbose {
        println!("{} Backing up user/group files", "[DEBUG]".cyan());
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let backup_dir = "/etc";
    let files_to_backup = vec![
        ("passwd", format!("{}/passwd.fedoraforge.{}.backup", backup_dir, timestamp)),
        ("group", format!("{}/group.fedoraforge.{}.backup", backup_dir, timestamp)),
        ("shadow", format!("{}/shadow.fedoraforge.{}.backup", backup_dir, timestamp)),
    ];

    for (file, backup) in files_to_backup {
        let source = format!("{}/{}", backup_dir, file);
        if Path::new(&source).exists() {
            run_command(&["sudo", "cp", "-p", &source, &backup],
                &format!("Backing up {}", file))?;
            if verbose {
                println!("{} Backed up {} to {}", "[DEBUG]".cyan(), source, backup);
            }
        }
    }

    println!("{} User/group files backed up successfully", "[SUCCESS]".green());
    Ok(())
}

// Discovery functions
fn get_current_users(verbose: bool) -> Result<HashMap<String, CurrentUserInfo>> {
    if verbose {
        println!("{} Discovering users (UID >= {})", "[DEBUG]".cyan(), MIN_USER_UID);
    }

    let passwd_content = fs::read_to_string("/etc/passwd")
        .context("Failed to read /etc/passwd")?;

    let mut users = HashMap::new();

    for line in passwd_content.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 7 {
            continue;
        }

        let username = parts[0];
        let uid: u32 = parts[2].parse().unwrap_or(0);
        let gid: u32 = parts[3].parse().unwrap_or(0);
        let comment = parts[4];
        let home = parts[5];
        let shell = parts[6];

        // Filter system users (UID < 1000) and special accounts
        if uid < MIN_USER_UID {
            continue;
        }

        if uid > MAX_USER_UID {
            continue;
        }

        // Get supplementary groups using id command
        let groups = get_user_supplementary_groups(username)?;

        users.insert(username.to_string(), CurrentUserInfo {
            uid,
            gid,
            groups,
            home: home.to_string(),
            shell: shell.to_string(),
            comment: comment.to_string(),
        });
    }

    if verbose {
        println!("{} Discovered {} non-system users", "[DEBUG]".cyan(), users.len());
    }

    Ok(users)
}

fn get_user_supplementary_groups(username: &str) -> Result<Vec<String>> {
    let output = Command::new("id")
        .args(&["-nG", username])
        .output()
        .context(format!("Failed to get groups for user {}", username))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let groups_str = String::from_utf8(output.stdout)?;
    let groups: Vec<String> = groups_str
        .split_whitespace()
        .skip(1) // Skip primary group (first in list)
        .map(|s| s.to_string())
        .collect();

    Ok(groups)
}

fn get_current_groups(verbose: bool) -> Result<HashMap<String, CurrentGroupInfo>> {
    if verbose {
        println!("{} Discovering groups (GID >= {})", "[DEBUG]".cyan(), MIN_GROUP_GID);
    }

    let group_content = fs::read_to_string("/etc/group")
        .context("Failed to read /etc/group")?;

    let mut groups = HashMap::new();

    for line in group_content.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 4 {
            continue;
        }

        let groupname = parts[0];
        let gid: u32 = parts[2].parse().unwrap_or(0);
        let members_str = parts[3];

        // Filter system groups (GID < 1000)
        if gid < MIN_GROUP_GID {
            continue;
        }

        if gid > MAX_GROUP_GID {
            continue;
        }

        let members: Vec<String> = if members_str.is_empty() {
            Vec::new()
        } else {
            members_str.split(',').map(|s| s.to_string()).collect()
        };

        groups.insert(groupname.to_string(), CurrentGroupInfo {
            gid,
            members,
        });
    }

    if verbose {
        println!("{} Discovered {} non-system groups", "[DEBUG]".cyan(), groups.len());
    }

    Ok(groups)
}

// Config management functions
fn load_users_groups_config() -> Result<UsersGroupsConfig> {
    let config_path = "config/users-groups.toml";

    if !Path::new(config_path).exists() {
        return Ok(UsersGroupsConfig {
            users: None,
            groups: None,
        });
    }

    let content = fs::read_to_string(config_path)
        .context(format!("Failed to read {}", config_path))?;

    let config: UsersGroupsConfig = toml::from_str(&content)
        .context("Failed to parse users-groups TOML file")?;

    Ok(config)
}

// State management functions
fn load_users_groups_state() -> Result<UsersGroupsState> {
    let state_dir = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".config/fedoraforge");

    let state_file = state_dir.join("users_groups_state.json");

    if !state_file.exists() {
        return Ok(UsersGroupsState::default());
    }

    let content = fs::read_to_string(&state_file)
        .context("Failed to read users/groups state file")?;

    let state: UsersGroupsState = serde_json::from_str(&content)
        .context("Failed to parse users/groups state JSON")?;

    Ok(state)
}

fn save_users_groups_state(state: &UsersGroupsState) -> Result<()> {
    let state_dir = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".config/fedoraforge");

    fs::create_dir_all(&state_dir)
        .context("Failed to create state directory")?;

    let state_file = state_dir.join("users_groups_state.json");

    let json = serde_json::to_string_pretty(state)
        .context("Failed to serialize users/groups state")?;

    fs::write(&state_file, json)
        .context("Failed to write users/groups state file")?;

    Ok(())
}

fn update_users_groups_config_with_discovered(
    discovered_users: &HashMap<String, CurrentUserInfo>,
    discovered_groups: &HashMap<String, CurrentGroupInfo>,
    config_path: &str,
) -> Result<()> {
    // Build TOML content manually for better formatting
    let mut toml_content = String::from("# Users and Groups Configuration\n\n");

    // Add users section
    if !discovered_users.is_empty() {
        toml_content.push_str("[users]\n");
        for (username, info) in discovered_users {
            toml_content.push_str(&format!("[users.\"{}\"]\n", username));
            toml_content.push_str(&format!("uid = {}\n", info.uid));
            toml_content.push_str(&format!("gid = {}\n", info.gid));

            if !info.groups.is_empty() {
                toml_content.push_str("groups = [");
                toml_content.push_str(&info.groups.iter()
                    .map(|g| format!("\"{}\"", g))
                    .collect::<Vec<_>>()
                    .join(", "));
                toml_content.push_str("]\n");
            }

            toml_content.push_str(&format!("home = \"{}\"\n", info.home));
            toml_content.push_str(&format!("shell = \"{}\"\n", info.shell));

            if !info.comment.is_empty() {
                toml_content.push_str(&format!("comment = \"{}\"\n", info.comment));
            }

            toml_content.push_str("create_home = true\n");
            toml_content.push_str("system = false\n\n");
        }
    }

    // Add groups section
    if !discovered_groups.is_empty() {
        toml_content.push_str("[groups]\n");
        for (groupname, info) in discovered_groups {
            toml_content.push_str(&format!("[groups.\"{}\"]\n", groupname));
            toml_content.push_str(&format!("gid = {}\n", info.gid));

            if !info.members.is_empty() {
                toml_content.push_str("members = [");
                toml_content.push_str(&info.members.iter()
                    .map(|m| format!("\"{}\"", m))
                    .collect::<Vec<_>>()
                    .join(", "));
                toml_content.push_str("]\n");
            }

            toml_content.push_str("system = false\n\n");
        }
    }

    fs::write(config_path, toml_content)
        .context(format!("Failed to write {}", config_path))?;

    Ok(())
}

// Group management functions
fn create_group(groupname: &str, config: &GroupConfig, verbose: bool) -> Result<()> {
    validate_groupname(groupname)?;

    if let Some(gid) = config.gid {
        validate_gid(gid)?;
    }

    let mut cmd_args = vec!["sudo", "groupadd"];
    let gid_str;

    if let Some(gid) = config.gid {
        cmd_args.push("-g");
        gid_str = gid.to_string();
        cmd_args.push(&gid_str);
    }

    if config.system.unwrap_or(false) {
        cmd_args.push("--system");
    }

    cmd_args.push(groupname);

    run_command(&cmd_args, &format!("Creating group {}", groupname))?;

    // Add members if specified
    if let Some(members) = &config.members {
        for member in members {
            let result = run_command(
                &["sudo", "gpasswd", "-a", member, groupname],
                &format!("Adding {} to group {}", member, groupname)
            );
            if result.is_err() && verbose {
                println!("{} Failed to add user {} to group {}: user may not exist yet",
                    "[WARN]".yellow(), member, groupname);
            }
        }
    }

    Ok(())
}

fn modify_group(groupname: &str, current: &CurrentGroupInfo, desired: &GroupConfig, verbose: bool) -> Result<()> {
    // Check if GID needs to change
    if let Some(desired_gid) = desired.gid {
        if desired_gid != current.gid {
            validate_gid(desired_gid)?;
            run_command(
                &["sudo", "groupmod", "-g", &desired_gid.to_string(), groupname],
                &format!("Changing GID for group {}", groupname)
            )?;
        }
    }

    // Handle member changes
    if let Some(desired_members) = &desired.members {
        let current_members: std::collections::HashSet<_> = current.members.iter().collect();
        let desired_members_set: std::collections::HashSet<_> = desired_members.iter().collect();

        // Add missing members
        for member in desired_members_set.difference(&current_members) {
            let result = run_command(
                &["sudo", "gpasswd", "-a", member, groupname],
                &format!("Adding {} to group {}", member, groupname)
            );
            if result.is_err() && verbose {
                println!("{} Failed to add user {} to group {}: user may not exist",
                    "[WARN]".yellow(), member, groupname);
            }
        }

        // Remove extra members
        for member in current_members.difference(&desired_members_set) {
            run_command(
                &["sudo", "gpasswd", "-d", member, groupname],
                &format!("Removing {} from group {}", member, groupname)
            )?;
        }
    }

    Ok(())
}

fn delete_group(groupname: &str) -> Result<()> {
    validate_groupname(groupname)?;
    run_command(
        &["sudo", "groupdel", groupname],
        &format!("Deleting group {}", groupname)
    )?;
    Ok(())
}

// User management functions
fn create_user(username: &str, config: &UserConfig, verbose: bool) -> Result<()> {
    validate_username(username)?;

    if let Some(uid) = config.uid {
        validate_uid(uid)?;
    }

    let mut cmd_args = vec!["sudo", "useradd"];
    let uid_str;
    let gid_str;

    if let Some(uid) = config.uid {
        cmd_args.push("-u");
        uid_str = uid.to_string();
        cmd_args.push(&uid_str);
    }

    if let Some(gid) = config.gid {
        validate_gid(gid)?;
        cmd_args.push("-g");
        gid_str = gid.to_string();
        cmd_args.push(&gid_str);
    }

    if let Some(home) = &config.home {
        cmd_args.push("-d");
        cmd_args.push(home);
    }

    if let Some(shell) = &config.shell {
        validate_shell(shell)?;
        cmd_args.push("-s");
        cmd_args.push(shell);
    }

    if let Some(comment) = &config.comment {
        cmd_args.push("-c");
        cmd_args.push(comment);
    }

    // Create home directory by default unless explicitly disabled
    if config.create_home.unwrap_or(true) {
        cmd_args.push("-m");
    } else {
        cmd_args.push("-M");
    }

    if config.system.unwrap_or(false) {
        cmd_args.push("--system");
    }

    cmd_args.push(username);

    run_command(&cmd_args, &format!("Creating user {}", username))?;

    // Add to supplementary groups
    if let Some(groups) = &config.groups {
        if !groups.is_empty() {
            let groups_str = groups.join(",");
            let result = run_command(
                &["sudo", "usermod", "-aG", &groups_str, username],
                &format!("Adding {} to groups: {}", username, groups_str)
            );
            if result.is_err() && verbose {
                println!("{} Failed to add user to some groups: groups may not exist yet",
                    "[WARN]".yellow());
            }
        }
    }

    Ok(())
}

fn modify_user(username: &str, current: &CurrentUserInfo, desired: &UserConfig, verbose: bool) -> Result<()> {
    // Check UID change
    if let Some(desired_uid) = desired.uid {
        if desired_uid != current.uid {
            validate_uid(desired_uid)?;
            run_command(
                &["sudo", "usermod", "-u", &desired_uid.to_string(), username],
                &format!("Changing UID for user {}", username)
            )?;
        }
    }

    // Check GID change
    if let Some(desired_gid) = desired.gid {
        if desired_gid != current.gid {
            validate_gid(desired_gid)?;
            run_command(
                &["sudo", "usermod", "-g", &desired_gid.to_string(), username],
                &format!("Changing primary GID for user {}", username)
            )?;
        }
    }

    // Check home directory change
    if let Some(desired_home) = &desired.home {
        if desired_home != &current.home {
            run_command(
                &["sudo", "usermod", "-d", desired_home, username],
                &format!("Changing home directory for user {}", username)
            )?;
        }
    }

    // Check shell change
    if let Some(desired_shell) = &desired.shell {
        if desired_shell != &current.shell {
            validate_shell(desired_shell)?;
            run_command(
                &["sudo", "usermod", "-s", desired_shell, username],
                &format!("Changing shell for user {}", username)
            )?;
        }
    }

    // Check comment change
    if let Some(desired_comment) = &desired.comment {
        if desired_comment != &current.comment {
            run_command(
                &["sudo", "usermod", "-c", desired_comment, username],
                &format!("Changing comment for user {}", username)
            )?;
        }
    }

    // Handle supplementary groups
    if let Some(desired_groups) = &desired.groups {
        let current_groups_set: std::collections::HashSet<_> = current.groups.iter().collect();
        let desired_groups_set: std::collections::HashSet<_> = desired_groups.iter().collect();

        if current_groups_set != desired_groups_set {
            // Set groups using -G flag (replaces all supplementary groups)
            let groups_str = desired_groups.join(",");
            let result = run_command(
                &["sudo", "usermod", "-G", &groups_str, username],
                &format!("Updating groups for user {}", username)
            );
            if result.is_err() && verbose {
                println!("{} Failed to update groups: some groups may not exist",
                    "[WARN]".yellow());
            }
        }
    }

    Ok(())
}

fn delete_user(username: &str, remove_home: bool, verbose: bool) -> Result<()> {
    validate_username(username)?;

    let mut cmd_args = vec!["sudo", "userdel"];

    if remove_home {
        cmd_args.push("-r");
        if verbose {
            println!("{} Will remove home directory for user {}", "[INFO]".yellow(), username);
        }
    }

    cmd_args.push(username);

    run_command(&cmd_args, &format!("Deleting user {}", username))?;
    Ok(())
}

// Bidirectional sync functions
fn sync_groups_bidirectional(
    declared: &HashMap<String, GroupConfig>,
    current: &HashMap<String, CurrentGroupInfo>,
    state: &mut UsersGroupsState,
    yes: bool,
    no: bool,
    verbose: bool,
) -> Result<()> {
    if verbose {
        println!("{} Syncing groups bidirectionally", "[DEBUG]".cyan());
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    // Find groups in system but not in config (add to config or delete)
    let undeclared_groups: HashMap<String, CurrentGroupInfo> = current.iter()
        .filter(|(name, _)| !declared.contains_key(*name) && !state.managed_groups.contains_key(*name))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if !undeclared_groups.is_empty() {
        println!("{} Found {} groups not in config:", "[INFO]".blue(), undeclared_groups.len());
        for (name, info) in &undeclared_groups {
            println!("  - {} (GID: {}, members: {})", name, info.gid,
                if info.members.is_empty() { "none".to_string() } else { info.members.join(", ") });
        }

        println!("What would you like to do with these groups?");
        println!("  1. Add to config (manage them)");
        println!("  2. Delete from system");
        println!("  3. Ignore (leave as-is)");

        if yes {
            // Auto-yes means add to config
            update_users_groups_config_with_discovered(&HashMap::new(), &undeclared_groups, "config/users-groups.toml")?;
            println!("{} Added {} groups to config/users-groups.toml", "[SUCCESS]".green(), undeclared_groups.len());
        } else if !no {
            print!("Enter choice [1-3]: ");
            io::stdout().flush()?;
            let mut choice = String::new();
            io::stdin().read_line(&mut choice)?;

            match choice.trim() {
                "1" => {
                    update_users_groups_config_with_discovered(&HashMap::new(), &undeclared_groups, "config/users-groups.toml")?;
                    println!("{} Added {} groups to config/users-groups.toml", "[SUCCESS]".green(), undeclared_groups.len());
                }
                "2" => {
                    if ask_user_confirmation("Are you sure you want to delete these groups?", false, false, verbose)? {
                        for (name, _) in &undeclared_groups {
                            delete_group(name)?;
                            println!("{} Deleted group {}", "[SUCCESS]".green(), name);
                        }
                    }
                }
                "3" | "" => {
                    println!("{} Ignoring undeclared groups", "[INFO]".blue());
                }
                _ => {
                    println!("{} Invalid choice, ignoring", "[WARN]".yellow());
                }
            }
        }
    }

    // Find groups in config but not in system (create them)
    let groups_to_create: Vec<_> = declared.iter()
        .filter(|(name, _)| !current.contains_key(*name))
        .collect();

    if !groups_to_create.is_empty() {
        println!("{} Found {} groups in config that don't exist:", "[INFO]".blue(), groups_to_create.len());
        for (name, _) in &groups_to_create {
            println!("  - {}", name);
        }

        if ask_user_confirmation("Create these groups?", yes, no, verbose)? {
            for (name, config) in groups_to_create {
                create_group(name, config, verbose)?;
                // Get the created group's GID
                let created_info = get_current_groups(false)?;
                if let Some(info) = created_info.get(name) {
                    state.managed_groups.insert(name.clone(), ManagedGroupInfo {
                        gid: info.gid,
                        managed_at: timestamp,
                    });
                }
                println!("{} Created group {}", "[SUCCESS]".green(), name);
            }
        }
    }

    // Find groups with different states (update them)
    let groups_to_modify: Vec<_> = declared.iter()
        .filter_map(|(name, desired)| {
            current.get(name).and_then(|current_info| {
                // Check if GID differs
                let gid_differs = desired.gid.map_or(false, |gid| gid != current_info.gid);

                // Check if members differ
                let desired_members = desired.members.as_ref().map(|m| m.iter().collect::<std::collections::HashSet<_>>());
                let current_members = current_info.members.iter().collect::<std::collections::HashSet<_>>();
                let members_differ = desired_members.map_or(false, |dm| dm != current_members);

                if gid_differs || members_differ {
                    Some((name, desired, current_info))
                } else {
                    None
                }
            })
        })
        .collect();

    if !groups_to_modify.is_empty() {
        println!("{} Found {} groups with different states:", "[INFO]".blue(), groups_to_modify.len());
        for (name, desired, current) in &groups_to_modify {
            println!("  - {}: current(GID={}, members=[{}]) -> desired(GID={}, members=[{}])",
                name,
                current.gid,
                current.members.join(", "),
                desired.gid.map_or_else(|| current.gid.to_string(), |g| g.to_string()),
                desired.members.as_ref().map_or_else(|| "".to_string(), |m| m.join(", "))
            );
        }

        if ask_user_confirmation("Apply these group changes?", yes, no, verbose)? {
            for (name, desired, current) in groups_to_modify {
                modify_group(name, current, desired, verbose)?;
                // Update state with new GID if changed
                let new_gid = desired.gid.unwrap_or(current.gid);
                state.managed_groups.insert(name.clone(), ManagedGroupInfo {
                    gid: new_gid,
                    managed_at: timestamp,
                });
                println!("{} Modified group {}", "[SUCCESS]".green(), name);
            }
        }
    }

    // Update state for all declared groups
    for (name, _config) in declared {
        if let Some(info) = current.get(name) {
            state.managed_groups.entry(name.clone()).or_insert(ManagedGroupInfo {
                gid: info.gid,
                managed_at: timestamp,
            });
        }
    }

    Ok(())
}

fn sync_users_bidirectional(
    declared: &HashMap<String, UserConfig>,
    current: &HashMap<String, CurrentUserInfo>,
    state: &mut UsersGroupsState,
    yes: bool,
    no: bool,
    verbose: bool,
) -> Result<()> {
    if verbose {
        println!("{} Syncing users bidirectionally", "[DEBUG]".cyan());
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    // Find users in system but not in config (add to config or delete)
    let undeclared_users: HashMap<String, CurrentUserInfo> = current.iter()
        .filter(|(name, _)| !declared.contains_key(*name) && !state.managed_users.contains_key(*name))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if !undeclared_users.is_empty() {
        println!("{} Found {} users not in config:", "[INFO]".blue(), undeclared_users.len());
        for (name, info) in &undeclared_users {
            println!("  - {} (UID: {}, shell: {}, home: {})", name, info.uid, info.shell, info.home);
        }

        println!("What would you like to do with these users?");
        println!("  1. Add to config (manage them)");
        println!("  2. Delete from system");
        println!("  3. Ignore (leave as-is)");

        if yes {
            // Auto-yes means add to config
            update_users_groups_config_with_discovered(&undeclared_users, &HashMap::new(), "config/users-groups.toml")?;
            println!("{} Added {} users to config/users-groups.toml", "[SUCCESS]".green(), undeclared_users.len());
        } else if !no {
            print!("Enter choice [1-3]: ");
            io::stdout().flush()?;
            let mut choice = String::new();
            io::stdin().read_line(&mut choice)?;

            match choice.trim() {
                "1" => {
                    update_users_groups_config_with_discovered(&undeclared_users, &HashMap::new(), "config/users-groups.toml")?;
                    println!("{} Added {} users to config/users-groups.toml", "[SUCCESS]".green(), undeclared_users.len());
                }
                "2" => {
                    if ask_user_confirmation("Are you sure you want to delete these users?", false, false, verbose)? {
                        for (name, _) in &undeclared_users {
                            let remove_home = ask_user_confirmation(
                                &format!("Remove home directory for user '{}'?", name),
                                false, false, verbose
                            )?;
                            delete_user(name, remove_home, verbose)?;
                            println!("{} Deleted user {}", "[SUCCESS]".green(), name);
                        }
                    }
                }
                "3" | "" => {
                    println!("{} Ignoring undeclared users", "[INFO]".blue());
                }
                _ => {
                    println!("{} Invalid choice, ignoring", "[WARN]".yellow());
                }
            }
        }
    }

    // Find users in config but not in system (create them)
    let users_to_create: Vec<_> = declared.iter()
        .filter(|(name, _)| !current.contains_key(*name))
        .collect();

    if !users_to_create.is_empty() {
        println!("{} Found {} users in config that don't exist:", "[INFO]".blue(), users_to_create.len());
        for (name, _) in &users_to_create {
            println!("  - {}", name);
        }

        if ask_user_confirmation("Create these users?", yes, no, verbose)? {
            for (name, config) in users_to_create {
                create_user(name, config, verbose)?;
                // Get the created user's UID
                let created_info = get_current_users(false)?;
                if let Some(info) = created_info.get(name) {
                    state.managed_users.insert(name.clone(), ManagedUserInfo {
                        uid: info.uid,
                        managed_at: timestamp,
                    });
                }
                println!("{} Created user {}", "[SUCCESS]".green(), name);
            }
        }
    }

    // Find users with different states (update them)
    let users_to_modify: Vec<_> = declared.iter()
        .filter_map(|(name, desired)| {
            current.get(name).and_then(|current_info| {
                // Check for differences
                let uid_differs = desired.uid.map_or(false, |uid| uid != current_info.uid);
                let gid_differs = desired.gid.map_or(false, |gid| gid != current_info.gid);
                let home_differs = desired.home.as_ref().map_or(false, |h| h != &current_info.home);
                let shell_differs = desired.shell.as_ref().map_or(false, |s| s != &current_info.shell);
                let comment_differs = desired.comment.as_ref().map_or(false, |c| c != &current_info.comment);

                let desired_groups_set = desired.groups.as_ref().map(|g| g.iter().collect::<std::collections::HashSet<_>>());
                let current_groups_set = current_info.groups.iter().collect::<std::collections::HashSet<_>>();
                let groups_differ = desired_groups_set.map_or(false, |dg| dg != current_groups_set);

                if uid_differs || gid_differs || home_differs || shell_differs || comment_differs || groups_differ {
                    Some((name, desired, current_info))
                } else {
                    None
                }
            })
        })
        .collect();

    if !users_to_modify.is_empty() {
        println!("{} Found {} users with different states:", "[INFO]".blue(), users_to_modify.len());
        for (name, desired, current) in &users_to_modify {
            println!("  - {}: UID {} -> {}, shell {} -> {}, groups [{}] -> [{}]",
                name,
                current.uid,
                desired.uid.map_or_else(|| current.uid.to_string(), |u| u.to_string()),
                current.shell,
                desired.shell.as_ref().unwrap_or(&current.shell),
                current.groups.join(", "),
                desired.groups.as_ref().map_or_else(|| "".to_string(), |g| g.join(", "))
            );
        }

        if ask_user_confirmation("Apply these user changes?", yes, no, verbose)? {
            for (name, desired, current) in users_to_modify {
                modify_user(name, current, desired, verbose)?;
                // Update state with new UID if changed
                let new_uid = desired.uid.unwrap_or(current.uid);
                state.managed_users.insert(name.clone(), ManagedUserInfo {
                    uid: new_uid,
                    managed_at: timestamp,
                });
                println!("{} Modified user {}", "[SUCCESS]".green(), name);
            }
        }
    }

    // Update state for all declared users
    for (name, _config) in declared {
        if let Some(info) = current.get(name) {
            state.managed_users.entry(name.clone()).or_insert(ManagedUserInfo {
                uid: info.uid,
                managed_at: timestamp,
            });
        }
    }

    Ok(())
}

// Main sync function
fn sync_users_and_groups(yes: bool, no: bool, verbose: bool) -> Result<()> {
    println!("{} Synchronizing users and groups with system state...", "[INFO]".blue());

    // Backup files before making changes
    backup_user_files(verbose)?;

    // Load config and state
    let config = load_users_groups_config()?;
    let mut state = load_users_groups_state()?;

    // Get current system state
    let current_users = get_current_users(verbose)?;
    let current_groups = get_current_groups(verbose)?;

    // Check for orphaned groups (previously managed but removed from config)
    let declared_group_names: std::collections::HashSet<_> = config.groups
        .as_ref()
        .map(|g| g.keys().collect())
        .unwrap_or_default();

    let orphaned_groups: Vec<_> = state.managed_groups.keys()
        .filter(|name| !declared_group_names.contains(name) && current_groups.contains_key(*name))
        .cloned()
        .collect();

    if !orphaned_groups.is_empty() {
        println!("{} Found {} groups removed from config but still exist in system:", "[INFO]".blue(), orphaned_groups.len());
        for group in &orphaned_groups {
            println!("  - {}", group);
        }

        if ask_user_confirmation("Delete these groups from the system?", yes, no, verbose)? {
            for group in &orphaned_groups {
                delete_group(group)?;
                state.managed_groups.remove(group);
                println!("{} Deleted group {}", "[SUCCESS]".green(), group);
            }
        }
    }

    // Check for orphaned users (previously managed but removed from config)
    let declared_user_names: std::collections::HashSet<_> = config.users
        .as_ref()
        .map(|u| u.keys().collect())
        .unwrap_or_default();

    let orphaned_users: Vec<_> = state.managed_users.keys()
        .filter(|name| !declared_user_names.contains(name) && current_users.contains_key(*name))
        .cloned()
        .collect();

    if !orphaned_users.is_empty() {
        println!("{} Found {} users removed from config but still exist in system:", "[INFO]".blue(), orphaned_users.len());
        for user in &orphaned_users {
            println!("  - {}", user);
        }

        if ask_user_confirmation("Delete these users from the system?", yes, no, verbose)? {
            for user in &orphaned_users {
                if ask_user_confirmation(&format!("Remove home directory for user '{}'?", user), false, false, verbose)? {
                    delete_user(user, true, verbose)?;
                } else {
                    delete_user(user, false, verbose)?;
                }
                state.managed_users.remove(user);
                println!("{} Deleted user {}", "[SUCCESS]".green(), user);
            }
        }
    }

    // Sync groups first (users may depend on groups)
    if let Some(declared_groups) = &config.groups {
        sync_groups_bidirectional(declared_groups, &current_groups, &mut state, yes, no, verbose)?;
    } else if verbose {
        println!("{} No groups declared in config", "[DEBUG]".cyan());
    }

    // Then sync users
    if let Some(declared_users) = &config.users {
        sync_users_bidirectional(declared_users, &current_users, &mut state, yes, no, verbose)?;
    } else if verbose {
        println!("{} No users declared in config", "[DEBUG]".cyan());
    }

    // Save updated state
    save_users_groups_state(&state)?;

    println!("{} Users and groups synchronization complete", "[SUCCESS]".green());
    Ok(())
}

// Initial config generation
fn generate_initial_users_groups_config() -> Result<()> {
    println!("{} Generating users and groups configuration from current system state...", "[INFO]".blue());

    let current_users = get_current_users(false)?;
    let current_groups = get_current_groups(false)?;

    if current_users.is_empty() && current_groups.is_empty() {
        println!("{} No non-system users or groups found to add to config", "[WARN]".yellow());
        return Ok(());
    }

    update_users_groups_config_with_discovered(&current_users, &current_groups, "config/users-groups.toml")?;

    println!("{} Generated config/users-groups.toml with {} users and {} groups",
        "[SUCCESS]".green(), current_users.len(), current_groups.len());

    Ok(())
}

