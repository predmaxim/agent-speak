
pub mod claude;
pub mod codex;
pub mod tail;

/// Кусок текста агента из транскрипта.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub message_id: String,
    pub text: String,
}
