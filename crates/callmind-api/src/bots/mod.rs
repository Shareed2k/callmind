pub mod evolution;
pub mod formatter;
pub mod telegram;
pub mod webhook;

pub use evolution::{handle_evolution_webhook, handle_evolution_webhook_by_event};
pub use formatter::BotResponseFormatter;
pub use telegram::TelegramBotService;
pub use webhook::handle_audio_webhook;
