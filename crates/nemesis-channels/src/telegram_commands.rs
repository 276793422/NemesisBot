//! Telegram bot command handlers (/help, /start, /show, /list).

use serde::Serialize;
use tracing::debug;

use nemesis_types::error::Result;

/// Parameters for sendMessage used by command handlers.
#[derive(Serialize)]
pub struct SendMessageParams {
    pub chat_id: i64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i64>,
}

/// Trait for Telegram command handling.
#[async_trait::async_trait]
pub trait TelegramCommander: Send + Sync {
    /// Handle /help command.
    async fn help(&self, chat_id: i64, message_id: i64) -> Result<()>;
    /// Handle /start command.
    async fn start(&self, chat_id: i64, message_id: i64) -> Result<()>;
    /// Handle /show command.
    async fn show(&self, chat_id: i64, message_id: i64, args: &str) -> Result<()>;
    /// Handle /list command.
    async fn list(&self, chat_id: i64, message_id: i64, args: &str) -> Result<()>;
}

/// Extracts command arguments from a message text.
pub fn command_args(text: &str) -> &str {
    let parts: Vec<&str> = text.splitn(2, ' ').collect();
    if parts.len() < 2 {
        ""
    } else {
        parts[1].trim()
    }
}

/// Default help text.
pub fn help_text() -> String {
    "/start - Start the bot\n\
     /help - Show this help message\n\
     /show [model|channel] - Show current configuration\n\
     /list [models|channels] - List available options"
        .to_string()
}

/// Default start text.
pub fn start_text() -> String {
    "Hello! I am NemesisBot".to_string()
}

/// Builds a show response.
pub fn show_response(args: &str, default_model: &str) -> String {
    match args {
        "model" => format!("Current Model: {default_model}"),
        "channel" => "Current Channel: telegram".to_string(),
        _ => format!("Unknown parameter: {args}. Try 'model' or 'channel'."),
    }
}

/// Builds a list response.
pub fn list_response(args: &str, default_model: &str, channels: &[&str]) -> String {
    match args {
        "models" => format!(
            "Configured Model: {default_model}\n\nTo change models, update config.yaml"
        ),
        "channels" => {
            let list = channels
                .iter()
                .map(|c| format!("- {c}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("Enabled Channels:\n{list}")
        }
        _ => format!("Unknown parameter: {args}. Try 'models' or 'channels'."),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_args_with_args() {
        assert_eq!(command_args("/show model"), "model");
    }

    #[test]
    fn test_command_args_no_args() {
        assert_eq!(command_args("/help"), "");
    }

    #[test]
    fn test_help_text() {
        let text = help_text();
        assert!(text.contains("/start"));
        assert!(text.contains("/help"));
    }

    #[test]
    fn test_start_text() {
        let text = start_text();
        assert!(text.contains("NemesisBot"));
    }

    #[test]
    fn test_show_response_model() {
        let resp = show_response("model", "gpt-4");
        assert!(resp.contains("gpt-4"));
    }

    #[test]
    fn test_show_response_unknown() {
        let resp = show_response("foo", "gpt-4");
        assert!(resp.contains("Unknown parameter"));
    }

    #[test]
    fn test_list_response_channels() {
        let resp = list_response("channels", "gpt-4", &["telegram", "discord"]);
        assert!(resp.contains("telegram"));
        assert!(resp.contains("discord"));
    }

    #[test]
    fn test_list_response_models() {
        let resp = list_response("models", "gpt-4", &[]);
        assert!(resp.contains("gpt-4"));
    }
}
