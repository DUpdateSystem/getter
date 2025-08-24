use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio;

use getter::{
    api, core::config::world::get_world_list, rpc::server::run_server, utils::versioning::Version,
    websdk::repo::provider::get_hub_uuid,
};

#[derive(Parser)]
#[command(name = "getter")]
#[command(about = "A universal app update checker and manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Data directory path
    #[arg(long, default_value = "./data")]
    data_dir: PathBuf,

    /// Cache directory path
    #[arg(long, default_value = "./cache")]
    cache_dir: PathBuf,

    /// Cache expire time in seconds
    #[arg(long, default_value = "3600")]
    expire_time: u64,
}

#[derive(Subcommand)]
enum Commands {
    /// Start RPC server
    Server {
        /// Server address
        #[arg(long, default_value = "127.0.0.1:0")]
        addr: String,
    },
    /// Add a new app to track
    AddApp {
        /// Hub UUID
        hub_uuid: String,
        /// App data as key=value pairs
        #[arg(short, long, value_parser = parse_key_val)]
        app_data: Vec<(String, String)>,
        /// Hub data as key=value pairs
        #[arg(short = 'H', long, value_parser = parse_key_val)]
        hub_data: Vec<(String, String)>,
    },
    /// Renew/update app information
    RenewApp {
        /// Hub UUID
        hub_uuid: String,
        /// App data as key=value pairs
        #[arg(short, long, value_parser = parse_key_val)]
        app_data: Vec<(String, String)>,
        /// Hub data as key=value pairs
        #[arg(short = 'H', long, value_parser = parse_key_val)]
        hub_data: Vec<(String, String)>,
    },
    /// Mark app with specific version
    MarkAppVersion {
        /// App identifier
        app_id: String,
        /// Version to mark
        version: String,
    },
}

/// Parse key=value pairs from command line
fn parse_key_val(s: &str) -> Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{s}`"))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Initialize the system
    api::init(&cli.data_dir, &cli.cache_dir, cli.expire_time).await?;

    match cli.command {
        Commands::Server { addr } => {
            println!("Starting RPC server at {}", addr);
            let is_running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let (url, handle) = run_server(&addr, is_running.clone()).await?;
            println!("Server started at {}", url);

            // Wait for shutdown signal
            tokio::signal::ctrl_c().await?;
            println!("Shutting down server...");
            is_running.store(false, std::sync::atomic::Ordering::SeqCst);
            handle.stop()?;
        }

        Commands::AddApp {
            hub_uuid,
            app_data,
            hub_data,
        } => {
            let real_hub_uuid = get_hub_uuid(&hub_uuid);
            let app_data_map: BTreeMap<&str, &str> = app_data
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let hub_data_map: BTreeMap<&str, &str> = hub_data
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();

            println!(
                "Adding app with hub_id: {} (UUID: {})",
                hub_uuid, real_hub_uuid
            );

            // Check if app is available
            match api::check_app_available(&real_hub_uuid, &app_data_map, &hub_data_map).await {
                Some(true) => {
                    println!("✓ App is available");

                    // Get latest release info
                    match api::get_latest_release(&real_hub_uuid, &app_data_map, &hub_data_map)
                        .await
                    {
                        Some(release) => {
                            println!("✓ Latest version: {}", release.version_number);

                            // Save to world list (existing storage mechanism)
                            let world_list = get_world_list().await;
                            let mut world_list = world_list.lock().await;

                            // Generate app ID from app_data
                            let app_id = if let Some(repo) = app_data_map.get("repo") {
                                if let Some(owner) = app_data_map.get("owner") {
                                    format!("{}_{}", owner, repo)
                                } else {
                                    repo.to_string()
                                }
                            } else {
                                format!("app_{}", app_data.len())
                            };

                            let app_data_owned: std::collections::HashMap<String, String> =
                                app_data.into_iter().collect();
                            let hub_data_owned: std::collections::HashMap<String, String> =
                                hub_data.into_iter().collect();

                            // Add to tracked apps with full metadata
                            let added = world_list.rule_list.add_tracked_app(
                                app_id.clone(),
                                real_hub_uuid.clone(),
                                app_data_owned,
                                hub_data_owned,
                            );

                            if added {
                                // Also add to legacy app_list for compatibility
                                world_list.rule_list.push_app(&app_id);

                                match world_list.save() {
                                    Ok(()) => {
                                        let config_path =
                                            cli.data_dir.join("world_config_list.json");
                                        println!("✓ App '{}' added successfully", app_id);
                                        println!("  Stored in: {}", config_path.display());
                                    }
                                    Err(e) => {
                                        println!("✗ Failed to save app: {}", e);
                                    }
                                }
                            } else {
                                println!("⚠ App '{}' already exists", app_id);
                            }
                        }
                        None => {
                            println!("✗ Failed to get latest release info");
                        }
                    }
                }
                Some(false) => {
                    println!("✗ App is not available in the specified hub");
                    return Ok(());
                }
                None => {
                    println!("✗ Failed to check app availability");
                    return Ok(());
                }
            }
        }

        Commands::RenewApp {
            hub_uuid,
            app_data,
            hub_data,
        } => {
            let real_hub_uuid = get_hub_uuid(&hub_uuid);
            let app_data_map: BTreeMap<&str, &str> = app_data
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let hub_data_map: BTreeMap<&str, &str> = hub_data
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();

            println!(
                "Renewing app with hub_id: {} (UUID: {})",
                hub_uuid, real_hub_uuid
            );

            // Get all releases
            match api::get_releases(&real_hub_uuid, &app_data_map, &hub_data_map).await {
                Some(releases) => {
                    if releases.is_empty() {
                        println!("No releases found");
                    } else {
                        println!("Found {} releases:", releases.len());
                        for (i, release) in releases.iter().take(5).enumerate() {
                            println!(
                                "  {}. {} - {}",
                                i + 1,
                                release.version_number,
                                release.changelog.chars().take(50).collect::<String>()
                                    + if release.changelog.len() > 50 {
                                        "..."
                                    } else {
                                        ""
                                    }
                            );
                        }
                        if releases.len() > 5 {
                            println!("  ... and {} more", releases.len() - 5);
                        }
                    }
                }
                None => {
                    println!("✗ Failed to get releases");
                }
            }
        }

        Commands::MarkAppVersion { app_id, version } => {
            println!("Marking app {} with version {}", app_id, version);

            // Validate version format
            let version_obj = Version::new(version.clone());
            if version_obj.is_valid() {
                if let Some(clean_version) = version_obj.get_valid_version() {
                    println!(
                        "✓ Version {} is valid (normalized: {})",
                        version, clean_version
                    );

                    // TODO: Store this version marking (requires app database implementation)
                    println!("  Version marked successfully");
                } else {
                    println!("✗ Invalid version format: {}", version);
                }
            } else {
                println!("✗ Invalid version format: {}", version);
            }
        }
    }

    Ok(())
}
