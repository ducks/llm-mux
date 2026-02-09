mod backend_executor;
mod config;
mod role;
mod template;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "llmux")]
#[command(about = "Multiplexer for LLMs - route prompts to multiple backends")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Working directory (defaults to current)
    #[arg(global = true)]
    dir: Option<PathBuf>,

    /// Team to use (overrides auto-detection)
    #[arg(long, global = true)]
    team: Option<String>,

    /// Additional context files to include
    #[arg(long, global = true)]
    context: Option<Vec<PathBuf>>,

    /// Enable debug output
    #[arg(long, global = true)]
    debug: bool,

    /// Suppress normal output
    #[arg(long, global = true)]
    quiet: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a workflow
    Run {
        /// Workflow name
        workflow: String,

        /// Workflow arguments
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },

    /// Validate a workflow without running
    Validate {
        /// Workflow name
        workflow: String,
    },

    /// Check backend availability
    Doctor,

    /// List configured backends
    Backends,

    /// List configured teams
    Teams,

    /// List available workflows
    Workflows,

    /// Gather and seed project context
    Context,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let project_dir = cli.dir.as_deref();
    let config = config::LlmuxConfig::load(project_dir)?;

    match cli.command {
        Commands::Doctor => {
            println!("Checking backends...\n");
            for (name, backend) in config.enabled_backends() {
                let status = if backend.is_http() {
                    format!("http: {}", backend.command)
                } else {
                    format!("cli: {}", backend.command)
                };
                println!("  {} - {}", name, status);
            }
            if config.backends.is_empty() {
                println!("  (no backends configured)");
            }
            println!();
        }

        Commands::Backends => {
            for (name, backend) in &config.backends {
                let enabled = if backend.enabled { "✓" } else { "✗" };
                println!("{} {} - {}", enabled, name, backend.command);
            }
        }

        Commands::Teams => {
            for (name, team) in &config.teams {
                println!("{}", name);
                if !team.description.is_empty() {
                    println!("  {}", team.description);
                }
                if !team.detect.is_empty() {
                    println!("  detect: {:?}", team.detect);
                }
                if let Some(ref verify) = team.verify {
                    println!("  verify: {}", verify);
                }
            }
            if config.teams.is_empty() {
                println!("(no teams configured)");
            }
        }

        Commands::Workflows => {
            println!("(workflow listing not yet implemented)");
        }

        Commands::Validate { workflow } => {
            match config::load_workflow(&workflow, project_dir) {
                Ok(wf) => {
                    println!("✓ Workflow '{}' is valid", wf.name);
                    println!("  {} steps", wf.steps.len());
                }
                Err(e) => {
                    eprintln!("✗ Workflow validation failed:\n{}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Run { workflow, args } => {
            let _wf = config::load_workflow(&workflow, project_dir)?;
            let _ = args; // TODO: parse workflow args
            println!("(workflow execution not yet implemented)");
        }

        Commands::Context => {
            println!("(context seeding not yet implemented)");
        }
    }

    Ok(())
}
