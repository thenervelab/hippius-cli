# Hippius CLI
A Rust-based Command-Line Interface (CLI) for simplifying Docker registry interactions with your custom Substrate/IPFS-based Docker registry.

## Overview
The `hippius-cli` tool allows users to execute simplified commands such as:

```bash
hippius-cli docker push repo1/image2:latest
```

This internally maps to:

```bash
docker push localhost:3000/repo1/image2:latest
```

This CLI enhances usability by abstracting the need to specify the full registry URL every time.

---

## Features
- Simplified Docker commands (e.g., `docker push`, `docker pull`).
- Automatic URL mapping to a custom Docker registry (`localhost:3000` by default).
- Compatible with existing Docker workflows.

---

## Prerequisites
1. Rust installed on your system. ([Install Rust](https://www.rust-lang.org/tools/install))
2. Docker installed and running. ([Install Docker](https://docs.docker.com/get-docker/))

---

## Installation

### Step 1: Clone the Repository
```bash
git clone <repository-url>
cd hippius-cli
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

## Usage

### Push an Image
```bash
hippius-cli docker push repo1/image2:latest
```
This is equivalent to:
```bash
docker push localhost:3000/repo1/image2:latest
```

### Pull an Image
```bash
hippius-cli docker pull repo1/image2:latest
```
This is equivalent to:
```bash
docker pull localhost:3000/repo1/image2:latest
```



## License
This project is licensed under the MIT License. See the `LICENSE` file for details.
