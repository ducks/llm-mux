//! Output handlers for CLI commands
//!
//! Supports console (pretty), JSON, and log output modes.

use crate::config::StepResult;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

/// Output mode for CLI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputMode {
    #[default]
    Console,
    Json,
    Quiet,
}

impl OutputMode {
    /// Parse from string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "json" => Self::Json,
            "quiet" => Self::Quiet,
            _ => Self::Console,
        }
    }
}

/// Events emitted during workflow execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OutputEvent {
    WorkflowStart {
        name: String,
        steps: usize,
    },
    StepStart {
        name: String,
        index: usize,
        total: usize,
    },
    StepComplete {
        name: String,
        duration_ms: u64,
        success: bool,
    },
    StepError {
        name: String,
        error: String,
    },
    ParallelProgress {
        step: String,
        backends: Vec<String>,
        completed: usize,
    },
    WorkflowComplete {
        success: bool,
        duration_ms: u64,
        steps_completed: usize,
    },
    WorkflowError {
        error: String,
    },
    Info {
        message: String,
    },
    Debug {
        message: String,
    },
}

/// Output handler trait
pub trait OutputHandler: Send + Sync {
    /// Emit an event
    fn emit(&self, event: OutputEvent);

    /// Write final result
    fn result(&self, success: bool, output: Option<&str>);
}

/// Console output handler with colors
pub struct ConsoleHandler {
    debug: bool,
}

impl ConsoleHandler {
    /// Create a new console handler
    pub fn new(debug: bool) -> Self {
        Self { debug }
    }

    fn format_duration(ms: u64) -> String {
        if ms < 1000 {
            format!("{}ms", ms)
        } else {
            format!("{:.1}s", ms as f64 / 1000.0)
        }
    }
}

impl OutputHandler for ConsoleHandler {
    fn emit(&self, event: OutputEvent) {
        match event {
            OutputEvent::WorkflowStart { name, steps } => {
                eprintln!("Running workflow '{}' ({} steps)", name, steps);
            }
            OutputEvent::StepStart { name, index, total } => {
                eprint!("[{}/{}] {}... ", index, total, name);
                let _ = io::stderr().flush();
            }
            OutputEvent::StepComplete {
                duration_ms,
                success,
                ..
            } => {
                if success {
                    eprintln!("✓ ({})", Self::format_duration(duration_ms));
                } else {
                    eprintln!("✗ ({})", Self::format_duration(duration_ms));
                }
            }
            OutputEvent::StepError { name, error } => {
                eprintln!("Error in step '{}': {}", name, error);
            }
            OutputEvent::ParallelProgress {
                step,
                backends,
                completed,
            } => {
                eprintln!(
                    "  {} (parallel: {} - {}/{})",
                    step,
                    backends.join(", "),
                    completed,
                    backends.len()
                );
            }
            OutputEvent::WorkflowComplete {
                success,
                duration_ms,
                steps_completed,
            } => {
                eprintln!();
                if success {
                    eprintln!(
                        "✓ Workflow completed successfully ({} steps in {})",
                        steps_completed,
                        Self::format_duration(duration_ms)
                    );
                } else {
                    eprintln!(
                        "✗ Workflow failed after {} steps ({})",
                        steps_completed,
                        Self::format_duration(duration_ms)
                    );
                }
            }
            OutputEvent::WorkflowError { error } => {
                eprintln!("Error: {}", error);
            }
            OutputEvent::Info { message } => {
                eprintln!("{}", message);
            }
            OutputEvent::Debug { message } => {
                if self.debug {
                    eprintln!("[debug] {}", message);
                }
            }
        }
    }

    fn result(&self, _success: bool, output: Option<&str>) {
        if let Some(out) = output {
            println!("{}", out);
        }
    }
}

/// JSON output handler
pub struct JsonHandler {
    pretty: bool,
}

impl JsonHandler {
    /// Create a new JSON handler
    pub fn new(pretty: bool) -> Self {
        Self { pretty }
    }

    fn print_json<T: Serialize>(&self, value: &T) {
        let json = if self.pretty {
            serde_json::to_string_pretty(value)
        } else {
            serde_json::to_string(value)
        };

        if let Ok(s) = json {
            println!("{}", s);
        }
    }
}

impl OutputHandler for JsonHandler {
    fn emit(&self, event: OutputEvent) {
        self.print_json(&event);
    }

    fn result(&self, success: bool, output: Option<&str>) {
        #[derive(Serialize)]
        struct FinalResult<'a> {
            success: bool,
            output: Option<&'a str>,
        }

        self.print_json(&FinalResult { success, output });
    }
}

/// Quiet handler that emits nothing
pub struct QuietHandler;

impl OutputHandler for QuietHandler {
    fn emit(&self, _event: OutputEvent) {}
    fn result(&self, _success: bool, output: Option<&str>) {
        // Only print final output, nothing else
        if let Some(out) = output {
            println!("{}", out);
        }
    }
}

/// Create an output handler based on mode
pub fn create_handler(mode: OutputMode, debug: bool) -> Box<dyn OutputHandler> {
    match mode {
        OutputMode::Console => Box::new(ConsoleHandler::new(debug)),
        OutputMode::Json => Box::new(JsonHandler::new(true)),
        OutputMode::Quiet => Box::new(QuietHandler),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Mock handler for testing
    struct MockHandler {
        events: Arc<Mutex<Vec<OutputEvent>>>,
    }

    impl MockHandler {
        fn new() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn events(&self) -> Vec<OutputEvent> {
            self.events.lock().unwrap().clone()
        }
    }

    impl OutputHandler for MockHandler {
        fn emit(&self, event: OutputEvent) {
            self.events.lock().unwrap().push(event);
        }

        fn result(&self, _success: bool, _output: Option<&str>) {}
    }

    #[test]
    fn test_output_mode_from_str() {
        assert_eq!(OutputMode::from_str("json"), OutputMode::Json);
        assert_eq!(OutputMode::from_str("quiet"), OutputMode::Quiet);
        assert_eq!(OutputMode::from_str("console"), OutputMode::Console);
        assert_eq!(OutputMode::from_str("unknown"), OutputMode::Console);
    }

    #[test]
    fn test_mock_handler_captures_events() {
        let handler = MockHandler::new();

        handler.emit(OutputEvent::WorkflowStart {
            name: "test".into(),
            steps: 3,
        });
        handler.emit(OutputEvent::StepStart {
            name: "step1".into(),
            index: 1,
            total: 3,
        });

        let events = handler.events();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_console_format_duration() {
        assert_eq!(ConsoleHandler::format_duration(500), "500ms");
        assert_eq!(ConsoleHandler::format_duration(1000), "1.0s");
        assert_eq!(ConsoleHandler::format_duration(2500), "2.5s");
    }

    #[test]
    fn test_json_handler_serializes() {
        let handler = JsonHandler::new(false);
        // This would print to stdout; just verify it doesn't panic
        handler.emit(OutputEvent::Info {
            message: "test".into(),
        });
    }

    #[test]
    fn test_create_handler() {
        let _ = create_handler(OutputMode::Console, false);
        let _ = create_handler(OutputMode::Json, false);
        let _ = create_handler(OutputMode::Quiet, false);
    }
}
