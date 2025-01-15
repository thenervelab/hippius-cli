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

    println!("🐳 Executing Docker command: {}", docker_command);
    println!("📦 Arguments: {}", args.join(" "));

    // Transform arguments, adding the registry URL for specific commands like "push" or "pull"
    let transformed_args: Vec<String> = args
        .into_iter()
        .map(|arg| {
            if arg.contains(':') && (docker_command == "push" || docker_command == "pull") {
                let modified_arg = format!("{}/{}", registry_url, arg);
                println!("🌐 Modifying image path to: {}", modified_arg);
                modified_arg
            } else {
                arg
            }
        })
        .collect();

    // Execute the transformed Docker command
    println!("🚀 Running docker {} {}...", docker_command, transformed_args.join(" "));
    let output = Command::new("docker")
        .arg(docker_command.clone())
        .args(transformed_args)
        .output();

    match output {
        Ok(output) => {
            if !output.stdout.is_empty() {
                println!("📝 Command Output:");
                println!("{}", String::from_utf8_lossy(&output.stdout));
            }
            if !output.stderr.is_empty() {
                eprintln!("❗ Command Error Output:");
                eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            }
            
            if output.status.success() {
                println!("✅ Docker command completed successfully!");
            } else {
                eprintln!("❌ Docker command failed with exit code: {}", output.status.code().unwrap_or(-1));
            }
        }
        Err(error) => {
            eprintln!("🚨 Failed to execute docker command: {}", error);
        }
    }
}

async fn handle_create_docker_space(name: String) -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Initializing Docker space creation for: {}", name);
    
    let url = env::var("SUBSTRATE_NODE_URL")
        .unwrap_or_else(|_| "ws://127.0.0.1:9944".to_string());
    
    println!("🌐 Connecting to Substrate node at: {}", url);
    let api = OnlineClient::<PolkadotConfig>::from_url(&url).await?;
    
    println!("🔑 Preparing transaction signer...");
    let seed_phrase = env::var("SUBSTRATE_SEED_PHRASE")
        .unwrap_or_else(|_| "brick end genuine caution author bulk school rose trap ramp garden milk".to_string());

    let pair = sr25519::Pair::from_string(seed_phrase.as_str(), None)
        .map_err(|e| format!("Failed to create pair: {:?}", e))?;

    let signer = PairSigner::new(pair);
    
    println!("📤 Submitting transaction to create Docker space...");
    let tx = custom_runtime::tx().container_registry().create_space(name.clone().into_bytes());

    let progress = api
        .tx()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await?;
    
    println!("⏳ Waiting for transaction to be finalized...");
    let _ = progress.wait_for_finalized_success().await?;
    
    println!("✅ Successfully created Docker space!");
    println!("📦 Space Name: {}", name);

    Ok(())
}