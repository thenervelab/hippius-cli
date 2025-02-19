use clap::{Parser, Subcommand, ValueEnum};
use std::process::Command;
use subxt::{OnlineClient, PolkadotConfig};
use dotenv::dotenv;
use std::env;
use subxt::tx::PairSigner;
use sp_core::{Pair, sr25519};
use subxt::utils::H256;
use sp_core::Encode;
use reqwest;
use serde_json;
use crate::custom_runtime::runtime_types::pallet_registration::types::NodeInfo;

use crate::custom_runtime::runtime_types::pallet_compute::types::MinerComputeRequest;
use crate::custom_runtime::registration::calls::types::register_node::NodeType;
use crate::custom_runtime::runtime_types::pallet_rankings::types::NodeRankings;
use crate::custom_runtime::runtime_types::pallet_marketplace::types::FileInput;
use crate::custom_runtime::runtime_types::pallet_credits::pallet::LockedCredit;
use crate::custom_runtime::runtime_types::pallet_credits::pallet::LockPeriod;
use crate::custom_runtime::runtime_types::pallet_marketplace::types::Plan;
use sp_core::crypto::Ss58Codec;
use subxt::utils::AccountId32;
use std::fs;
use std::path::Path;
use codec::Decode;
use subxt::dynamic;
use csv::ReaderBuilder;
use crate::custom_runtime::runtime_types::pallet_compute::types::ComputeRequest;

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
    CreateSpace {
        /// The type of entity to create (must be "docker").
        #[arg(value_enum, help = "Specify the entity type to create. Currently, only 'docker' is supported.")]
        space_type: SpaceType,

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
    BuyCompute {
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

        /// Optional miner ID
        #[arg(long, help = "Optional miner ID")]
        miner_id: Option<String>,
    },
    /// Storage operations for pinning and unpinning files
    Storage {
        /// The storage operation to perform
        #[arg(value_enum, help = "Specify the storage operation")]
        storage_command: StorageCommand,

        /// File hash and VM name
        #[arg(help = "File hash and VM name")]
        file_hash: String,
        /// VM name
        #[arg(help = "VM name")]
        vm_name: String,
    },
    /// List available OS disk images from the marketplace
    ListImages,
    /// Query free credits for signer's account
    GetCredits,
    /// Insert a key
    InsertKey {
        /// The seed phrase for the key
        #[arg(help = "Specify the seed phrase for the key")]
        seed_phrase: String,

        /// The public key to insert
        #[arg(help = "Specify the public key to insert")]
        public_key: String,
    },
    /// Get information about your registered node
    GetNodeInfo,
    /// Miner-related operations
    Miner {
        /// The miner operation to perform
        #[arg(value_enum, help = "Specify the miner operation")]
        miner_command: MinerCommand,
    },
    /// Get VNC port for a specific miner
    GetVncPort {
        /// The ID of the miner to query
        #[arg(long = "miner-id", help = "Specify the ID of the miner to query")]
        miner_id: Option<String>,
    },
    /// Get rankings for a specific miner
    GetRankings {
        /// Type of the node to register
        #[arg(long, help = "Type of node to register (Validator, ComputeMiner, StorageMiner)")]
        node_type: CliNodeType,

        /// Node ID (typically a peer ID)
        #[arg(long, help = "Node ID (e.g., libp2p peer ID)")]
        node_id: String,
    },
    /// Register a new node
    RegisterNode {
        /// Type of the node to register
        #[arg(long, help = "Type of node to register (Validator, ComputeMiner, StorageMiner)")]
        node_type: CliNodeType,

        /// Node ID (typically a peer ID)
        #[arg(long, help = "Node ID (e.g., libp2p peer ID)")]
        node_id: String,

        /// Optional flag to pay for registration in credits
        #[arg(long, help = "Pay for node registration using credits")]
        pay_in_credits: bool,

        /// Optional IPFS Node ID (required for Miner nodes)
        #[arg(long, help = "IPFS Node ID (required for Miner nodes)")]
        ipfs_node_id: Option<String>,
    },
    /// Generate a new Sr25519 keypair for Substrate
    GenerateKeys,
    /// Lock credits for a specific account
    LockCredits {
        /// The amount of credits to lock
        #[arg(help = "Specify the amount of credits to lock")]
        amount: u128,
    },
    /// List locked credits for the current account
    ListLockedCredits,
    /// Upload multiple files from a CSV file
    BulkUpload {
        /// Path to the CSV file containing file CIDs and names
        #[arg(short, long)]
        csv_path: String,
    },
    /// List all available marketplace plans
    ListPlans,
    /// List all compute requests (VMs) for the current user
    ListVms,
    /// List all IPFS file storage requests for the current user
    ListIpfsFiles,
    /// Fetch the current lock period from Credits pallet
    GetCurrentLockPeriod,
    /// Fetch the minimum lock amount from Credits pallet
    GetMinLockAmount,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum SpaceType {
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

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum MinerCommand {
    /// Fetch compute-related information
    Compute,
    /// Fetch storage-related information
    Storage,
    /// Get registration requirements for a Compute Miner
    RegisterComputeMiner,
    /// Get registration requirements for a Storage Miner
    RegisterStorageMiner,
    /// Get registration requirements for a Validator
    RegisterValidator,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
enum CliNodeType {
    /// Validator node
    Validator,
    /// Compute miner node
    ComputeMiner,
    /// Storage miner node
    StorageMiner,
}


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    
    let cli = Cli::parse();
    
    match &cli.command {
        Commands::Docker { docker_command, args } => {
            handle_docker_command(docker_command.clone(), args.clone());
        }
        Commands::CreateSpace { space_type, name } => {
            match space_type {
                SpaceType::Docker => {
                    if let Err(e) = handle_create_docker_space(name.clone()).await {
                        eprintln!("❌ Error creating Docker space: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        }
        Commands::Vm { vm_command, name, plan_id } => {
            handle_vm_command(vm_command.clone(), name.clone(), plan_id.clone()).await;
        }
        Commands::BuyCompute { 
            buy_type: BuyType::Plan, 
            plan_id, 
            location_id, 
            image_name, 
            cloud_init_cid, 
            pay_for, 
            miner_id 
        } => {
            if let Err(e) = handle_purchase_compute_plan(
                plan_id.clone(), 
                location_id.clone(), 
                image_name.clone(), 
                cloud_init_cid.clone(), 
                pay_for.clone(),
                miner_id.clone()
            ).await {
                eprintln!("❌ Failed to purchase plan: {}", e);
            }
        }
        Commands::Storage { 
            storage_command, 
            file_hash,
            vm_name
        } => {
            if let Err(e) = handle_storage_command(
                storage_command.clone(), 
                file_hash.clone(), 
                vm_name.clone()
            ).await {
                eprintln!("❌ Failed to perform storage operation: {}", e);
            }
        }
        Commands::ListImages => {
            handle_list_images().await?;
        }
        Commands::GetCredits => {
            handle_get_credits().await?;
        }
        Commands::InsertKey { seed_phrase, public_key } => {
            handle_insert_key(seed_phrase.to_string(), public_key.to_string()).await?;
        }
        Commands::GetNodeInfo => {
            handle_query_my_node().await?;
        }
        Commands::Miner { miner_command } => {
            match miner_command {
                MinerCommand::Compute => {
                    if let Err(e) = handle_compute_infos().await {
                        eprintln!("❌ Error fetching compute information: {}", e);
                        std::process::exit(1);
                    }
                }
                MinerCommand::Storage => {
                    if let Err(e) = handle_storage_infos().await {
                        eprintln!("❌ Error fetching storage information: {}", e);
                        std::process::exit(1);
                    }
                }
                MinerCommand::RegisterComputeMiner => {
                    if let Err(e) = handle_register_compute_miner_info().await {
                        eprintln!("❌ Error displaying compute miner registration info: {}", e);
                        std::process::exit(1);
                    }
                }
                MinerCommand::RegisterStorageMiner => {
                    if let Err(e) = handle_register_storage_miner_info().await {
                        eprintln!("❌ Error displaying storage miner registration info: {}", e);
                        std::process::exit(1);
                    }
                }
                MinerCommand::RegisterValidator => {
                    if let Err(e) = handle_register_validator_info().await {
                        eprintln!("❌ Error displaying validator registration info: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        }
        Commands::GetVncPort { miner_id } => {
            if let Err(e) = handle_get_vnc_port(miner_id.clone()).await {
                eprintln!("❌ Failed to get VNC port: {}", e);
            }
        }
        Commands::GetRankings { node_type, node_id } => {
            if let Err(e) = handle_get_rankings(*node_type, node_id.clone()).await {
                eprintln!("❌ Failed to get rankings: {}", e);
            }
        }
        Commands::RegisterNode { node_type, node_id, pay_in_credits, ipfs_node_id } => {
            if let Err(e) = handle_register_node(*node_type, node_id.clone(), *pay_in_credits, ipfs_node_id.clone()).await {
                eprintln!("❌ Failed to register node: {}", e);
            }
        }
        Commands::GenerateKeys => {
            if let Err(e) = handle_generate_keys().await {
                eprintln!("❌ Failed to generate keys: {}", e);
                std::process::exit(1);
            }
        }
        Commands::LockCredits { amount } => {
            if let Err(e) = handle_lock_credits(*amount).await {
                eprintln!("❌ Failed to lock credits: {}", e);
                std::process::exit(1);
            }
        }
        Commands::ListLockedCredits => {
            if let Err(e) = handle_list_locked_credits().await {
                eprintln!("❌ Failed to list locked credits: {}", e);
                std::process::exit(1);
            }
        }
        Commands::BulkUpload { csv_path } => {
            handle_bulk_upload(csv_path).await?;
        }
        Commands::ListPlans => {
            handle_list_plans().await?;
        }
        Commands::ListVms => {
            handle_list_vms().await?;
        }
        Commands::ListIpfsFiles => {
            handle_list_ipfs_files().await?;
        }
        Commands::GetCurrentLockPeriod => {
            handle_get_current_lock_period().await?;
        }
        Commands::GetMinLockAmount => {
            handle_get_min_lock_amount().await?;
        }
    }
    
    Ok(())
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
        .unwrap_or_else(|_| "wss://testnet.hippius.com".to_string());
    
    println!("🌐 Connecting to Substrate node at: {}", url);
    let api = OnlineClient::<PolkadotConfig>::from_url(&url).await?;
    
    println!("🔑 Preparing transaction signer...");
    let seed_phrase = env::var("SUBSTRATE_SEED_PHRASE")
        .unwrap_or_else(|_| "brick end genuine caution author bulk school rose trap ramp garden milk".to_string());

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

async fn handle_purchase_compute_plan(
    plan_id: H256, 
    location_id: Option<u32>, 
    image_name: String, 
    cloud_init_cid: Option<String>, 
    _pay_for: Option<String>,
    miner_id: Option<String>
) -> Result<(), Box<dyn std::error::Error>> {
    println!("🛒 Initiating Plan Purchase");
    
    let (api, signer) = setup_substrate_client().await?;
    
    // Convert inputs to required types
    let image_name_bytes = image_name.into_bytes();
    let cloud_init_cid_bytes = cloud_init_cid.map(|cid| cid.into_bytes());
    
    // Convert pay_for to AccountId if provided
    let pay_for_account: Option<_> = None;

    // Convert miner_id to bytes if provided
    let miner_id_bytes = miner_id.map(|id| id.into_bytes());

    println!("📤 Submitting transaction to purchase plan...");
    let tx = custom_runtime::tx()
        .marketplace()
        .purchase_plan(
            plan_id, 
            location_id, 
            image_name_bytes, 
            cloud_init_cid_bytes, 
            pay_for_account,
            miner_id_bytes
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
    file_hash: String,
    vm_name: String,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("🗄️ Initiating Storage Operation");
    
    let (api, signer) = setup_substrate_client().await?;
    
    match storage_command {
        StorageCommand::Pin => {
            // Create FileInput with file hash and VM name
            let file_input = FileInput {
                file_hash: file_hash.as_bytes().to_vec(),
                file_name: vm_name.as_bytes().to_vec(),
            };

            println!("📌 Submitting transaction to pin files...");
            let tx = custom_runtime::tx()
                .marketplace()
                .storage_request(vec![file_input]); 

            let progress = api
                .tx()
                .sign_and_submit_then_watch_default(&tx, &signer)
                .await?;
            
            println!("⏳ Waiting for transaction to be finalized...");
            let _ = progress.wait_for_finalized_success().await?;
            
            println!("✅ Successfully pinned files!");
        },
        StorageCommand::Unpin => {
            println!("🔓 Submitting transaction to unpin file...");
            let tx = custom_runtime::tx()
                .marketplace()
                .storage_unpin_request(file_hash.clone().into());

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

async fn handle_list_images() -> Result<(), Box<dyn std::error::Error>> {
    println!("🖼️  Fetching Available OS Disk Images...");
    
    let (api, _) = setup_substrate_client().await?;
    
    // Build a dynamic storage query for OS disk image URLs
    let storage_query = subxt::dynamic::storage("Marketplace", "OSDiskImageUrls", vec![]);
    
    // Fetch storage entries
    let mut results = api.storage().at_latest().await?.iter(storage_query).await?;
    
    let mut image_list = Vec::new();
    
    // Iterate through results
    while let Some(Ok(kv)) = results.next().await {
        // Convert keys and values to bytes
        let os_name_bytes = kv.key_bytes[kv.key_bytes.len() - 32..].to_vec();
        
        // Attempt to decode the value into a Vec<u8>
        let url_bytes: Vec<u8> = kv.value.as_type()?;
        
        // Convert bytes to strings
        let os_name = String::from_utf8_lossy(&os_name_bytes).into_owned();
        let url = String::from_utf8_lossy(&url_bytes).into_owned();
        
        // Optional: Add a filter to ensure valid URLs
        if !os_name.is_empty() && !url.is_empty() {
            image_list.push((os_name, url));
        }
    }
    
    if image_list.is_empty() {
        println!("No OS disk images found in the marketplace.");
        return Ok(());
    }
    
    println!("Available OS Disk Images:");
    println!("--------------------");
    for (os_name, url) in image_list {
        println!("OS: {:<10} | URL: {}", os_name, url);
    }
    
    Ok(())
}

/// Query free credits for signer's account
async fn handle_get_credits() -> Result<(), Box<dyn std::error::Error>> {
    println!("💰 Querying Free Credits...");

    let (api, signer) = setup_substrate_client().await?;

    // Use signer's account ID directly
    let target_account = subxt::dynamic::Value::from_bytes(&signer.account_id().encode());

    // Build a dynamic storage query for free credits
    let storage_query = subxt::dynamic::storage("Credits", "FreeCredits", vec![target_account]);

    // Fetch the credits value
    let credits_result = api.storage().at_latest().await?.fetch(&storage_query).await;

    match credits_result {
        Ok(Some(credits_value)) => {
            // Convert credits value to u128
            let credits: u128 = credits_value.as_type().unwrap_or(0);

            println!("✅ Free Credits:");
            println!("🔢 Amount: {}", credits);
        },
        Ok(None) => {
            println!("❌ No credits found for the account.");
        },
        Err(e) => {
            eprintln!("🚨 Error querying credits: {}", e);
            return Err(e.into());
        }
    }

    Ok(())
}

async fn handle_insert_key(seed_phrase: String, public_key: String) -> Result<(), Box<dyn std::error::Error>> {
    println!("🔑 Inserting key to local node...");

    // Prepare the JSON-RPC request payload
    let client = reqwest::Client::new();
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "author_insertKey",
        "params": [
            "hips",  // Hardcoded key type
            seed_phrase,
            public_key
        ]
    });

    // Send the request to the local node
    let response = client
        .post("http://localhost:9944")
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;

    // Check the response
    if response.status().is_success() {
        let response_text = response.text().await?;
        println!("✅ Key insertion response: {}", response_text);
        println!("🔑 Key inserted successfully!");
    } else {
        return Err(format!("Failed to insert key. Status: {}", response.status()).into());
    }

    Ok(())
}


/// Query and print node information where the signer is the owner
async fn handle_query_my_node() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Querying Node Registration for Your Node...");

    let (api, signer) = setup_substrate_client().await?;

    // Get the signer's account ID
    let signer_account_id = signer.account_id();

    // Build a dynamic storage query for the NodeRegistration map
    let storage_query = subxt::dynamic::storage("Registration", "NodeRegistration", vec![]);

    // Fetch all entries from the NodeRegistration map
    let mut results = api.storage().at_latest().await?.iter(storage_query).await?;

    let mut found = false;

    // Iterate through the results
    while let Some(Ok(kv)) = results.next().await {
        // Decode the value into the expected type
        let node_info: Option<NodeInfo<u32, AccountId32>> = kv.value.as_type()?;

        if let Some(node_info) = node_info {
            // Check if the owner matches the signer's account ID
            if node_info.owner == *signer_account_id {
                println!("✅ Your Node Information:");
                println!("------------------------");

                // Convert Vec<u8> fields to strings
                let node_id = String::from_utf8(node_info.node_id).unwrap_or_else(|_| "Invalid UTF-8".to_string());
                let node_type = node_info.node_type;
                let ipfs_node_id = node_info.ipfs_node_id
                    .map(|id| String::from_utf8(id).unwrap_or_else(|_| "Invalid UTF-8".to_string()))
                    .unwrap_or_else(|| "None".to_string());
                let status = node_info.status;

                // Convert AccountId32 to SS58 address
                let owner= node_info.owner;
                println!("Node ID: {}", node_id);
                println!("Node Type: {:?}", node_type);
                println!("IPFS Node ID: {}", ipfs_node_id);
                println!("Status: {:?}", status);
                println!("Registered At: {}", node_info.registered_at);
                println!("Owner: {:?}", owner);
                println!("------------------------");

                found = true;
                break; // Exit the loop once the node is found
            }
        }
    }

    if !found {
        println!("❌ Your node is not registered yet.");
    }

    Ok(())
}

/// Fetch and display compute-related information
async fn handle_compute_infos() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Querying Node Registration for Your Node...");

    let (api, signer) = setup_substrate_client().await?;

    // Get the signer's account ID
    let signer_account_id = signer.account_id();

    // Build a dynamic storage query for the NodeRegistration map
    let storage_query = subxt::dynamic::storage("Registration", "NodeRegistration", vec![]);

    // Fetch all entries from the NodeRegistration map
    let mut results = api.storage().at_latest().await?.iter(storage_query).await?;

    let mut found = false;

    // Iterate through the results
    while let Some(Ok(kv)) = results.next().await {
        // Decode the value into the expected type
        let node_info: Option<NodeInfo<u32, AccountId32>> = kv.value.as_type()?;

        if let Some(node_info) = node_info {
            // Check if the owner matches the signer's account ID
            if node_info.owner == *signer_account_id {
                println!("✅ Your Node Information:");
                println!("------------------------");

                // Convert Vec<u8> fields to strings
                let node_id = String::from_utf8(node_info.node_id).unwrap_or_else(|_| "Invalid UTF-8".to_string());
                let node_type = node_info.node_type;
                let ipfs_node_id = node_info.ipfs_node_id
                    .map(|id| String::from_utf8(id).unwrap_or_else(|_| "Invalid UTF-8".to_string()))
                    .unwrap_or_else(|| "None".to_string());
                let status = node_info.status;

                // Convert AccountId32 to SS58 address
                let owner= node_info.owner;
                println!("Node ID: {}", node_id);
                println!("Node Type: {:?}", node_type);
                println!("IPFS Node ID: {}", ipfs_node_id);
                println!("Status: {:?}", status);
                println!("Registered At: {}", node_info.registered_at);
                println!("Owner: {:?}", owner);
                println!("------------------------");

                found = true;
                break; // Exit the loop once the node is found
            }
        }
    }

    if !found {
        println!("❌ Your node is not registered yet.");
    }

    println!("🖥️ Fetching Compute Information...");

    // Fetch libvirt version
    let libvirt_version = Command::new("libvirtd")
        .arg("--version")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|_| "Not installed".to_string());
    println!("📦 Libvirt Version: {}", libvirt_version);

    Ok(())
}

/// Fetch and display storage-related information
async fn handle_storage_infos() -> Result<(), Box<dyn std::error::Error>> {

    println!("🔍 Querying Node Registration for Your Node...");

    let (api, signer) = setup_substrate_client().await?;

    // Get the signer's account ID
    let signer_account_id = signer.account_id();

    // Build a dynamic storage query for the NodeRegistration map
    let storage_query = subxt::dynamic::storage("Registration", "NodeRegistration", vec![]);

    // Fetch all entries from the NodeRegistration map
    let mut results = api.storage().at_latest().await?.iter(storage_query).await?;

    let mut found = false;

    // Iterate through the results
    while let Some(Ok(kv)) = results.next().await {
        // Decode the value into the expected type
        let node_info: Option<NodeInfo<u32, AccountId32>> = kv.value.as_type()?;

        if let Some(node_info) = node_info {
            // Check if the owner matches the signer's account ID
            if node_info.owner == *signer_account_id {
                println!("✅ Your Node Information:");
                println!("------------------------");

                // Convert Vec<u8> fields to strings
                let node_id = String::from_utf8(node_info.node_id).unwrap_or_else(|_| "Invalid UTF-8".to_string());
                let node_type = node_info.node_type;
                let ipfs_node_id = node_info.ipfs_node_id
                    .map(|id| String::from_utf8(id).unwrap_or_else(|_| "Invalid UTF-8".to_string()))
                    .unwrap_or_else(|| "None".to_string());
                let status = node_info.status;

                // Convert AccountId32 to SS58 address
                let owner= node_info.owner;
                println!("Node ID: {}", node_id);
                println!("Node Type: {:?}", node_type);
                println!("IPFS Node ID: {}", ipfs_node_id);
                println!("Status: {:?}", status);
                println!("Registered At: {}", node_info.registered_at);
                println!("Owner: {:?}", owner);
                println!("------------------------");

                found = true;
                break; // Exit the loop once the node is found
            }
        }
    }

    if !found {
        println!("❌ Your node is not registered yet.");
    }


    println!("💽 Fetching Storage Information...");

    // Fetch IPFS version
    let ipfs_version = Command::new("ipfs")
        .arg("--version")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|_| "Not installed".to_string());
    println!("📦 IPFS Version of your node is : {}", ipfs_version);


    Ok(())
}

/// Display registration requirements for a Compute Miner
async fn handle_register_compute_miner_info() -> Result<(), Box<dyn std::error::Error>> {
    println!("🖥️ Compute Miner Node Registration Requirements:");
    println!("------------------------------------------------");
    println!("1. Node Type: ComputeMiner");
    println!("2. Required Information:");
    println!("   a. Node ID: A unique identifier for your compute node");
    println!("      - Recommended format: Cryptographically secure hash or UUID");
    println!("      - Example: 'compute-node-01' or a SHA256 hash");
    println!("   b. IPFS Node ID (Optional):");
    println!("      - If you're running an IPFS node alongside your compute node");
    println!("      - Can be retrieved using `ipfs id` command");
    println!("\n🔧 Technical Recommendations:");
    println!("- Ensure your node meets minimum compute requirements");
    println!("- Have a stable internet connection");
    println!("- Recommended Hardware:");
    println!("  * CPU: 4+ cores");
    println!("  * RAM: 16+ GB");
    println!("  * Storage: 256+ GB SSD");
    println!("  * Network: 100+ Mbps bandwidth");
    
    println!("\n📝 Example Registration Command:");
    println!("`hippius-cli register-node --type ComputeMiner --node-id <your-unique-node-id>`");
    
    Ok(())
}

/// Display registration requirements for a Storage Miner
async fn handle_register_storage_miner_info() -> Result<(), Box<dyn std::error::Error>> {
    println!("💽 Storage Miner Node Registration Requirements:");
    println!("------------------------------------------------");
    println!("1. Node Type: StorageMiner");
    println!("2. Required Information:");
    println!("   a. Node ID: A unique identifier for your storage node");
    println!("      - Recommended format: Cryptographically secure hash or UUID");
    println!("      - Example: 'storage-node-01' or a SHA256 hash");
    println!("   b. IPFS Node ID (Recommended):");
    println!("      - Retrieve using `ipfs id` command");
    println!("      - Helps in distributed storage network integration");
    
    println!("\n🔧 Technical Recommendations:");
    println!("- High-capacity, reliable storage infrastructure");
    println!("- Recommended Hardware:");
    println!("  * Storage: 10+ TB HDD/SSD");
    println!("  * CPU: 4+ cores");
    println!("  * RAM: 16+ GB");
    println!("  * Network: 100+ Mbps bandwidth, stable connection");
    
    println!("\n📝 Example Registration Command:");
    println!("`hippius-cli register-node --type StorageMiner --node-id <your-unique-node-id> --ipfs-node-id <optional-ipfs-node-id>`");
    
    Ok(())
}

/// Display registration requirements for a Validator
async fn handle_register_validator_info() -> Result<(), Box<dyn std::error::Error>> {
    println!("🛡️ Validator Node Registration Requirements:");
    println!("------------------------------------------------");
    println!("1. Node Type: Validator");
    println!("2. Required Information:");
    println!("   a. Node ID: A unique identifier for your validator node");
    println!("      - Recommended format: Cryptographically secure hash or UUID");
    println!("      - Example: 'validator-node-01' or a SHA256 hash");
    
    println!("\n🔧 Technical Recommendations:");
    println!("- High uptime and reliability");
    println!("- Secure and well-maintained infrastructure");
    println!("- Recommended Hardware:");
    println!("  * CPU: 8+ cores, high single-thread performance");
    println!("  * RAM: 32+ GB");
    println!("  * Storage: 1+ TB SSD (NVMe preferred)");
    println!("  * Network: 1+ Gbps bandwidth, low latency");
    
    println!("\n🔐 Additional Requirements:");
    println!("- Sufficient stake to be elected as a validator");
    println!("- Running a full node with latest chain state");
    println!("- Secure key management");
    
    println!("\n📝 Example Registration Command:");
    println!("`hippius-cli register-node --type Validator --node-id <your-unique-node-id>`");
    
    Ok(())
}

async fn handle_get_vnc_port(miner_id: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Querying VNC Ports{}", 
        miner_id.as_ref().map_or_else(|| " for All Miners".to_string(), |id| format!(" for Miner: {}", id)));

    let (api, _) = setup_substrate_client().await?;

    // If a specific miner ID is provided, create a targeted storage query
    let storage_query = match &miner_id {
        Some(id) => {
            let miner_id_bytes = id.as_bytes().to_vec();
            subxt::dynamic::storage("Compute", "MinerComputeRequests", vec![
                subxt::dynamic::Value::from_bytes(&miner_id_bytes)
            ])
        },
        None => subxt::dynamic::storage("Compute", "MinerComputeRequests", vec![])
    };

    // Fetch entries from the MinerComputeRequests map
    let mut results = api.storage().at_latest().await?.iter(storage_query).await?;

    let mut found_any = false;

    // Iterate through the results
    while let Some(Ok(kv)) = results.next().await {
        // Decode the value as a Vec<MinerComputeRequest>
        let compute_requests: Vec<MinerComputeRequest<u32, H256>> = match kv.value.as_type() {
            Ok(requests) => requests,
            Err(e) => {
                eprintln!("🚨 Error decoding MinerComputeRequests: {}", e);
                continue; // Skip this entry and continue with next
            }
        };

        // If we got here, we found at least one request
        found_any = true;

        // Iterate through each compute request for this miner
        for (index, compute_request) in compute_requests.into_iter().enumerate() {
            println!("✅ Compute Request #{} Details:", index + 1);

            // Handle VNC port with pattern matching
            match compute_request.vnc_port {
                Some(port) => println!("🚪 VNC Port: {}", port),
                None => println!("❌ No VNC port assigned"),
            }

            println!("");
            println!("------------------------");
            println!("📋 Additional Request Details:");
            println!("   Miner Account ID: {:?}", String::from_utf8_lossy(&compute_request.miner_account_id));
            println!("   Job ID: {:?}", compute_request.job_id.map(|id| String::from_utf8_lossy(&id).to_string()));
            println!("   Request ID: {}", compute_request.request_id);
            println!("   Plan ID: {:?}", compute_request.plan_id);
            println!("------------------------");
        }

        // If a specific miner ID was provided, we can break after the first iteration
        if miner_id.is_some() {
            break;
        }
    }

    if !found_any {
        if let Some(id) = miner_id {
            println!("❌ No compute requests found for Miner: {}", id);
        } else {
            println!("❌ No compute requests found");
        }
    }

    Ok(())
}

async fn handle_get_rankings(node_type: CliNodeType, node_id: String) -> Result<(), Box<dyn std::error::Error>> {
    println!("🏆 Fetching Rankings for Miner: {} ({:?})", node_id, node_type);

    let (api, _) = setup_substrate_client().await?;

    // Determine the appropriate storage query based on node type
    let storage_query = match node_type {
        CliNodeType::Validator => {
            println!("Querying Validator Rankings...");
            subxt::dynamic::storage("RankingValidators", "RankedList", vec![])
        },
        CliNodeType::StorageMiner => {
            println!("Querying Storage Miner Rankings...");
            subxt::dynamic::storage("RankingStorage", "RankedList", vec![])
        },
        CliNodeType::ComputeMiner => {
            println!("Querying Compute Miner Rankings...");
            subxt::dynamic::storage("RankingCompute", "RankedList", vec![])
        },
    };

    // Fetch the ranked list
    let ranked_list_result = api.storage().at_latest().await?.fetch(&storage_query).await;

    match ranked_list_result {
        Ok(Some(list)) => {
            // Attempt to decode the list of node rankings
            let node_rankings: Vec<NodeRankings<u32>> = list.as_type()?;
            
            println!("\n📊 Rankings for {:?} Node:", node_type);
            println!("------------------------");

            // Convert the input node_id to Vec<u8> for comparison
            let target_node_id = node_id.as_bytes().to_vec();

            // Calculate total weight for normalization
            let total_weight: u128 = node_rankings.iter().map(|r| r.weight as u128).sum();

            // Iterate through the rankings and find the matching node
            let mut found = false;
            for (index, ranking) in node_rankings.iter().enumerate() {
                if ranking.node_id == target_node_id {
                    println!("Rank #{}: ", index + 1);
                    println!("  Node ID: {}", String::from_utf8_lossy(&ranking.node_id));
                    println!("  Node SS58 Address: {}", String::from_utf8_lossy(&ranking.node_ss58_address));
                    println!("  Node Type: {:?}", ranking.node_type);
                    println!("  Weight: {}", ranking.weight);
                    println!("  Node Ranking: {}", ranking.rank);
                    println!("  Last Updated: {}", ranking.last_updated);
                    println!("  Active: {}", ranking.is_active);

                    // Reward estimation logic
                    match node_type {
                        CliNodeType::Validator => {
                            println!("  Estimated Reward: 0 (Validators do not receive direct rewards)");
                        },
                        CliNodeType::ComputeMiner => {
                            // Fetch balance of the pallet
                            match query_pallet_balance(&api, 2).await {
                                Ok(balance) => {
                                    println!("💰 Ranking Pallet Balance: {} tokens", balance);
                                    let estimated_reward = if total_weight > 0 {
                                        (ranking.weight as u128 * balance) / total_weight
                                    } else {
                                        0
                                    };
                                    
                                    println!("  Estimated Reward: {} tokens", estimated_reward);
                                },
                                Err(_e) => {
                                    println!(" Estimated Reward: 0 ");
                                },
                            };
                            

                        },
                        CliNodeType::StorageMiner => {
                            // Fetch balance of the pallet
                            match query_pallet_balance(&api, 1).await {
                                Ok(balance) => {
                                    println!("💰 Ranking Pallet Balance: {} tokens", balance);
                                    let estimated_reward = if total_weight > 0 {
                                        (ranking.weight as u128 * balance) / total_weight
                                    } else {
                                        0
                                    };
                                    
                                    println!("  Estimated Reward: {} tokens", estimated_reward);
                                },
                                Err(_e) => {
                                    println!(" Estimated Reward: 0 ");
                                },
                            };
                        }
                    }

                    println!("------------------------");
                    found = true;
                    break; // Exit the loop once the matching node is found
                }
            }

            if !found {
                println!("❌ No rankings found for the specified node ID: {}", node_id);
            }
        },
        Ok(None) => {
            println!("No rankings found for {:?} nodes.", node_type);
        },
        Err(e) => {
            eprintln!("🚨 Error querying rankings: {}", e);
            return Err(e.into());
        }
    }

    Ok(())
}

#[derive(codec::Decode)]
struct AccountInfo {
    nonce: u32,
    consumers: u32,
    providers: u32,
    sufficients: u32,
    data: AccountData,
}

#[derive(codec::Decode)]
struct AccountData {
    free: u128,
    reserved: u128,
    frozen: u128,  // Corrected field name
    flags: u128,   // Added missing field
}


async fn query_pallet_balance(
    api: &OnlineClient<PolkadotConfig>, 
    pallet_id: u128
) -> Result<u128, Box<dyn std::error::Error>> {

    // compute Ranking Pallet Balance
    if pallet_id == 2 {
        let account_id: AccountId32 = "5EYCAe5j7t7RXEmC8rYDo9i4Z6tWLWf1SbncYcPTkRreCc58"
        .parse()
        .map_err(|e| format!("Invalid SS58 address: {:?}", e))?;

        let target_account = dynamic::Value::from(account_id.encode());
        let balance_query = dynamic::storage("System", "Account", vec![target_account]);
    
        let balance_result = api.storage().at_latest().await?.fetch(&balance_query).await;
    
        match balance_result {
            Ok(Some(balance_value)) => {
                match AccountInfo::decode(&mut &balance_value.encoded()[..]) {
                    Ok(account_info) => {
                        let free_balance = account_info.data.free;
                        Ok(free_balance)
                    }
                    Err(e) => {
                        eprintln!("🚨 Failed to decode account info: {:?}", e);
                        Err("Failed to decode account balance".into())
                    }
                }
            }
            Ok(None) => {
                println!("❌ No balance found for the account");
                Ok(0)
            }
            Err(e) => {
                eprintln!("🚨 Error querying pallet balance: {}", e);
                Err(e.into())
            }
        }
    }
    else{
        let account_id: AccountId32 = "5EYCAe5j7t7RXEmC8qLjtLHVtXsw8pj56jCBZEZZM7x5ETVJ"
        .parse()
        .map_err(|e| format!("Invalid SS58 address: {:?}", e))?;

        let target_account = dynamic::Value::from(account_id.encode());
        let balance_query = dynamic::storage("System", "Account", vec![target_account]);
    
        let balance_result = api.storage().at_latest().await?.fetch(&balance_query).await;
    
        match balance_result {
            Ok(Some(balance_value)) => {
                match AccountInfo::decode(&mut &balance_value.encoded()[..]) {
                    Ok(account_info) => {
                        let free_balance = account_info.data.free;
                        Ok(free_balance)
                    }
                    Err(e) => {
                        eprintln!("🚨 Failed to decode account info: {:?}", e);
                        Err("Failed to decode account balance".into())
                    }
                }
            }
            Ok(None) => {
                println!("❌ No balance found for the account");
                Ok(0)
            }
            Err(e) => {
                eprintln!("🚨 Error querying pallet balance: {}", e);
                Err(e.into())
            }
        }
    }

}

async fn handle_register_node(node_type: CliNodeType, node_id: String, pay_in_credits: bool, ipfs_node_id: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Initializing Node Registration for: {} ", node_id);
    
    let (api, signer) = setup_substrate_client().await?;
    
    // Convert CliNodeType to runtime NodeType
    let runtime_node_type = match node_type {
        CliNodeType::Validator => NodeType::Validator,
        CliNodeType::ComputeMiner => NodeType::ComputeMiner,
        CliNodeType::StorageMiner => NodeType::StorageMiner,
    };
    
    println!("📤 Submitting transaction to register node...");
    let tx = custom_runtime::tx().registration().register_node(runtime_node_type, node_id.clone().into_bytes(), pay_in_credits, ipfs_node_id.map(|id| id.into_bytes()));

    let progress = api
        .tx()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await?;
    
    println!("⏳ Waiting for transaction to be finalized...");
    let _ = progress.wait_for_finalized_success().await?;
    
    println!("✅ Successfully registered node!");
    println!("📦 Node ID: {}", node_id);

    Ok(())
}

async fn handle_generate_keys() -> Result<(), Box<dyn std::error::Error>> {
    // Hardcoded keypair directory
    let keypair_dir = "/home/faiz/hippius/chains/hippius-testnet/keystore";

    // Ensure directory exists
    fs::create_dir_all(keypair_dir)?;

    // Generate a new Sr25519 keypair
    let (pair, seed) = sr25519::Pair::generate();

    // Serialize keypair components
    let public_key = pair.public();
    let public_key_ss58 = public_key.to_ss58check(); // Convert public key to SS58 format

    // Prepare file paths
    let public_key_path = Path::new(keypair_dir).join("public_key.ss58");
    let seed_path = Path::new(keypair_dir).join("seed.bin");

    // Write public key and seed to files
    fs::write(&public_key_path, &public_key_ss58)?;
    fs::write(&seed_path, &seed)?; // Save seed as raw binary

    println!("🔑 Keypair Generated Successfully!");
    println!("📁 Keypair Directory: {}", keypair_dir);
    println!("📄 Public Key Path: {}", public_key_path.display());
    println!("📄 Seed Path: {}", seed_path.display());

    Ok(())
}

async fn handle_lock_credits(amount: u128) -> Result<(), Box<dyn std::error::Error>> {
    let (api, signer) = setup_substrate_client().await?;

    println!("📤 Submitting transaction to lock credits...");
    let tx = custom_runtime::tx().credits().lock_credits(amount);
    
    let progress = api
        .tx()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await?;
    
    println!("⏳ Waiting for transaction to be finalized...");
    let _ = progress.wait_for_finalized_success().await?;
    
    println!("✅ Successfully locked {} credits!", amount);
    
    Ok(())
}

async fn handle_list_locked_credits() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔒 Fetching Locked Credits...");

    let (api, signer) = setup_substrate_client().await?;

    // Get the signer's account ID
    let signer_account_id = signer.account_id();

    // Build a dynamic storage query for LockedCredits
    let storage_query = subxt::dynamic::storage("Credits", "LockedCredits", vec![
        subxt::dynamic::Value::from_bytes(&signer_account_id.encode())
    ]);

    // Fetch the locked credits
    let locked_credits_result = api.storage().at_latest().await?.fetch(&storage_query).await;

    match locked_credits_result {
        Ok(Some(credits_value)) => {
            // Decode the locked credits
            let locked_credits: Vec<LockedCredit<AccountId32, u32>> = credits_value.as_type()?;

            if locked_credits.is_empty() {
                println!("❌ No locked credits found for your account.");
                return Ok(());
            }

            println!("🏦 Locked Credits:");
            println!("------------------------");
            for (index, credit) in locked_credits.iter().enumerate() {
                println!("Lock #{}", index + 1);
                println!("  Amount Locked: {}", credit.amount_locked);
                println!("  Created At Block: {}", credit.created_at);
                println!("  Lock ID: {}", credit.id);
                println!("  Fulfilled: {}", credit.is_fulfilled);
                if let Some(tx_hash) = &credit.tx_hash {
                    println!("  Transaction Hash: {}", String::from_utf8_lossy(tx_hash));
                }
                println!("------------------------");
            }

            let total_locked: u128 = locked_credits.iter().map(|c| c.amount_locked).sum();
            println!("💰 Total Locked Credits: {}", total_locked);
        },
        Ok(None) => {
            println!("❌ No locked credits found for your account.");
        },
        Err(e) => {
            eprintln!("🚨 Error querying locked credits: {}", e);
            return Err(e.into());
        }
    }

    Ok(())
}

async fn handle_bulk_upload(csv_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("🗄️ Initiating Bulk File Upload from CSV: {}", csv_path);

    // Validate CSV file exists
    if !Path::new(csv_path).exists() {
        return Err(format!("CSV file not found: {}", csv_path).into());
    }

    // Create a CSV reader
    let mut rdr = ReaderBuilder::new()
        .has_headers(true)
        .from_path(csv_path)?;

    // Prepare file inputs
    let mut file_inputs = Vec::new();

    // Iterate through CSV records
    for result in rdr.records() {
        let record = result?;
        
        // Assume CSV has two columns: file CID and file name
        if record.len() != 2 {
            return Err("CSV must have exactly two columns: file CID and file name".into());
        }

        let file_hash = record[0].to_string();
        let vm_name = record[1].to_string();

        file_inputs.push(FileInput {
            file_hash: file_hash.as_bytes().to_vec(),
            file_name: vm_name.as_bytes().to_vec(),
        });
    }

    // Perform bulk upload
    if !file_inputs.is_empty() {
        let (api, signer) = setup_substrate_client().await?;

        println!("📌 Submitting transaction to pin files...");
        let tx = custom_runtime::tx()
            .marketplace()
            .storage_request(file_inputs); 

        let progress = api
            .tx()
            .sign_and_submit_then_watch_default(&tx, &signer)
            .await?;
        
        println!("⏳ Waiting for transaction to be finalized...");
        let _ = progress.wait_for_finalized_success().await?;
        
        println!("✅ Successfully pinned files!");
    } else {
        println!("⚠️ No files found in the CSV to upload.");
    }

    Ok(())
}

async fn handle_list_plans() -> Result<(), Box<dyn std::error::Error>> {
    println!("📋 Fetching Available Marketplace Plans");

    let (api, _) = setup_substrate_client().await?;

    // Build a dynamic storage query for plans
    let storage_query = subxt::dynamic::storage("Marketplace", "Plans", vec![]);
    
    // Fetch storage entries
    let mut results = api.storage().at_latest().await?.iter(storage_query).await?;
    
    let mut plan_count = 0;
    
    // Iterate through results
    while let Some(Ok(kv)) = results.next().await {
        // Decode the plan from the value
        let plan: Plan<H256> = kv.value.as_type()?;
        
        // Convert byte vectors to strings for display
        let plan_name = String::from_utf8_lossy(&plan.plan_name).to_string();
        let plan_description = String::from_utf8_lossy(&plan.plan_description).to_string();
        let plan_technical_description = String::from_utf8_lossy(&plan.plan_technical_description).to_string();

        // Print plan details
        println!("Plan Details:");
        println!("  ID: {:?}", plan.id);
        println!("  Name: {}", plan_name);
        println!("  Description: {}", plan_description);
        println!("  Technical Description: {}", plan_technical_description);
        println!("  Price: {} tokens", plan.price);
        println!("  Suspended: {}", if plan.is_suspended { "Yes" } else { "No" });
        println!("---");

        plan_count += 1;
    }

    if plan_count == 0 {
        println!("⚠️ No plans found in the marketplace.");
    } else {
        println!("✅ Total Plans Found: {}", plan_count);
    }

    Ok(())
}

async fn handle_list_vms() -> Result<(), Box<dyn std::error::Error>> {
    println!("🖥️  Fetching Compute Requests for Current User");

    let (api, signer) = setup_substrate_client().await?;

    // Get the current user's account ID and convert to SS58 string
    let account_id = signer.account_id();

    // Build a dynamic storage query for compute requests
    let storage_query = subxt::dynamic::storage("Compute", "ComputeRequests", vec![
        subxt::dynamic::Value::from(account_id.encode())
    ]);
    
    // Fetch storage entries
    let storage_client = api.storage().at_latest().await?;
    let compute_requests_result = storage_client.fetch(&storage_query).await;

    match compute_requests_result {
        Ok(Some(value)) => {
            // Decode the compute requests for the user
            let compute_requests: Vec<ComputeRequest<AccountId32, u32, H256>> = value.as_type()?;

            if compute_requests.is_empty() {
                println!("⚠️ No compute requests found for the current user.");
                return Ok(());
            }
            
            println!("🔢 Total Compute Requests: {}", compute_requests.len());
            
            for (index, request) in compute_requests.iter().enumerate() {
                // Convert byte vectors to strings for display
                let image_name = String::from_utf8_lossy(&request.selected_image.name).to_string();
                let image_url = String::from_utf8_lossy(&request.selected_image.image_url).to_string();
                let plan_description = String::from_utf8_lossy(&request.plan_technical_description).to_string();
                
                // Convert cloud init CID to string if present
                let cloud_init_cid = request.cloud_init_cid
                    .as_ref()
                    .map(|cid| String::from_utf8_lossy(cid).to_string())
                    .unwrap_or_else(|| "Not specified".to_string());

                println!("\n🖥️ Compute Request #{}", index + 1);
                println!("  Request ID: {}", request.request_id);
                println!("  Status: {:?}", request.status);
                println!("  Plan ID: {:?}", request.plan_id);
                println!("  Plan Description: {}", plan_description);
                println!("  Image Name: {}", image_name);
                println!("  Image URL: {}", image_url);
                println!("  Created At: Block {}", request.created_at);
                println!("  Last Charged At: {}", 
                    request.last_charged_at
                        .map(|block| block.to_string())
                        .unwrap_or_else(|| "Never".to_string())
                );
                println!("  Is Assigned: {}", request.is_assigned);
                println!("  Cloud Init CID: {}", cloud_init_cid);
                println!("---");
            }
        },
        Ok(None) => {
            println!("⚠️ No compute requests found for the current user.");
        },
        Err(e) => {
            eprintln!("❌ Error fetching compute requests: {}", e);
            return Err(e.into());
        }
    }

    Ok(())
}

async fn handle_list_ipfs_files() -> Result<(), Box<dyn std::error::Error>> {
    println!("📦 Fetching IPFS File Hashes for Current User");

    let (api, signer) = setup_substrate_client().await?;

    // Get the current user's account ID
    let account_id = signer.account_id();

    // Build a dynamic storage query for user file hashes
    let storage_query = subxt::dynamic::storage("Marketplace", "UserFileHashes", vec![
        subxt::dynamic::Value::from(account_id.encode())
    ]);
    
    // Fetch storage entries
    let storage_client = api.storage().at_latest().await?;
    let file_hashes_result = storage_client.fetch(&storage_query).await;

    match file_hashes_result {
        Ok(Some(value)) => {
            // Decode the file hashes for the user
            let file_hashes: Vec<Vec<u8>> = value.as_type()?;

            if file_hashes.is_empty() {
                println!("⚠️ No file hashes found for the current user.");
                return Ok(());
            }

            println!("🔢 Total File Hashes: {}", file_hashes.len());
            
            for (index, file_hash) in file_hashes.iter().enumerate() {
                // Convert file hash to string for display
                let file_hash_str = String::from_utf8_lossy(file_hash).to_string();

                println!("\n📄 File Hash #{}", index + 1);
                println!("  {}", file_hash_str);
            }
        },
        Ok(None) => {
            println!("⚠️ No file hashes found for the current user.");
        },
        Err(e) => {
            eprintln!("❌ Error fetching file hashes: {}", e);
            return Err(e.into());
        }
    }

    Ok(())
}

async fn handle_get_current_lock_period() -> Result<(), Box<dyn std::error::Error>> {
    println!("🕒 Fetching Current Lock Period...");

    let (api, _) = setup_substrate_client().await?;

    // Build a dynamic storage query for CurrentLockPeriod
    let storage_query = subxt::dynamic::storage("Credits", "CurrentLockPeriod", vec![]);

    // Fetch the current lock period
    let lock_period_result = api.storage().at_latest().await?.fetch(&storage_query).await;

    match lock_period_result {
        Ok(Some(lock_period_value)) => {
            // Attempt to decode the lock period
            let lock_period: LockPeriod<u32> = lock_period_value.as_type()?;

            println!("✅ Current Lock Period Details:");
            println!("  Start Block: {}", lock_period.start_block);
            println!("  End Block: {}", lock_period.end_block);
        },
        Ok(None) => {
            println!("❌ No current lock period found.");
        },
        Err(e) => {
            eprintln!("🚨 Error querying current lock period: {}", e);
            return Err(e.into());
        }
    }

    Ok(())
}

async fn handle_get_min_lock_amount() -> Result<(), Box<dyn std::error::Error>> {
    println!("💰 Fetching Minimum Lock Amount...");

    let (api, _) = setup_substrate_client().await?;

    // Build a dynamic storage query for MinLockAmount
    let storage_query = subxt::dynamic::storage("Credits", "MinLockAmount", vec![]);

    // Fetch the minimum lock amount
    let min_lock_amount_result = api.storage().at_latest().await?.fetch(&storage_query).await;

    match min_lock_amount_result {
        Ok(Some(min_lock_amount_value)) => {
            // Attempt to decode the minimum lock amount
            let min_lock_amount: u128 = min_lock_amount_value.as_type()?;

            println!("✅ Minimum Lock Amount:");
            println!("  Amount: {}", min_lock_amount);
        },
        Ok(None) => {
            println!("❌ No minimum lock amount found.");
        },
        Err(e) => {
            eprintln!("🚨 Error querying minimum lock amount: {}", e);
            return Err(e.into());
        }
    }

    Ok(())
}
