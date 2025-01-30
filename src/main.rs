use clap::{Parser, Subcommand, ValueEnum};
use std::process::Command;
use subxt::{OnlineClient, PolkadotConfig};
use dotenv::dotenv;
use std::env;
use subxt::tx::PairSigner;
use sp_core::{Pair, sr25519};
use subxt::utils::H256;

#[subxt::subxt(runtime_metadata_path = "metadata.scale")]
pub mod custom_runtime {}

/// A CLI for interacting with the Hippius Docker Registry and Substrate Chain
#[derive(Parser)]
#[command(name = "hippius-cli", about = "A CLI for managing Docker registries and interacting with a Substrate blockchain.")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// The subcommand to run (e.g., "docker" or "create")
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Execute Docker commands like push or pull with registry transformations.
    Docker {
        /// The Docker subcommand (e.g., "push" or "pull").
        #[arg(help = "Specify the Docker command to execute (e.g., push, pull).")]
        docker_command: String,

        /// Arguments for the Docker command (e.g., "repo1/image2:latest").
        #[arg(help = "Specify additional arguments for the Docker command.")]
        args: Vec<String>,
    },
    /// Create a new Docker space on the Substrate chain.
    Create {
        /// The type of entity to create (must be "docker").
        #[arg(value_enum, help = "Specify the entity type to create. Currently, only 'docker' is supported.")]
        entity_type: EntityType,

        /// The name of the space to create.
        #[arg(help = "Specify the name of the Docker space to create.")]
        name: String,
    },
    /// Manage Virtual Machines
    Vm {
        /// The VM management command
        #[arg(value_enum, help = "Specify the VM management command")]
        vm_command: VmCommand,

        /// The name of the VM
        #[arg(help = "Specify the name of the VM")]
        name: String,

        /// The plan ID for the VM operation
        #[arg(help = "Specify the plan ID for the VM operation")]
        plan_id: H256,
    },
    /// Purchase a plan in the marketplace
    Buy {
        /// The type of item to buy
        #[arg(value_enum, help = "Specify the type of item to buy")]
        buy_type: BuyType,

        /// The plan ID to purchase
        #[arg(help = "Specify the plan ID to purchase")]
        plan_id: H256,

        /// Optional location ID
        #[arg(long, help = "Optional location ID")]
        location_id: Option<u32>,

        /// Selected image name
        #[arg(long, help = "Name of the selected image")]
        image_name: String,

        /// Optional cloud init CID
        #[arg(long, help = "Optional cloud init CID")]
        cloud_init_cid: Option<String>,

        /// Optional account to pay for the plan
        #[arg(long, help = "Optional account to pay for the plan")]
        pay_for: Option<String>,
    },
    /// Storage operations for pinning and unpinning files
    Storage {
        /// The storage operation to perform
        #[arg(value_enum, help = "Specify the storage operation")]
        storage_command: StorageCommand,

        /// File hash(es) to pin or unpin
        #[arg(help = "File hash(es) to pin or unpin")]
        file_hashes: Vec<String>,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum EntityType {
    /// Docker space.
    Docker,
}

#[derive(Debug)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum VmCommand {
    /// Boot a VM
    Boot,
    /// Stop a VM
    Stop,
    /// Delete a VM
    Delete,
    /// Reboot a VM
    Reboot
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum BuyType {
    /// Purchase a plan
    Plan,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum StorageCommand {
    /// Pin files to storage
    Pin,
    /// Unpin a specific file
    Unpin,
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
                        eprintln!("❌ Error creating Docker space: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        }
        Commands::Vm { vm_command, name, plan_id } => {
            handle_vm_command(vm_command, name, plan_id).await;
        }
        Commands::Buy { 
            buy_type: BuyType::Plan, 
            plan_id, 
            location_id, 
            image_name, 
            cloud_init_cid, 
            pay_for 
        } => {
            if let Err(e) = handle_purchase_plan(
                plan_id, 
                location_id, 
                image_name, 
                cloud_init_cid, 
                pay_for
            ).await {
                eprintln!("❌ Failed to purchase plan: {}", e);
            }
        }
        Commands::Storage { 
            storage_command, 
            file_hashes 
        } => {
            if let Err(e) = handle_storage_command(
                storage_command, 
                file_hashes
            ).await {
                eprintln!("❌ Failed to perform storage operation: {}", e);
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
    
    let (api, signer) = setup_substrate_client().await?;
    
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

async fn setup_substrate_client() -> Result<(OnlineClient<PolkadotConfig>, PairSigner<PolkadotConfig, sr25519::Pair>), Box<dyn std::error::Error>> {
    let url = env::var("SUBSTRATE_NODE_URL")
        .unwrap_or_else(|_| "ws://127.0.0.1:9944".to_string());
    
    println!("🌐 Connecting to Substrate node at: {}", url);
    let api = OnlineClient::<PolkadotConfig>::from_url(&url).await?;
    
    println!("🔑 Preparing transaction signer...");
    let seed_phrase = env::var("SUBSTRATE_SEED_PHRASE")
        .unwrap_or_else(|_| "//Alice".to_string());

    let pair = sr25519::Pair::from_string(seed_phrase.as_str(), None)
        .map_err(|e| format!("Failed to create pair: {:?}", e))?;

    let signer = PairSigner::new(pair);

    Ok((api, signer))
}

async fn handle_request_boot(name: String, plan_id: H256) -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Initializing Boot Request For Minner: {}", name);
    
    let (api, signer) = setup_substrate_client().await?;
    
    println!("📤 Submitting transaction to request boot...");
    let tx = custom_runtime::tx().compute().request_compute_boot(plan_id);

    let progress = api
        .tx()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await?;
    
    println!("⏳ Waiting for transaction to be finalized...");
    let _ = progress.wait_for_finalized_success().await?;
    
    println!("✅ Successfully requested boot!");
    println!("📦 Space Name: {}", name);
    println!("🆔 Plan ID: {:?}", plan_id);

    Ok(())
}

async fn handle_request_reboot(name: String, plan_id: H256) -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Initializing Boot Request For Minner: {}", name);
    
    let (api, signer) = setup_substrate_client().await?;
    
    println!("📤 Submitting transaction to request boot...");
    let tx = custom_runtime::tx().compute().request_compute_reboot(plan_id);

    let progress = api
        .tx()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await?;
    
    println!("⏳ Waiting for transaction to be finalized...");
    let _ = progress.wait_for_finalized_success().await?;
    
    println!("✅ Successfully requested boot!");
    println!("📦 Space Name: {}", name);

    Ok(())
}

async fn handle_request_delete(name: String, plan_id: H256) -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Initializing Delete Request For Minner: {}", name);
    
    let (api, signer) = setup_substrate_client().await?;
    
    println!("📤 Submitting transaction to request delete...");
    let tx = custom_runtime::tx().compute().request_compute_delete(plan_id);

    let progress = api
        .tx()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await?;
    
    println!("⏳ Waiting for transaction to be finalized...");
    let _ = progress.wait_for_finalized_success().await?;
    
    println!("✅ Successfully requested delete!");
    println!("📦 Space Name: {}", name);

    Ok(())
}

async fn handle_request_stop(name: String, plan_id: H256) -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Initializing Stop Request For Minner: {}", name);
    
    let (api, signer) = setup_substrate_client().await?;
    
    println!("📤 Submitting transaction to request stop...");
    let tx = custom_runtime::tx().compute().request_compute_stop(plan_id);

    let progress = api
        .tx()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await?;
    
    println!("⏳ Waiting for transaction to be finalized...");
    let _ = progress.wait_for_finalized_success().await?;
    
    println!("✅ Successfully requested stop!");
    println!("📦 Space Name: {}", name);

    Ok(())
}

async fn handle_vm_command(vm_command: VmCommand, name: String, plan_id: H256) {
    match vm_command {
        VmCommand::Boot => {
            // Call the new handle_request_boot function with plan_id
            if let Err(e) = handle_request_boot(name, plan_id).await {
                eprintln!("❌ Failed to stop VM: {}", e);
            }
        },
        VmCommand::Stop => {
            // Call the new handle_request_stop function
            if let Err(e) = handle_request_stop(name, plan_id).await {
                eprintln!("❌ Failed to stop VM: {}", e);
            }
        },
        VmCommand::Delete => {
            // Call the new handle_request_delete function
            if let Err(e) = handle_request_delete(name, plan_id).await {
                eprintln!("❌ Failed to delete VM: {}", e);
            }
        },
        VmCommand::Reboot => {
            // Call the new handle_request_reboot function
            if let Err(e) = handle_request_reboot(name, plan_id).await {
                eprintln!("❌ Failed to reboot VM: {}", e);
            }
        }
    }
}

async fn handle_purchase_plan(
    plan_id: H256, 
    location_id: Option<u32>, 
    image_name: String, 
    cloud_init_cid: Option<String>, 
    _pay_for: Option<String>
) -> Result<(), Box<dyn std::error::Error>> {
    println!("🛒 Initiating Plan Purchase");
    
    let (api, signer) = setup_substrate_client().await?;
    
    // Convert inputs to required types
    let image_name_bytes = image_name.into_bytes();
    let cloud_init_cid_bytes = cloud_init_cid.map(|cid| cid.into_bytes());
    
    // Convert pay_for to AccountId if provided
    let pay_for_account: Option<_> = None;

    println!("📤 Submitting transaction to purchase plan...");
    let tx = custom_runtime::tx()
        .marketplace()
        .purchase_plan(
            plan_id, 
            location_id, 
            image_name_bytes, 
            cloud_init_cid_bytes, 
            pay_for_account
        );

    let progress = api
        .tx()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await?;
    
    println!("⏳ Waiting for transaction to be finalized...");
    let _ = progress.wait_for_finalized_success().await?;
    
    println!("✅ Successfully purchased plan!");
    println!("🆔 Plan ID: {:?}", plan_id);

    Ok(())
}

async fn handle_storage_command(
    storage_command: StorageCommand, 
    file_hashes: Vec<String>
) -> Result<(), Box<dyn std::error::Error>> {
    println!("🗄️ Initiating Storage Operation");
    
    let (api, signer) = setup_substrate_client().await?;
    
    match storage_command {
        StorageCommand::Pin => {

            // Convert file hashes to Vec<Vec<u8>>
            let parsed_file_hashes: Vec<Vec<u8>> = file_hashes
                .into_iter()
                .map(|hash| hash.into_bytes())
                .collect();

            println!("📌 Submitting transaction to pin files...");
            let tx = custom_runtime::tx()
                .marketplace()
                .storage_request(parsed_file_hashes);

            let progress = api
                .tx()
                .sign_and_submit_then_watch_default(&tx, &signer)
                .await?;
            
            println!("⏳ Waiting for transaction to be finalized...");
            let _ = progress.wait_for_finalized_success().await?;
            
            println!("✅ Successfully pinned files!");
        },
        StorageCommand::Unpin => {
            // Ensure only one file hash is provided for unpinning
            if file_hashes.len() != 1 {
                return Err("Unpin operation requires exactly one file hash".into());
            }

            // Convert file hash to the required format
            let file_hash = file_hashes[0].clone();

            println!("🔓 Submitting transaction to unpin file...");
            let tx = custom_runtime::tx()
                .marketplace()
                .storage_unpin_request(file_hash.into());

            let progress = api
                .tx()
                .sign_and_submit_then_watch_default(&tx, &signer)
                .await?;
            
            println!("⏳ Waiting for transaction to be finalized...");
            let _ = progress.wait_for_finalized_success().await?;
            
            println!("✅ Successfully unpinned file!");
        }
    }

    Ok(())
}