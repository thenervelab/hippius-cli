# Hippius CLI
A Rust-based Command-Line Interface (CLI) for managing Docker registries, compute resources, storage, and node operations on a Substrate/IPFS-based blockchain.

## Overview
The `hippius-cli` tool provides a comprehensive set of commands for interacting with a decentralized infrastructure, including:
- Docker registry management
- Compute resource provisioning
- Storage operations
- Marketplace interactions
- Node registration and management
- Miner and validator operations

### Quick Examples
```bash
# Push a Docker image to the registry
hippius-cli docker push repo1/image2:latest

# Create a Docker space
hippius-cli create-space docker --name my-space

# Purchase a compute plan
hippius-cli buy-compute plan --plan-id <plan-hash> --image-name ubuntu-22.04

# Manage virtual machines
hippius-cli vm boot --name my-vm --plan-id <plan-hash>

# Pin files to storage
hippius-cli storage pin <file-hash1> <file-hash2>

# Register a compute miner node
hippius-cli register-node --node-type ComputeMiner --node-id my-compute-node
```

---

## Features
- **Docker Registry**
  - Simplified Docker commands (`push`, `pull`)
  - Automatic URL mapping to custom registry
  - Create Docker spaces on blockchain

- **Compute Resources**
  - Purchase compute plans
  - Manage Virtual Machines (VM)
    - Boot, stop, delete, reboot VMs
  - List available OS disk images
  - Get VNC ports for miners

- **Storage Operations**
  - Pin and unpin files
  - Decentralized file storage management

- **Node Management**
  - Register different node types:
    - Validator
    - Compute Miner
    - Storage Miner
  - Query node information
  - View node registration requirements

- **Miner Operations**
  - Fetch compute and storage information
  - Check miner registration requirements

- **Marketplace Interactions**
  - Browse and purchase compute plans
  - Discover available resources
  - Check account credits

---

## Prerequisites
1. Rust installed on your system. ([Install Rust](https://www.rust-lang.org/tools/install))
2. Docker installed and running. ([Install Docker](https://docs.docker.com/get-docker/))
3. A running Substrate node with required modules
4. Environment variables:
   - `SUBSTRATE_NODE_URL`: Substrate node URL (default: `ws://127.0.0.1:9944`)
   - `SUBSTRATE_SEED_PHRASE`: Seed phrase for signing transactions

---

## Installation

### Step 1: Clone the Repository
```bash
git clone <repository-url>
cd hippius-relay-cli
```

### Step 2: Build the CLI
```bash
cargo build --release
```

### Step 3: Install the CLI
Move the binary to a directory in your `PATH`:
```bash
sudo cp target/release/hippius-cli /usr/local/bin
```

Verify the installation:
```bash
hippius-cli --help
```

---

## Detailed Usage

### Docker Commands
```bash
# Push an image to the registry
hippius-cli docker push repo1/image2:latest

# Pull an image from the registry
hippius-cli docker pull repo1/image2:latest
```

### Create Docker Space
```bash
# Create a Docker space
hippius-cli create-space docker --name my-space
```

### Compute Plan Management
```bash
# Purchase a compute plan
hippius-cli buy-compute plan --plan-id <plan-hash> \
    --image-name ubuntu-22.04 \
    --location-id 1 \
    --cloud-init-cid <optional-cloud-init-cid>

# List available OS images
hippius-cli list-images
```

### Virtual Machine Management
```bash
# Boot a VM
hippius-cli vm boot --name my-vm --plan-id <plan-hash>

# Stop a VM
hippius-cli vm stop --name my-vm --plan-id <plan-hash>

# Delete a VM
hippius-cli vm delete --name my-vm --plan-id <plan-hash>

# Reboot a VM
hippius-cli vm reboot --name my-vm --plan-id <plan-hash>
```

### Storage Operations
```bash
# Pin files to storage
hippius-cli storage pin <file-hash1> <file-hash2>

# Unpin a file from storage
hippius-cli storage unpin <file-hash>
```

### Node Registration
```bash
# Register a Validator node
hippius-cli register-node --node-type Validator --node-id my-validator-node

# Register a Compute Miner node
hippius-cli register-node --node-type ComputeMiner \
    --node-id my-compute-node \
    --ipfs-node-id <optional-ipfs-node-id>

# Register a Storage Miner node
hippius-cli register-node --node-type StorageMiner \
    --node-id my-storage-node \
    --ipfs-node-id <optional-ipfs-node-id>

# Get information about your registered node
hippius-cli get-node-info
```

### Miner Operations
```bash
# Fetch compute-related information
hippius-cli miner compute

# Fetch storage-related information
hippius-cli miner storage

# Get compute miner registration requirements
hippius-cli miner register-compute-miner

# Get storage miner registration requirements
hippius-cli miner register-storage-miner

# Get validator registration requirements
hippius-cli miner register-validator
```

### Other Utilities
```bash
# Check free credits for your account
hippius-cli get-credits

# Get VNC port for a miner
hippius-cli get-vnc-port [--miner-id <optional-miner-id>]

# Insert a key to the local node
hippius-cli insert-key --seed-phrase <seed-phrase> --public-key <public-key>
```

---

## Configuration
Configure your CLI by setting environment variables:
- Create a `.env` file in the project root
- Add the following variables:
  ```
  SUBSTRATE_NODE_URL=ws://your-substrate-node:9944
  SUBSTRATE_SEED_PHRASE=your-seed-phrase-here
  ```

---

## Contributing
Contributions are welcome! Please submit pull requests or open issues on the project's repository.

## License
[Specify your project's license]
