# Hippius CLI
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Latest Release](https://img.shields.io/github/v/release/thenervelab/hipc)](https://github.com/thenervelab/hipc/releases/latest)
A Rust-based Command-Line Interface (CLI) for Ipfs and s3 storage, referral and node operations on Hippius blockchain.

## Overview
The `hipc` tool provides a comprehensive set of commands for interacting with a decentralized infrastructure, including:
- Docker registry management
- Storage operations
- Marketplace interactions
- Node registration and management
- Miner and validator operations

### Quick Examples
```bash
# Pin files to storage
hipc storage pin <file-hash1> <file-hash2>

```

---

## Features
- **Storage Operations**
  - Pin and unpin files
  - Decentralized file storage management

- **Node Management**
  - Register different node types:
    - Validator
    - Storage Miner
  - Query node information
  - View node registration requirements

- **Miner Operations**
  - Check miner registration requirements

- **Marketplace Interactions**
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

# Clone the repository
 git clone https://github.com/thenervelab/hipc.git
 cd hipc

# Build and install
 cargo install --path .

# Move the binary to a location in your PATH
 cp target/release/hipc /usr/local/bin/

---

## Detailed Usage

## Available Commands

### Storage Operations
- **Pin files to storage**
  ```bash
  hipc storage pin <file-hash1> <file-hash2>
  ```

- **Unpin a file from storage**
  ```bash
  hipc storage unpin <file-hash>
  ```

### Node Management
- **Register a Validator node**
  ```bash
  hipc register-node --node-type Validator --node-id my-validator-node
  ```

- **Register a Storage Miner node**
  ```bash
  hipc register-node --node-type StorageMiner --node-id my-storage-node --ipfs-node-id <optional-ipfs-node-id>
  ```

- **Get information about your registered node**
  ```bash
  hipc get-node-info
  ```

### Miner Operations
- **Fetch storage-related information**
  ```bash
  hipc miner storage
  ```

- **Get storage miner registration requirements**
  ```bash
  hipc miner register-storage-miner
  ```

- **Get validator registration requirements**
  ```bash
  hipc miner register-validator
  ```

### Account Operations
- **Transfer funds from one account to another**
  ```bash
  hipc account transfer --account-id <account_id> --amount <amount>
  ```

- **Stake funds**
  ```bash
  hipc account stake --amount <amount>
  ```

- **Unstake funds**
  ```bash
  hipc account unStake --amount <amount>
  ```

- **Withdraw funds**
  ```bash
  hipc account withdraw --amount <amount>
  ```

### Other Utilities
- **Check free credits for your account**
  ```bash
  hipc get-credits
  ```

- **Insert a key to the local node**
  ```bash
  hipc insert-key --seed-phrase <seed-phrase> --public-key <public-key>
  ```
  
- **Get HIPS key files**
  ```bash
  hipc get-hips-key
  ```

- **Get Ipfs node ID**
  ```bash
  hipc get-ipfs-node-id
  ```

- **Get node ID**
  ```bash
  hipc get-node-id
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
