// WebSocket Client for NemesisBot
mod client;
mod config;
mod external_input;
mod external_output;
mod request_lock;

use anyhow::Result;
use clap::Parser;
use colored::Colorize;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc;

use client::WebSocketClient;
use config::Config;
use external_output::ExternalOutput;
use request_lock::RequestLock;

// Command constants
const CMD_QUIT: &str = "/quit";
const CMD_EXIT: &str = "/exit";
const CMD_Q: &str = "/q";
const CMD_HELP: &str = "/help";
const CMD_H: &str = "/h";
const CMD_CLEAR: &str = "/clear";
const CMD_C: &str = "/c";
const CMD_STATS: &str = "/stats";

/// Command line arguments
#[derive(Parser, Debug)]
#[command(name = "websocket-client")]
#[command(about = "WebSocket client for NemesisBot communication", long_about = None)]
struct Args {
    /// Input program path (reads from stdout)
    #[arg(short = 'i', long = "input", value_name = "EXE_PATH")]
    input_program: Option<PathBuf>,

    /// Output program path (writes to stdin)
    #[arg(short = 'o', long = "output", value_name = "EXE_PATH")]
    output_program: Option<PathBuf>,
}

/// CLI State
struct CliState {
    running: Arc<AtomicBool>,
    input_buffer: String,
    request_lock: Arc<RequestLock>,
    output_program: Option<Arc<ExternalOutput>>,
}

impl CliState {
    fn new(running: Arc<AtomicBool>) -> Self {
        Self {
            running,
            input_buffer: String::new(),
            request_lock: Arc::new(RequestLock::new()),
            output_program: None,
        }
    }

    fn new_with_lock(running: Arc<AtomicBool>, lock: Arc<RequestLock>) -> Self {
        Self {
            running,
            input_buffer: String::new(),
            request_lock: lock,
            output_program: None,
        }
    }

    fn new_with_output_and_lock(running: Arc<AtomicBool>, output: Arc<ExternalOutput>, lock: Arc<RequestLock>) -> Self {
        Self {
            running,
            input_buffer: String::new(),
            request_lock: lock,
            output_program: Some(output),
        }
    }
}

/// Print banner
fn print_banner() {
    println!();
    let border = "╔════════════════════════════════════════════════════════╗";
    let title = "║  🤖 NemesisBot WebSocket Client v0.4.0                ";
    println!("{}", border.bright_blue());
    println!("{}", "║".bright_blue());
    println!("{}", title.bright_blue());
    println!("{}", "║".bright_blue());
    println!("{}", border.bright_blue());
    println!();
}

/// Print help
fn print_help() {
    println!();
    let header = "📖 Available Commands:";
    println!("{}", header.bright_cyan());
    println!("  {} - Show this help message", CMD_HELP);
    println!("  {}, {} - Exit the client", CMD_QUIT, CMD_EXIT);
    println!("  {} - Show connection statistics", CMD_STATS);
    println!("  {} - Clear the screen", CMD_CLEAR);
    println!("  ... - Any other text will be sent as a message to the server");
    println!();
}

/// Print prompt
fn print_prompt(config: &Config) {
    let prompt_str = if config.ui.prompt_style == "detailed" {
        "➤ [Connected] "
    } else {
        "➤ "
    };

    let prompt = prompt_str.bright_green();
    print!("{}", prompt);
    io::stdout().flush().unwrap();
}

/// Clear screen
fn clear_screen() {
    print!("\x1B[2J\x1B[1;1H");
    io::stdout().flush().unwrap();
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args = Args::parse();

    // Load configuration
    let config = Config::load_or_create_default();

    // Initialize logger
    if config.logging.enabled {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(&config.logging.level))
            .init();
    }

    // Print banner
    print_banner();

    let config_header = "📁 Configuration:";
    println!("{}", config_header.dimmed());
    println!("   Server URL: {}", config.server.url.bright_white());
    let reconnect_status = if config.reconnect.enabled { "✅" } else { "❌" };
    println!("   Auto-reconnect: {}", if config.reconnect.enabled { reconnect_status.green() } else { reconnect_status.red() });
    let heartbeat_status = if config.heartbeat.enabled { "✅" } else { "❌" };
    println!("   Heartbeat: {}", if config.heartbeat.enabled { heartbeat_status.green() } else { heartbeat_status.red() });
    let logging_status = if config.logging.enabled { "✅" } else { "❌" };
    println!("   Logging: {}", if config.logging.enabled { logging_status.green() } else { logging_status.red() });

    // Display external programs info
    if args.input_program.is_some() || args.output_program.is_some() {
        println!();
        let programs_header = "🔌 External Programs:";
        println!("{}", programs_header.dimmed());

        if let Some(ref input_path) = args.input_program {
            println!("   Input Program: {}", input_path.display().to_string().bright_white());
        } else {
            println!("   Input Program: {}", "CLI (none)".dimmed());
        }

        if let Some(ref output_path) = args.output_program {
            println!("   Output Program: {}", output_path.display().to_string().bright_white());
        } else {
            println!("   Output Program: {}", "CLI (none)".dimmed());
        }
    }
    println!();

    // Create channel for CLI to client communication
    let (cli_tx, cli_rx) = mpsc::unbounded_channel::<String>();

    // Setup output program if specified
    let output_program = if let Some(output_path) = args.output_program {
        let output_path_str = output_path.to_string_lossy().to_string();
        let output = Arc::new(ExternalOutput::new(config.clone(), output_path_str));

        match output.start().await {
            Ok(_) => {
                println!("{}", format!("✅ Output program started successfully").green());
                Some(output)
            }
            Err(e) => {
                eprintln!("{}", format!("⚠️  Failed to start output program: {}, using CLI output", e).yellow());
                None
            }
        }
    } else {
        None
    };

    // Create WebSocket client with external receiver and optional output/lock
    let config_for_client = config.clone();
    let mut ws_client = WebSocketClient::new(config_for_client)
        .with_external_receiver(cli_rx);

    // Add output program if specified
    if let Some(ref output) = output_program {
        ws_client = ws_client.with_output_program(output.clone());
    }

    // Add request lock (always add for busy control)
    let request_lock = Arc::new(RequestLock::new());
    ws_client = ws_client.with_request_lock(request_lock.clone());

    // Get the running flag to share with CLI
    let client_running = ws_client.get_running_flag();

    // Setup input program if specified
    if let Some(input_path) = args.input_program {
        let input_path_str = input_path.to_string_lossy().to_string();
        let input_tx = cli_tx.clone();
        let config_clone = config.clone();

        tokio::spawn(async move {
            let input_manager = external_input::ExternalInput::new(config_clone, input_path_str);
            if let Err(e) = input_manager.start(input_tx).await {
                eprintln!("{}", format!("⚠️  Input program manager failed: {}", e).yellow());
            }
        });
    }

    let mut client_handle = tokio::spawn(async move {
        if let Err(e) = ws_client.start().await {
            let error_msg = format!("❌ Client error: {}", e);
            eprintln!("{}", error_msg.red().bold());
        }
    });

    // Wait a bit for connection
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let ready_msg = "✅ Ready! Type your messages below.";
    println!("{}", ready_msg.green());
    print_help();

    let state = Arc::new(if let Some(ref output) = output_program {
        CliState::new_with_output_and_lock(client_running, output.clone(), request_lock.clone())
    } else {
        CliState::new_with_lock(client_running, request_lock.clone())
    });
    let state_clone = state.clone();
    let config_for_input = config.clone();
    let cli_tx_clone = cli_tx.clone();

    // Spawn input handling in a separate thread to avoid blocking async runtime
    // This is crucial for Windows where async stdin can block the entire runtime
    run_cli_loop_thread(state_clone, config_for_input, cli_tx_clone);

    // Wait for client task to complete
    // The client runs until /quit command or connection error
    if let Err(e) = client_handle.await {
        eprintln!("{}", format!("❌ Client error: {}", e).red().bold());
    }

    let closed_msg = "🔌 Connection closed";
    println!("\n{}", closed_msg.yellow());

    // Print final statistics
    println!();
    // Note: ws_client was moved, so we can't access stats here
    // Statistics will be printed by the client task when it ends

    Ok(())
}

/// Run CLI input loop using a separate thread to avoid blocking async runtime
fn run_cli_loop_thread(state: Arc<CliState>, config: Config, cli_tx: mpsc::UnboundedSender<String>) {
    thread::spawn(move || {
        let stdin = io::stdin();

        loop {
            print_prompt(&config);
            io::stdout().flush().unwrap();

            let mut input = String::new();

            // Read line (blocking, but in separate thread so it won't block async runtime)
            match stdin.read_line(&mut input) {
                Ok(0) => {
                    // EOF
                    break;
                }
                Ok(_) => {
                    let input = input.trim();

                    // Skip empty input
                    if input.is_empty() {
                        continue;
                    }

                    // Handle commands (synchronous version)
                    if handle_command_sync(&state, &config, input, &cli_tx) {
                        // Quit command
                        break;
                    }
                }
                Err(e) => {
                    let error_msg = format!("⚠️  Input error: {}", e);
                    eprintln!("{}", error_msg.yellow());
                    continue;
                }
            }
        }
    });
}

/// Handle CLI commands (synchronous version for thread)
/// Returns true if should exit
fn handle_command_sync(state: &CliState, _config: &Config, input: &str, cli_tx: &mpsc::UnboundedSender<String>) -> bool {
    if input == CMD_QUIT || input == CMD_EXIT || input == CMD_Q {
        state.running.store(false, Ordering::Relaxed);
        return true;
    }

    if input == CMD_HELP || input == CMD_H {
        print_help();
        return false;
    }

    if input == CMD_CLEAR || input == CMD_C {
        clear_screen();
        return false;
    }

    if input == CMD_STATS {
        // Statistics will be printed on exit
        let msg = "📊 Statistics will be shown on exit";
        println!("{}", msg.dimmed());
        return false;
    }

    // Try to acquire request lock before sending
    let runtime = tokio::runtime::Handle::try_current();
    if let Ok(rt) = runtime {
        let lock = state.request_lock.clone();
        let input_str = input.to_string();

        // Block on async lock acquisition
        match rt.block_on(async { lock.try_acquire(input_str).await }) {
            Ok(_) => {
                // Lock acquired, send message
                match cli_tx.send(input.to_string()) {
                    Ok(_) => {}
                    Err(e) => {
                        let error_msg = format!("⚠️  Failed to send message: {}", e);
                        eprintln!("{}", error_msg.yellow());
                        // Release lock on send failure
                        let _ = rt.block_on(async { lock.release().await });
                    }
                }
            }
            Err(err_msg) => {
                // Busy, show error message
                eprintln!("{}", err_msg.yellow());
            }
        }
    } else {
        // No runtime (shouldn't happen in main), send directly
        match cli_tx.send(input.to_string()) {
            Ok(_) => {}
            Err(e) => {
                let error_msg = format!("⚠️  Failed to send message: {}", e);
                eprintln!("{}", error_msg.yellow());
            }
        }
    }

    false
}
