use clap::{Parser, Subcommand};
use getter_rpc::{types::*, GetterRpcClient};

#[derive(Parser)]
#[command(name = "getter")]
#[command(about = "A CLI for managing application updates via RPC")]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    server: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Add a new app
    Add {
        /// App data as JSON
        data: String,
    },
    /// Remove an app
    Remove {
        /// App ID
        id: String,
    },
    /// Update an app
    Update {
        /// App ID
        id: String,
    },
    /// Get app status
    Status {
        /// App ID
        id: String,
    },
    /// List all apps
    List,
    /// Check if app update is available
    Check {
        /// App ID
        id: String,
    },
    /// Get latest release info
    Release {
        /// App ID
        id: String,
    },
    /// Get outdated apps
    Outdated,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let client = GetterRpcClient::new_http(&cli.server);

    match cli.command {
        Commands::Add { data } => {
            let app_data: serde_json::Value = serde_json::from_str(&data)?;
            client.add_app(app_data).await?;
            println!("App added successfully");
        }
        Commands::Remove { id } => {
            client.remove_app(id).await?;
            println!("App removed successfully");
        }
        Commands::Update { id } => {
            client.update_app(id).await?;
            println!("App updated successfully");
        }
        Commands::Status { id } => {
            if let Some(status) = client.get_app_status(id).await? {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("App not found");
            }
        }
        Commands::List => {
            let apps = client.list_apps().await?;
            for app in apps {
                println!("{}", app);
            }
        }
        Commands::Check { id } => {
            let available = client.check_app_available(id).await?;
            println!("Update available: {}", available);
        }
        Commands::Release { id } => {
            if let Some(release) = client.get_latest_release(id).await? {
                println!("{}", serde_json::to_string_pretty(&release)?);
            } else {
                println!("No release found");
            }
        }
        Commands::Outdated => {
            let apps = client.get_outdated_apps().await?;
            for app in apps {
                println!("{}", app);
            }
        }
    }

    Ok(())
}
