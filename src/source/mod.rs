
pub mod claude;
pub mod codex;
pub mod tail;

/// Кусок текста агента из транскрипта.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub message_id: String,
    pub text: String,
}

/// Служебный текст, который агент подставляет сам (`<task-notification>`, `<environment_context>`, `Caveat:`), а не человек.
pub fn is_injected(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("Caveat:") || t.strip_prefix('<').and_then(|r| r.chars().next()).is_some_and(|c| c.is_alphabetic())
}
