mod apply_and_verify;
mod backend_executor;
mod cli;
mod config;
mod discovery;
mod logging;
mod memory;
mod process;
mod role;
mod template;
mod workflow;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

use cli::output::{OutputMode, create_handler};
use cli::{commands, signals};

#[derive(Parser)]
#[command(name = "llmux")]
#[command(about = "Multiplexer for LLMs - route prompts to multiple backends")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Working directory (defaults to current)
    #[arg(long, short = 'C', global = true)]
    dir: Option<PathBuf>,

    /// Team to use (overrides auto-detection)
    #[arg(long, global = true)]
    team: Option<String>,

    /// Additional context files to include
    #[arg(long, global = true)]
    context: Option<Vec<PathBuf>>,

    /// Output format (console, json, quiet)
    #[arg(long, global = true, default_value = "console")]
    output: String,

    /// Write workflow output to file
    #[arg(long, global = true)]
    output_file: Option<PathBuf>,

    /// Enable debug output
    #[arg(long, global = true)]
    debug: bool,

    /// Suppress normal output (same as --output=quiet)
    #[arg(long, global = true)]
    quiet: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a workflow
    Run {
        /// Workflow name
        workflow: String,

        /// Workflow arguments (key=value or positional)
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

    /// List configured roles
    Roles,

    /// List configured ecosystems
    Ecosystems,

    /// List available workflows
    Workflows,

    /// Gather and seed project context
    Context,

    /// Interactive configuration setup
    Init {
        /// Initialize global config (~/.config/llm-mux/config.toml)
        #[arg(long, conflicts_with = "project")]
        global: bool,

        /// Initialize project config (.llm-mux/config.toml)
        #[arg(long, conflicts_with = "global")]
        project: bool,

        /// Skip project type detection (only for project init)
        #[arg(long)]
        no_detect: bool,

        /// Force overwrite existing config
        #[arg(long)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging first
    let log_file = if cli.debug {
        // Auto-generate log file path when debug is enabled
        Some(logging::default_log_path("llm-mux")?)
    } else {
        cli.output_file.as_ref().and_then(|path| {
            if path.extension().is_some_and(|ext| ext == "log") {
                Some(path.clone())
            } else {
                None
            }
        })
    };

    if let Some(ref log_path) = log_file {
        if cli.debug {
            eprintln!("Debug logging to: {}", log_path.display());
        }
    }

    logging::init_logging(cli.debug, cli.quiet, log_file)?;

    // Determine output mode
    let output_mode = if cli.quiet {
        OutputMode::Quiet
    } else {
        OutputMode::from_str(&cli.output)
    };

    let handler = create_handler(output_mode, cli.debug);

    // Get working directory
    let working_dir = cli
        .dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Load config
    let config = Arc::new(config::LlmuxConfig::load(Some(&working_dir))?);

    // Setup cancellation token for signal handling
    let cancel_token = signals::CancellationToken::new();

    // Spawn signal handler task
    let signal_token = cancel_token.clone();
    tokio::spawn(async move {
        signals::setup_signal_handlers(signal_token).await;
    });

    // Execute command
    let exit_code = match cli.command {
        Commands::Run { workflow, args } => {
            match commands::run_workflow(
                &workflow,
                args,
                &working_dir,
                cli.team.as_deref(),
                config,
                &*handler,
                cli.output_file.as_deref(),
            )
            .await
            {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    1
                }
            }
        }

        Commands::Validate { workflow } => {
            match commands::validate_workflow(&workflow, Some(&working_dir), &*handler) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    1
                }
            }
        }

        Commands::Doctor => commands::doctor(&config, &working_dir, &*handler).await,

        Commands::Backends => {
            commands::list_backends(&config, &*handler);
            0
        }

        Commands::Teams => {
            commands::list_teams(&config, &*handler);
            0
        }

        Commands::Roles => {
            commands::list_roles(&config, &*handler);
            0
        }

        Commands::Ecosystems => {
            commands::list_ecosystems(&config, &*handler);
            0
        }

        Commands::Workflows => {
            handler.emit(cli::OutputEvent::Info {
                message: "(workflow listing not yet implemented)".into(),
            });
            0
        }

        Commands::Context => {
            handler.emit(cli::OutputEvent::Info {
                message: "(context seeding not yet implemented)".into(),
            });
            0
        }

        Commands::Init {
            global,
            project,
            no_detect,
            force,
        } => {
            match commands::init_config(&working_dir, global, project, no_detect, force, &*handler)
                .await
            {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    1
                }
            }
        }
    };

    std::process::exit(exit_code);
}
