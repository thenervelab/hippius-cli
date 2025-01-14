use clap::{Parser, Subcommand};
use std::process::Command;

#[derive(Parser)]
#[command(name = "hippius-cli", about = "A CLI for interacting with the Hippius Docker Registry")]
struct Cli {
    /// The subcommand to run (e.g., "docker")
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
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Docker { docker_command, args } => {
            handle_docker_command(docker_command, args);
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
