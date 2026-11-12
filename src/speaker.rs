//! Поток воспроизведения: берёт фразу, синтезирует, пишет в pw-cat кусками по 0,1 с,
//! между кусками проверяет generation (стоп/пауза).

use crate::audio::Player;
use crate::queue::Queue;
use crate::queue::{Item, Kind};
use crate::tts::{Failed, Tts};
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
    pub changed: Mutex<Option<std::sync::mpsc::Sender<Play>>>, // сигналы основному циклу
    pub sink: Arc<Mutex<String>>, // узел PipeWire для pw-cat --target; пусто — вывод по умолчанию
    /// Когда кончается (кончился) звук последней речи — CLOCK_MONOTONIC, с (как time.monotonic() в Python); 0 — речи не было.
    /// «Смолкла» (busy гаснет) приходит раньше: pw-cat ещё доигрывает свой канал.
    pub played: Mutex<f64>,
    /// Номер реплики Омы, чья фраза у потока воспроизведения (синтез, запись); 0 — ничья. Ставится под замком очереди
    /// вместе с pop, снимается до «смолкла».
    pub voice_now: AtomicU64,
}

/// Сигнал потока воспроизведения основному циклу. Сессия — в самом сигнале: цикл, занятый дольше фразы, её не потеряет,
/// а судит по порядку сигналов, а не по тому, что видит в очереди к своему часу.
#[derive(Debug, PartialEq)]
pub enum Play {
    /// Заговорил или смолк.
    Busy,
    /// Пошёл звук фразы этой сессии: записан первый кусок (busy загорается раньше — до синтеза, а он бывает долгим).
    Sound(String),
    /// Фраза этой сессии ушла без звука и в очередь не вернётся: синтез не смог её текст, запись не прошла второй раз,
    /// стоп или перебив до первого куска.
    Dropped(String),
}

/// Синтезатор и приёмник звука — трейты только ради тестов с подделками.
pub trait Synth {
    /// tone и lead — интонация новости и проект перед паузой (см. python/silero_tts.py `ssml`); "" — как обычный текст.
    fn synth(&mut self, text: &str, speaker: &str, rate: &str, tone: &str, lead: &str) -> Result<Vec<u8>, Failed>;
}
pub trait Sink {
    fn write(&mut self, pcm: &[u8]) -> bool;
    fn reset(&mut self);
    fn close(&mut self);
    /// Сколько ещё звучит уже записанное.
    fn left(&self) -> Duration;
}
impl Synth for Tts {
    fn synth(&mut self, text: &str, speaker: &str, rate: &str, tone: &str, lead: &str) -> Result<Vec<u8>, Failed> {
        Tts::synth(self, text, speaker, rate, tone, lead)
    }
}
impl Sink for Player {
    fn write(&mut self, pcm: &[u8]) -> bool {
        Player::write(self, pcm)
    }
    fn reset(&mut self) {
        Player::reset(self)
    }
    fn close(&mut self) {
        Player::close(self)
    }
    fn left(&self) -> Duration {
        Player::left(self)
    }
}

/// CLOCK_MONOTONIC в секундах — те же часы, что time.monotonic() в Python у Омы.
pub fn mono() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime пишет в локальную структуру
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
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
            sink: Arc::new(Mutex::new(String::new())),
            played: Mutex::new(0.0),
            voice_now: AtomicU64::new(0),
        }
    }

    /// Сигнал основному циклу.
    fn signal(&self, p: Play) {
        if let Some(tx) = &*self.changed.lock().unwrap() {
            let _ = tx.send(p);
        }
    }

    /// Сколько реплик Омы с номером больше after ещё не отработаны: их фразы ждут в очереди или у потока воспроизведения.
    pub fn voices_left(&self, after: u64) -> usize {
        let q = self.queue.lock().unwrap();
        let mut left: Vec<u64> = q.voices().chain([self.voice_now.load(Ordering::SeqCst)]).filter(|&v| v > after).collect();
        left.sort_unstable();
        left.dedup(); // у реплики бывает несколько фраз
        left.len()
    }

    /// Речь смолкла, а звучать ей ещё left(): busy гаснет, конец звука — в played. Вызывающий НЕ держит замок played.
    fn idle(&self, left: impl FnOnce() -> Duration) {
        if self.busy.load(Ordering::SeqCst) {
            *self.played.lock().unwrap() = mono() + left().as_secs_f64();
        }
        self.set_busy(None);
    }

    /// generation растёт только под замком очереди — поэтому speaker, снявший снимок
    /// в одной секции с `pop`, не пропустит прерывание. Вызывающий НЕ держит замок очереди.
    fn bump(&self, q: &mut Queue, f: impl FnOnce(&mut Queue)) {
        f(q);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cv.notify_all();
    }

    /// Оборвать текущую фразу (в тестах — разбудить ожидание).
    #[cfg(test)]
    pub fn interrupt(&self) {
        self.bump(&mut self.queue.lock().unwrap(), |_| {});
    }

    pub fn pause(&self) {
        self.bump(&mut self.queue.lock().unwrap(), Queue::pause);
    }

    pub fn resume(&self) {
        self.queue.lock().unwrap().resume();
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
            self.signal(Play::Busy);
        }
    }

    /// Перебивание Омы: оборвать звучащую фразу; очередь — см. Queue::hush. Паузу не снимает.
    pub fn hush(&self) {
        self.bump(&mut self.queue.lock().unwrap(), Queue::hush);
    }

    pub fn stop(&self) {
        self.bump(&mut self.queue.lock().unwrap(), |q| {
            q.clear();
            q.resume();
        });
    }
}

const CHUNK: usize = 9600; // 0,1 с s16 48 кГц моно
const TAIL: Duration = Duration::from_millis(1000); // ~0,7 с ещё играет из буфера pw-cat
const RETRY: Duration = Duration::from_secs(10);

/// Вернуть фразу в голову очереди (после паузы — с начала предложения), если её не отменили (стоп, перебив) — пауза
/// не отменяет. false — фраза ушла. sounded — её звук уже начал писаться: знак второй раз не звучит.
fn requeue(shared: &Shared, q: &mut Queue, item: &Item, my_gen: u64, sounded: bool) -> bool {
    // образец не возвращается: на паузе pop отдал бы его снова
    let back = item.kind != Kind::Preview && (shared.generation.load(Ordering::SeqCst) == my_gen || q.paused());
    if back {
        q.push_front(Item { cue: item.cue.filter(|_| !sounded), ..item.clone() });
    }
    back
}

// ponytail: «синтез наперёд» — за счёт буфера канала pw-cat (~0,7 с): пока доигрывает хвост,
// синтезируется следующая фраза. Явный конвейер на 1–2 фразы — если паузы между фразами заметны.
// Замер 2026-11-12 (тест gap_between_sentences, от сети, нагрузка ~17): синтез 26–361 мс, пауз сверх звука Silero — 0 мс;
// с каналом 4 КиБ (PIPE_SZ=4096) — 22–119 мс, то есть замер паузы видит.
pub fn run(shared: Arc<Shared>, tts: Tts) {
    let player = Player::new(shared.sink.clone());
    run_with(shared, tts, player)
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
                            if q.paused() && it.kind != Kind::Preview {
                                q.push_front(Item { cue: None, ..it }); // после паузы — с начала предложения, знак уже звучал
                            }
                        } else {
                            player.close(); // хвост доигран, новой фразы нет
                        }
                    }
                }
                if let Some(it) = q.pop() {
                    tail = None;
                    shared.voice_now.store(it.voice, Ordering::SeqCst);
                    break (it, shared.generation.load(Ordering::SeqCst));
                }
                // фраза записана, оборвана или вернулась в очередь — у потока её нет; до «смолкла», чтобы строка
                // состояния с speaking false уже считала реплику отработанной
                shared.voice_now.store(0, Ordering::SeqCst);
                shared.idle(|| player.left());
                let to = if tail.is_some() { Duration::from_millis(100) } else { Duration::from_millis(500) };
                q = shared.cv.wait_timeout(q, to).unwrap().0;
            }
        };
        shared.set_busy(Some(&item.session));
        let (speaker, rate) = shared.voice.lock().unwrap().clone();
        let pcm = match tts.synth(&item.text, &speaker, &rate, item.tone, &item.lead) {
            // знак — перед звуком фразы, в том же pw-cat; читать нечего — и знака нет
            Ok(pcm) => match item.cue {
                Some(c) if !pcm.is_empty() => [crate::audio::cue_pcm(c), pcm].concat(),
                _ => pcm,
            },
            Err(Failed::Text) => {
                // модель не берёт этот текст (причина — в журнале от silero_tts): повтор не поможет, очередь не держим;
                // пустой звук, а не close: pw-cat доигрывает прежнюю фразу, следующая — в него же, а не в новый поверх
                eprintln!("agent-speak: синтез не смог фразу, пропущена: {}", item.text);
                Vec::new()
            }
            Err(Failed::Down) => {
                // синтез упал: очередь ждёт, а не теряет фразу; закрытый pw-cat доигрывает свой канал
                let left = player.left();
                player.close();
                let mut q = shared.queue.lock().unwrap();
                if !requeue(&shared, &mut q, &item, my_gen, false) {
                    shared.signal(Play::Dropped(item.session));
                }
                shared.idle(|| left);
                let deadline = Instant::now() + RETRY;
                while shared.generation.load(Ordering::SeqCst) == my_gen {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    q = shared.cv.wait_timeout(q, left).unwrap().0;
                }
                continue;
            }
        };
        let (mut done, mut sounded, mut kept) = (true, false, false); // kept — вернулась в очередь
        for (i, chunk) in pcm.chunks(CHUNK).enumerate() {
            if shared.generation.load(Ordering::SeqCst) != my_gen {
                player.reset();
                kept = requeue(&shared, &mut shared.queue.lock().unwrap(), &item, my_gen, sounded);
                done = false;
                break;
            }
            if !player.write(chunk) {
                // не потерять молча, но только один раз подряд для той же фразы
                if write_failed.as_deref() != Some(&item.text) {
                    write_failed = Some(item.text.clone());
                    kept = requeue(&shared, &mut shared.queue.lock().unwrap(), &item, my_gen, sounded);
                }
                done = false;
                break;
            }
            if i == 0 {
                shared.signal(Play::Sound(item.session.clone()));
                sounded = true;
            }
        }
        if !sounded && !kept {
            shared.signal(Play::Dropped(item.session.clone())); // пустой звук (синтез не смог), запись, стоп, перебив
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

    struct FakeSynth(Arc<Mutex<u32>>, Failed, Arc<Mutex<Vec<(String, String)>>>); // сколько первых вызовов провалить и как; (tone, lead) вызовов
    impl Synth for FakeSynth {
        fn synth(&mut self, _: &str, _: &str, _: &str, tone: &str, lead: &str) -> Result<Vec<u8>, Failed> {
            self.2.lock().unwrap().push((tone.into(), lead.into()));
            let mut n = self.0.lock().unwrap();
            if *n > 0 {
                *n -= 1;
                return Err(self.1);
            }
            Ok(vec![0; CHUNK * 2])
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
        fn close(&mut self) {
            self.0.lock().unwrap().push("close");
        }
        fn left(&self) -> Duration {
            Duration::from_secs(2)
        }
    }

    type Events = Arc<Mutex<Vec<&'static str>>>;

    fn start(synth_fails: u32, write_fails: u32) -> (Arc<Shared>, Events) {
        start_with(Arc::new(Mutex::new(synth_fails)), Failed::Down, write_fails)
    }

    fn start_with(fails: Arc<Mutex<u32>>, how: Failed, write_fails: u32) -> (Arc<Shared>, Events) {
        let shared = Arc::new(Shared::new(Queue::new(), "x".into(), "m".into()));
        let ev: Events = Default::default();
        let (s, e) = (shared.clone(), ev.clone());
        std::thread::spawn(move || {
            run_with(s, FakeSynth(fails, how, Default::default()), FakeSink(e, Arc::new(Mutex::new(write_fails))))
        });
        (shared, ev)
    }

    fn item(text: &str, kind: Kind) -> Item {
        Item { session: "s".into(), text: text.into(), kind, voice: 0, cue: None, tone: "", lead: String::new() }
    }

    fn say(shared: &Shared) {
        let mut q = shared.queue.lock().unwrap();
        q.push(item("фраза", Kind::Read));
        shared.cv.notify_all();
    }

    /// Новость со знаком «итог»: знак (380 мс) и звук фразы (0,2 с) — 6 кусков по 0,1 с.
    fn say_cued(shared: &Shared) {
        let mut q = shared.queue.lock().unwrap();
        q.push(Item { cue: Some(crate::audio::Cue::Done), tone: "done", lead: "плуг".into(), ..item("фраза", Kind::News) });
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
        let shared = Shared::new(Queue::new(), "x".into(), "m".into());
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
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Play::Busy); // начал говорить
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Play::Sound("s".into())); // пошёл звук
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), Play::Busy); // замолчал
        assert!(!shared.busy.load(Ordering::SeqCst));
        assert_eq!(*shared.current.lock().unwrap(), "s");
    }

    #[test]
    fn sound_start_is_reported_per_phrase() {
        // busy загорается до синтеза; звук идёт с первым записанным куском — по нему «Ты тут?» отсчитывает окно ответа
        let (shared, ev) = start(0, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        *shared.changed.lock().unwrap() = Some(tx);
        shared.pause();
        say(&shared);
        say(&shared);
        shared.resume();
        wait(|| count(&ev, "write") == 4 && !shared.busy.load(Ordering::SeqCst));
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [Play::Busy, Play::Sound("s".into()), Play::Sound("s".into()), Play::Busy], "начала, звук двух фраз, смолкла");
    }

    #[test]
    fn oma_voice_is_left_until_its_sound_is_written() {
        // реплика Омы (voice — её номер) не отработана, пока её фраза в очереди или у потока воспроизведения
        let (shared, ev) = start(0, 0);
        shared.pause();
        shared.queue.lock().unwrap().push(Item { session: "voice".into(), voice: 1, ..item("ответ", Kind::Voice) });
        assert_eq!(shared.voices_left(0), 1, "в очереди");
        assert_eq!(shared.voices_left(1), 0, "номер не новее after — реплика прежней Омы — не считается");
        shared.resume();
        wait(|| count(&ev, "write") == 2 && !shared.busy.load(Ordering::SeqCst));
        // busy гаснет после того, как поток отпустил реплику: строка с speaking false её уже считает
        assert_eq!(shared.voices_left(0), 0);
    }

    #[test]
    fn cue_is_glued_before_the_sound_and_tone_reaches_synth() {
        let calls: Arc<Mutex<Vec<(String, String)>>> = Default::default();
        let shared = Arc::new(Shared::new(Queue::new(), "x".into(), "m".into()));
        let ev: Events = Default::default();
        let (s, e, c) = (shared.clone(), ev.clone(), calls.clone());
        std::thread::spawn(move || run_with(s, FakeSynth(Arc::new(Mutex::new(0)), Failed::Down, c), FakeSink(e, Arc::new(Mutex::new(0)))));
        say_cued(&shared);
        wait(|| count(&ev, "write") == 6 && !shared.busy.load(Ordering::SeqCst));
        assert_eq!(*calls.lock().unwrap(), [("done".to_string(), "плуг".to_string())]);
        say(&shared);
        wait(|| count(&ev, "write") == 8);
    }

    #[test]
    fn cue_not_replayed_after_pause_but_kept_when_synth_was_down() {
        let (shared, ev) = start(0, 0);
        say_cued(&shared);
        wait(|| count(&ev, "write") == 6);
        shared.pause(); // фраза доиграна и в хвосте: после паузы — с начала, но знак уже прозвучал
        wait(|| count(&ev, "reset") == 1);
        shared.resume();
        wait(|| count(&ev, "write") == 8);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(count(&ev, "write"), 8, "повтор — без знака");
        let fails = Arc::new(Mutex::new(1));
        let (shared, ev) = start_with(fails.clone(), Failed::Down, 0);
        say_cued(&shared);
        wait(|| *fails.lock().unwrap() == 0 && !shared.busy.load(Ordering::SeqCst) && !shared.queue.lock().unwrap().is_empty());
        shared.interrupt();
        wait(|| count(&ev, "write") == 6); // синтез лежал — звука не было, знак звучит с фразой
    }

    #[test]
    fn synth_failure_keeps_item_and_retries() {
        let fails = Arc::new(Mutex::new(1));
        let (shared, ev) = start_with(fails.clone(), Failed::Down, 0);
        say(&shared);
        wait(|| *fails.lock().unwrap() == 0 && !shared.busy.load(Ordering::SeqCst) && !shared.queue.lock().unwrap().is_empty());
        assert_eq!(count(&ev, "write"), 0); // ждёт 10 с, не выбросила
        assert_eq!(count(&ev, "close"), 1); // pw-cat не простаивает, пока синтез лежит
        shared.interrupt(); // будит ожидание
        wait(|| count(&ev, "write") == 2);
    }

    /// Сигналы потока основному циклу — в канал, как у службы.
    fn signals(shared: &Shared) -> std::sync::mpsc::Receiver<Play> {
        let (tx, rx) = std::sync::mpsc::channel();
        *shared.changed.lock().unwrap() = Some(tx);
        rx
    }

    /// Следующие n сигналов.
    fn next(rx: &std::sync::mpsc::Receiver<Play>, n: usize) -> Vec<Play> {
        (0..n).map(|_| rx.recv_timeout(Duration::from_secs(3)).expect("не дождались сигнала")).collect()
    }

    #[test]
    fn text_the_model_rejects_is_dropped_not_retried() {
        let fails = Arc::new(Mutex::new(1));
        let (shared, ev) = start_with(fails.clone(), Failed::Text, 0);
        let rx = signals(&shared);
        say(&shared);
        wait(|| *fails.lock().unwrap() == 0 && !shared.busy.load(Ordering::SeqCst));
        assert!(shared.queue.lock().unwrap().is_empty(), "повтор того же текста не поможет — очередь не стоит");
        // фраза ушла без звука — служба узнаёт это из сигнала, а не по своему состоянию («Ты тут?» не ждёт ответа)
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [Play::Busy, Play::Dropped("s".into()), Play::Busy]);
        say(&shared); // следующая фраза звучит сразу, без паузы RETRY
        wait(|| count(&ev, "write") == 2);
        // пустой звук — как доигранная фраза: pw-cat не закрыт посреди хвоста прежней, новый поверх не запущен
        assert_eq!(ev.lock().unwrap()[..2], ["write", "write"]);
        wait(|| count(&ev, "close") == 1); // хвост доигран — закрыт, как после любой фразы
    }

    #[test]
    fn end_of_sound_is_when_the_buffer_plays_out() {
        // «смолкла» приходит, когда записан последний кусок; звук кончается позже — когда pw-cat доиграет канал (FakeSink: 2 с)
        let (shared, ev) = start(0, 0);
        let before = mono();
        say(&shared);
        wait(|| count(&ev, "write") == 2 && !shared.busy.load(Ordering::SeqCst));
        let at = *shared.played.lock().unwrap();
        assert!(at >= before + 2.0 && at <= mono() + 2.0, "конец звука {at}, а сейчас {}", mono());
    }

    #[test]
    fn silence_after_tail_closes_player() {
        let (shared, ev) = start(0, 0);
        say(&shared);
        wait(|| count(&ev, "close") == 1);
        assert_eq!(count(&ev, "reset"), 0); // не обрывает, а даёт доиграть
        say(&shared); // следующая фраза снова пишет
        wait(|| count(&ev, "write") == 4);
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
    fn preview_not_replayed_after_pause() {
        let (shared, ev) = start(0, 0);
        shared.queue.lock().unwrap().push(item("образец", Kind::Preview));
        shared.cv.notify_all();
        wait(|| count(&ev, "write") == 2);
        shared.pause();
        wait(|| count(&ev, "reset") == 1);
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(count(&ev, "write"), 2);
        assert!(shared.queue.lock().unwrap().is_empty());
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
        let rx = signals(&shared);
        say(&shared);
        wait(|| count(&ev, "write") == 2 && !shared.busy.load(Ordering::SeqCst));
        assert!(!rx.try_iter().any(|p| matches!(p, Play::Dropped(_))), "вернулась в очередь и прозвучала");
    }

    #[test]
    fn write_failing_twice_drops_without_sound() {
        let (shared, ev) = start(0, 2);
        let rx = signals(&shared);
        say(&shared);
        assert_eq!(next(&rx, 3), [Play::Busy, Play::Dropped("s".into()), Play::Busy]);
        assert_eq!(count(&ev, "write"), 0);
        assert!(shared.queue.lock().unwrap().is_empty());
    }

    /// Синтез, который идёт 0,5 с: за это время успеть оборвать фразу.
    struct SlowSynth;
    impl Synth for SlowSynth {
        fn synth(&mut self, _: &str, _: &str, _: &str, _: &str, _: &str) -> Result<Vec<u8>, Failed> {
            std::thread::sleep(Duration::from_millis(500));
            Ok(vec![0; CHUNK * 2])
        }
    }

    #[test]
    fn stop_before_first_chunk_drops_without_sound_and_pause_keeps() {
        let shared = Arc::new(Shared::new(Queue::new(), "x".into(), "m".into()));
        let ev: Events = Default::default();
        let (s, e) = (shared.clone(), ev.clone());
        std::thread::spawn(move || run_with(s, SlowSynth, FakeSink(e, Arc::new(Mutex::new(0)))));
        let rx = signals(&shared);
        say(&shared);
        wait(|| shared.busy.load(Ordering::SeqCst));
        shared.pause(); // пауза — фраза вернётся в очередь: не выброшена
        wait(|| !shared.busy.load(Ordering::SeqCst) && !shared.queue.lock().unwrap().is_empty());
        shared.resume();
        wait(|| shared.busy.load(Ordering::SeqCst));
        shared.stop(); // стоп — ушла без звука
        assert_eq!(next(&rx, 5), [Play::Busy, Play::Busy, Play::Busy, Play::Dropped("s".into()), Play::Busy]);
        assert_eq!(count(&ev, "write"), 0);
    }

    #[test]
    #[ignore] // замер с моделью: cargo test --release gap_between_sentences -- --ignored --nocapture
    fn gap_between_sentences() {
        use std::io::{Read, Write};
        const SAY: [&str; 8] = [
            "Готово.",
            "Агент в проекте агент спик закончил правку чисел и запустил тесты.",
            "Да.",
            "Все триста девяносто два теста прошли, ошибок нет, предупреждений тоже нет.",
            "Нет.",
            "Следующий шаг — проверить звук на наушниках, а потом закоммитить изменения в отдельную ветку и показать тебе разницу.",
            "Смотрю агентов.",
            "Второй агент ждёт ответа на вопрос про порядок миграций в базе данных.",
        ];
        let data = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".local/share/agent-speak");
        let sock = std::env::temp_dir().join(format!("tts-gap-{}.sock", std::process::id()));
        let mut tts = Tts::new(
            data.join("venv/bin/python"),
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("python/silero_tts.py"),
            data.join(crate::tts::MODEL),
            sock.clone(),
        );
        tts.synth("Проверка связи.", "xenia", "medium", "", "").unwrap(); // модель загружена и прогрета
        for s in SAY {
            let t = Instant::now();
            let n = tts.synth(s, "xenia", "medium", "", "").unwrap().len();
            eprintln!("синтез {:>4} мс, звук {:>4} мс: {s}", t.elapsed().as_millis(), n * 1000 / 96000);
        }
        struct PipeSink(Option<std::io::PipeWriter>);
        impl Sink for PipeSink {
            fn write(&mut self, pcm: &[u8]) -> bool {
                self.0.as_mut().is_some_and(|w| w.write_all(pcm).is_ok())
            }
            fn reset(&mut self) {}
            fn close(&mut self) {
                self.0 = None;
            }
            fn left(&self) -> Duration {
                Duration::ZERO
            }
        }
        let (mut r, w) = std::io::pipe().unwrap();
        if let Ok(sz) = std::env::var("PIPE_SZ") {
            use std::os::fd::AsRawFd;
            assert!(unsafe { libc::fcntl(w.as_raw_fd(), libc::F_SETPIPE_SZ, sz.parse::<i32>().unwrap()) } > 0);
        }
        // как pw-cat: канал 64 КиБ, читает в реальном времени и забирает наперёд не больше 100 мс
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 960];
            let mut end: Option<Instant> = None;
            let mut gaps = Vec::new();
            loop {
                let n = r.read(&mut buf).unwrap();
                if n == 0 {
                    return gaps;
                }
                let now = Instant::now();
                let e = match end {
                    Some(e) if e >= now => e,
                    Some(e) => {
                        gaps.push(now - e);
                        now
                    }
                    None => now,
                } + Duration::from_secs_f64(n as f64 / 96000.0);
                end = Some(e);
                let ahead = e.saturating_duration_since(Instant::now());
                if ahead > Duration::from_millis(100) {
                    std::thread::sleep(ahead - Duration::from_millis(100));
                }
            }
        });
        let shared = Arc::new(Shared::new(Queue::new(), "xenia".into(), "medium".into()));
        for s in SAY {
            shared.queue.lock().unwrap().push(Item { session: "s".into(), text: s.into(), kind: Kind::Read, voice: 0, cue: None, tone: "", lead: String::new() });
        }
        let s = shared.clone();
        std::thread::spawn(move || run_with(s, tts, PipeSink(Some(w))));
        let gaps: Vec<u128> = reader.join().unwrap().into_iter().filter(|g| *g > Duration::from_millis(5)).map(|g| g.as_millis()).collect();
        eprintln!("паузы сверх звука Silero, мс: {gaps:?}, всего {}", gaps.iter().sum::<u128>());
        let _ = std::process::Command::new("pkill").arg("-f").arg(&sock).status(); // сервер синтеза остался у потока speaker
    }
}
