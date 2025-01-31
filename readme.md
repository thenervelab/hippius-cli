# Hippius CLI
A Rust-based Command-Line Interface (CLI) for managing Docker registries, compute resources, and storage on a Substrate/IPFS-based blockchain.

## Overview
The `hippius-cli` tool provides a comprehensive set of commands for interacting with a decentralized infrastructure, including:
- Docker registry management
- Compute resource provisioning
- Storage operations
- Marketplace interactions

### Quick Examples
```bash
# Push a Docker image to the registry
hippius-cli docker push repo1/image2:latest

# Create a Docker space
hippius-cli create space docker --name my-space

# Purchase a compute plan
hippius-cli buy compute --plan-id <plan-hash> --image-name ubuntu-22.04

# Manage virtual machines
hippius-cli vm boot --name my-vm --plan-id <plan-hash>

# Pin files to storage
hippius-cli storage pin <file-hash1> <file-hash2>
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

- **Storage Operations**
  - Pin and unpin files
  - Decentralized file storage management

- **Marketplace Interactions**
  - Browse and purchase compute plans
  - Discover available resources

---

## Prerequisites
1. Rust installed on your system. ([Install Rust](https://www.rust-lang.org/tools/install))
2. Docker installed and running. ([Install Docker](https://docs.docker.com/get-docker/))
3. A running Substrate node with required modules
4. Environment variables:
   - `SUBSTRATE_NODE_URL`: Substrate node URL (default: `ws://127.0.0.1:9944`)
   - `SUBSTRATE_SEED_PHRASE`: Seed phrase for signing transactions (default: `//Alice`)

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
# Push an image
hippius-cli docker push repo1/image2:latest

# Pull an image
hippius-cli docker pull repo1/image2:latest
```

### Create Docker Space
```bash
# Create a Docker space
hippius-cli create space docker --name my-space
```

### Compute Plan Management
```bash
# List available OS images
hippius-cli list-images

# Purchase a compute plan
hippius-cli buy compute \
    --plan-id 0x1234... \
    --image-name ubuntu-22.04 \
    --location-id 1 \
    --cloud-init-cid optional-cloud-init-cid

# VM Operations
hippius-cli vm boot --name my-vm --plan-id 0x1234...
hippius-cli vm stop --name my-vm --plan-id 0x1234...
hippius-cli vm delete --name my-vm --plan-id 0x1234...
hippius-cli vm reboot --name my-vm --plan-id 0x1234...
```

### Storage Operations
```bash
# Pin files to storage
hippius-cli storage pin <file-hash1> <file-hash2>

# Unpin a specific file
hippius-cli storage unpin <file-hash>
```

---

## Getting Help
Use the `--help` flag to get detailed information about each command:
```bash
hippius-cli --help
hippius-cli docker --help
hippius-cli create space --help
hippius-cli buy compute --help
```

---

## License
This project is licensed under the MIT License. See the `LICENSE` file for details.
