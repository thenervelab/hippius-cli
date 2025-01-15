use clap::{Parser, Subcommand, ValueEnum};
use std::process::Command;
use subxt::{OnlineClient, PolkadotConfig};
use dotenv::dotenv;
use std::env;
use subxt::tx::PairSigner;
use sp_core::{Pair, sr25519};

#[subxt::subxt(runtime_metadata_path = "metadata.scale")]
pub mod custom_runtime {}

/// A CLI for interacting with the Hippius Docker Registry and Substrate Chain
#[derive(Parser)]
#[command(name = "hippius-cli")]
struct Cli {
    /// The subcommand to run (e.g., "docker" or "create")
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Docker command interface
    Docker {
        /// The Docker subcommand (e.g., "push")
        #[arg()]
        docker_command: String,

        /// Arguments for the Docker command (e.g., "repo1/image2:latest")
        #[arg()]
        args: Vec<String>,
    },
    /// Create a new Docker space in Substrate
    Create {
        /// The type of entity to create (must be "docker")
        #[arg(value_enum)]
        entity_type: EntityType,

        /// The name of the space to create
        #[arg()]
        name: String,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum EntityType {
    Docker,
}

#[tokio::main]
async fn main() {
    dotenv().ok();
    let cli = Cli::parse();

    match cli.command {
        Commands::Docker { docker_command, args } => {
            handle_docker_command(docker_command, args);
        }
        Commands::Create { entity_type, name } => {
            match entity_type {
                EntityType::Docker => {
                    if let Err(e) = handle_create_docker_space(name).await {
                        eprintln!("Error creating Docker space: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        }
    }
}

fn handle_docker_command(docker_command: String, args: Vec<String>) {
    // Default URL prefix for your registry
    let registry_url = "localhost:3000";

    // Transform arguments, adding the registry URL for specific commands like "push" or "pull"
    let transformed_args: Vec<String> = args
        .into_iter()
        .map(|arg| {
            if arg.contains(':') && (docker_command == "push" || docker_command == "pull") {
                format!("{}/{}", registry_url, arg)
            } else {
                arg
            }
        })
        .collect();

    // Execute the transformed Docker command
    let output = Command::new("docker")
        .arg(docker_command)
        .args(transformed_args)
        .output();

    match output {
        Ok(output) => {
            if !output.stdout.is_empty() {
                println!("{}", String::from_utf8_lossy(&output.stdout));
            }
            if !output.stderr.is_empty() {
                eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(error) => {
            eprintln!("Failed to execute docker command: {}", error);
        }
    }
}

async fn handle_create_docker_space(name: String) -> Result<(), Box<dyn std::error::Error>> {
    let url = env::var("SUBSTRATE_NODE_URL")
    .unwrap_or_else(|_| "ws://127.0.0.1:9944".to_string());

    // Try different configurations
    let api = OnlineClient::<PolkadotConfig>::from_url(&url).await?;

    // Load the seed phrase from the environment
    let seed_phrase = env::var("SUBSTRATE_SEED_PHRASE")
        .unwrap_or_else(|_| "//".to_string());

    let pair = sr25519::Pair::from_string(seed_phrase.as_str(), None)
        .map_err(|e| format!("Failed to create pair: {:?}", e))?;

    // Create a PairSigner using the sp_core pair
    let signer = PairSigner::new(pair);

    // Call the extrinsic to create a space
    let tx = custom_runtime::tx().container_registry().create_space(name.into_bytes()); // Example extrinsic, replace with the actual one

    api
        .tx()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await?
        .wait_for_finalized_success()
        .await?;

    println!("successfully created space!");  

    Ok(())
}