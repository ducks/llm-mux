use std::path::PathBuf;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize logging based on output mode and debug flag
pub fn init_logging(debug: bool, quiet: bool, log_file: Option<PathBuf>) -> anyhow::Result<()> {
    let env_filter = if debug {
        EnvFilter::new("llm_mux=debug")
    } else if quiet {
        EnvFilter::new("llm_mux=error")
    } else {
        EnvFilter::new("llm_mux=info")
    };

    let fmt_layer = fmt::layer()
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_line_number(debug)
        .with_file(debug)
        .with_writer(std::io::stderr);

    if let Some(log_path) = log_file {
        // Create log directory if needed
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;

        let file_layer = fmt::layer()
            .with_ansi(false)
            .with_writer(file)
            .with_target(true)
            .with_line_number(true)
            .with_file(true);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .with(file_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .init();
    }

    Ok(())
}

/// Get default log file path for a workflow
pub fn default_log_path(workflow_name: &str) -> anyhow::Result<PathBuf> {
    let log_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
        .join("llm-mux")
        .join("logs");

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{}-{}.log", workflow_name, timestamp);

    Ok(log_dir.join(filename))
}
