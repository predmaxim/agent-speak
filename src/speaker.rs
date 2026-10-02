//! Поток воспроизведения: берёт фразу, синтезирует, пишет в pw-cat кусками по 0,1 с,
//! между кусками проверяет generation (стоп/пауза).

use crate::audio::Player;
use crate::queue::Queue;
use crate::queue::Item;
use crate::tts::Tts;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub struct Shared {
    pub queue: Mutex<Queue>,
    pub cv: Condvar,
    pub generation: AtomicU64,
    pub voice: Mutex<(String, String)>, // (speaker, rate)
    pub busy: AtomicBool,
    pub current: Mutex<String>, // сессия звучащей фразы — для «Читаю: <проект>»
    pub changed: Mutex<Option<std::sync::mpsc::Sender<()>>>, // смена «говорит/молчит» → основной цикл
}

/// Синтезатор и приёмник звука — трейты только ради тестов с подделками.
pub trait Synth {
    fn synth(&mut self, text: &str, speaker: &str, rate: &str) -> Option<Vec<u8>>;
}
pub trait Sink {
    fn write(&mut self, pcm: &[u8]) -> bool;
    fn reset(&mut self);
}
impl Synth for Tts {
    fn synth(&mut self, text: &str, speaker: &str, rate: &str) -> Option<Vec<u8>> {
        Tts::synth(self, text, speaker, rate)
    }
}
impl Sink for Player {
    fn write(&mut self, pcm: &[u8]) -> bool {
        Player::write(self, pcm)
    }
    fn reset(&mut self) {
        Player::reset(self)
    }
}

impl Shared {
    pub fn new(queue: Queue, speaker: String, rate: String) -> Shared {
        Shared {
            queue: Mutex::new(queue),
            cv: Condvar::new(),
            generation: AtomicU64::new(0),
            voice: Mutex::new((speaker, rate)),
            busy: AtomicBool::new(false),
            current: Mutex::new(String::new()),
            changed: Mutex::new(None),
        }
    }

    /// generation растёт только под замком очереди — поэтому speaker, снявший снимок
    /// в одной секции с `pop`, не пропустит прерывание. Вызывающий НЕ держит замок очереди.
    fn bump(&self, q: &mut Queue, f: impl FnOnce(&mut Queue)) {
        f(q);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cv.notify_all();
    }

    /// Оборвать текущую фразу (сессионные прерывания).
    pub fn interrupt(&self) {
        self.bump(&mut self.queue.lock().unwrap(), |_| {});
    }

    pub fn pause(&self) {
        self.bump(&mut self.queue.lock().unwrap(), |q| q.pause(Instant::now()));
    }

    pub fn resume(&self) {
        self.queue.lock().unwrap().resume(Instant::now());
        self.cv.notify_all();
    }

    /// Some(сессия) — говорит, None — молчит. Сообщает основному циклу только об изменении.
    pub fn set_busy(&self, session: Option<&str>) {
        let was = self.busy.swap(session.is_some(), Ordering::SeqCst);
        let mut cur = self.current.lock().unwrap();
        let changed = was != session.is_some() || session.is_some_and(|s| *cur != s);
        if let Some(s) = session {
            *cur = s.to_string();
        }
        drop(cur);
        if changed {
            if let Some(tx) = &*self.changed.lock().unwrap() {
                let _ = tx.send(());
            }
        }
    }

    pub fn stop(&self) {
        self.bump(&mut self.queue.lock().unwrap(), |q| {
            q.clear();
            q.resume(Instant::now());
        });
    }
}

const CHUNK: usize = 9600; // 0,1 с s16 48 кГц моно
const TAIL: Duration = Duration::from_millis(1000); // ~0,7 с ещё играет из буфера pw-cat
const RETRY: Duration = Duration::from_secs(10);

/// Вернуть фразу в голову очереди, если её не отменили (стоп) — пауза не отменяет.
fn requeue(shared: &Shared, q: &mut Queue, item: Item, my_gen: u64) {
    if shared.generation.load(Ordering::SeqCst) == my_gen || q.paused() {
        q.push_front(item);
    }
}

// ponytail: «синтез наперёд» — за счёт буфера канала pw-cat (~0,7 с): пока доигрывает хвост,
// синтезируется следующая фраза. Явный конвейер на 1–2 фразы — если паузы между фразами заметны.
pub fn run(shared: Arc<Shared>, tts: Tts) {
    run_with(shared, tts, Player::new())
}

fn run_with(shared: Arc<Shared>, mut tts: impl Synth, mut player: impl Sink) {
    let mut tail: Option<(Item, u64, Instant)> = None;
    let mut write_failed: Option<String> = None;
    loop {
        let (item, my_gen) = {
            let mut q = shared.queue.lock().unwrap();
            loop {
                if let Some((_, g, t)) = &tail {
                    let changed = shared.generation.load(Ordering::SeqCst) != *g;
                    if changed || t.elapsed() > TAIL {
                        let (it, _, _) = tail.take().unwrap();
                        if changed {
                            player.reset();
                            if q.paused() {
                                q.push_front(it); // после паузы — с начала предложения
                            }
                        }
                    }
                }
                if let Some(it) = q.pop(Instant::now()) {
                    tail = None;
                    break (it, shared.generation.load(Ordering::SeqCst));
                }
                shared.set_busy(None);
                let to = if tail.is_some() { Duration::from_millis(100) } else { Duration::from_millis(500) };
                q = shared.cv.wait_timeout(q, to).unwrap().0;
            }
        };
        shared.set_busy(Some(&item.session));
        let (speaker, rate) = shared.voice.lock().unwrap().clone();
        let Some(pcm) = tts.synth(&item.text, &speaker, &rate) else {
            // синтез упал: очередь ждёт, а не теряет фразу
            let mut q = shared.queue.lock().unwrap();
            requeue(&shared, &mut q, item, my_gen);
            shared.set_busy(None);
            let deadline = Instant::now() + RETRY;
            while shared.generation.load(Ordering::SeqCst) == my_gen {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                q = shared.cv.wait_timeout(q, left).unwrap().0;
            }
            continue;
        };
        let mut done = true;
        for chunk in pcm.chunks(CHUNK) {
            if shared.generation.load(Ordering::SeqCst) != my_gen {
                player.reset();
                let mut q = shared.queue.lock().unwrap();
                if q.paused() {
                    q.push_front(item.clone()); // после паузы — с начала предложения
                }
                done = false;
                break;
            }
            if !player.write(chunk) {
                // не потерять молча, но только один раз подряд для той же фразы
                if write_failed.as_deref() != Some(&item.text) {
                    write_failed = Some(item.text.clone());
                    let mut q = shared.queue.lock().unwrap();
                    requeue(&shared, &mut q, item.clone(), my_gen);
                }
                done = false;
                break;
            }
        }
        if done {
            write_failed = None;
            tail = Some((item, my_gen, Instant::now()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::Kind;

    struct FakeSynth(Arc<Mutex<u32>>); // сколько первых вызовов провалить
    impl Synth for FakeSynth {
        fn synth(&mut self, _: &str, _: &str, _: &str) -> Option<Vec<u8>> {
            let mut n = self.0.lock().unwrap();
            if *n > 0 {
                *n -= 1;
                return None;
            }
            Some(vec![0; CHUNK * 2])
        }
    }
    struct FakeSink(Arc<Mutex<Vec<&'static str>>>, Arc<Mutex<u32>>); // события; сколько первых write провалить
    impl Sink for FakeSink {
        fn write(&mut self, _: &[u8]) -> bool {
            let mut n = self.1.lock().unwrap();
            if *n > 0 {
                *n -= 1;
                return false;
            }
            self.0.lock().unwrap().push("write");
            true
        }
        fn reset(&mut self) {
            self.0.lock().unwrap().push("reset");
        }
    }

    type Events = Arc<Mutex<Vec<&'static str>>>;

    fn start(synth_fails: u32, write_fails: u32) -> (Arc<Shared>, Events) {
        start_with(Arc::new(Mutex::new(synth_fails)), write_fails)
    }

    fn start_with(fails: Arc<Mutex<u32>>, write_fails: u32) -> (Arc<Shared>, Events) {
        let shared = Arc::new(Shared::new(Queue::new(Duration::from_secs(30)), "x".into(), "m".into()));
        let ev: Events = Default::default();
        let (s, e) = (shared.clone(), ev.clone());
        std::thread::spawn(move || {
            run_with(s, FakeSynth(fails), FakeSink(e, Arc::new(Mutex::new(write_fails))))
        });
        (shared, ev)
    }

    fn say(shared: &Shared) {
        let mut q = shared.queue.lock().unwrap();
        q.push(Item { session: "s".into(), text: "фраза".into(), kind: Kind::Manual, born: Instant::now() });
        shared.cv.notify_all();
    }

    fn count(ev: &Events, what: &str) -> usize {
        ev.lock().unwrap().iter().filter(|e| **e == what).count()
    }

    fn wait(mut f: impl FnMut() -> bool) {
        for _ in 0..100 {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("не дождались");
    }

    #[test]
    fn busy_changes_are_reported_once() {
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = Shared::new(Queue::new(Duration::from_secs(30)), "x".into(), "m".into());
        *shared.changed.lock().unwrap() = Some(tx);
        shared.set_busy(Some("a"));
        shared.set_busy(Some("a")); // без изменений — молчит
        shared.set_busy(Some("b")); // другая сессия — сообщает
        shared.set_busy(None);
        shared.set_busy(None);
        assert_eq!(rx.try_iter().count(), 3);
        assert_eq!(*shared.current.lock().unwrap(), "b");
        assert!(!shared.busy.load(Ordering::SeqCst));
    }

    #[test]
    fn speaker_reports_speaking_and_silence() {
        let (shared, ev) = start(0, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        *shared.changed.lock().unwrap() = Some(tx);
        say(&shared);
        wait(|| count(&ev, "write") == 2);
        rx.recv_timeout(Duration::from_secs(2)).unwrap(); // начал говорить
        rx.recv_timeout(Duration::from_secs(3)).unwrap(); // замолчал
        assert!(!shared.busy.load(Ordering::SeqCst));
        assert_eq!(*shared.current.lock().unwrap(), "s");
    }

    #[test]
    fn synth_failure_keeps_item_and_retries() {
        let fails = Arc::new(Mutex::new(1));
        let (shared, ev) = start_with(fails.clone(), 0);
        say(&shared);
        wait(|| *fails.lock().unwrap() == 0 && !shared.busy.load(Ordering::SeqCst) && !shared.queue.lock().unwrap().is_empty());
        assert_eq!(count(&ev, "write"), 0); // ждёт 10 с, не выбросила
        shared.interrupt(); // будит ожидание
        wait(|| count(&ev, "write") == 2);
    }

    #[test]
    fn pause_in_tail_resets_and_replays() {
        let (shared, ev) = start(0, 0);
        say(&shared);
        wait(|| count(&ev, "write") == 2);
        shared.pause();
        wait(|| count(&ev, "reset") == 1);
        shared.resume();
        wait(|| count(&ev, "write") == 4);
    }

    #[test]
    fn stop_in_tail_resets_without_replay() {
        let (shared, ev) = start(0, 0);
        say(&shared);
        wait(|| count(&ev, "write") == 2);
        shared.stop();
        wait(|| count(&ev, "reset") == 1);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(count(&ev, "write"), 2);
        assert!(shared.queue.lock().unwrap().is_empty());
    }

    #[test]
    fn write_failure_requeues_once() {
        let (shared, ev) = start(0, 1);
        say(&shared);
        wait(|| count(&ev, "write") == 2);
    }
}
