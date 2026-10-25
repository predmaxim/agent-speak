//! Сторож: раз в минуту — здоровье агентов и что делать, чтобы работа не стояла.

/// Что Ома сделала в терминале агента — по этому `done` выбирает фразу пользователю.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum What {
    Interrupt,
    Kill,
    Resume,
    Wake,
    Continue,
    Nudge,
}
