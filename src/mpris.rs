//! Кнопка наушников (дизайн «Джарвис», headset-button): MPRIS-плеер `org.mpris.MediaPlayer2.oma` на сессионной шине,
//! пока Ома говорит или ждёт ответа на «Ты тут?». Кнопку выбирает оболочка Omarchy: `media.playPause` уходит самому
//! старому играющему плееру (plugins/services/media/Service.qml, playerForAction), поэтому плеер всегда «играет» и
//! виден только это время. Что значит нажатие, решает служба (daemon.rs, on_button); здесь только шина.
//! Своя шина — в своём потоке: он ждёт сокет шины и байт службы одним poll, без опроса по таймеру.

use dbus::arg::{PropMap, RefArg, Variant};
use dbus::blocking::Connection;
use dbus::channel::{BusType, Channel, MatchingReceiver};
use dbus::message::MatchRule;
use dbus_crossroads::{Crossroads, IfaceBuilder};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub const NAME: &str = "org.mpris.MediaPlayer2.oma";
const PATH: &str = "/org/mpris/MediaPlayer2";

/// Что нажали: pause, play/pause и stop — одно и то же (у плеера Омы нет состояния паузы); next — сводка. Play — ничего:
/// она и так «играет», а «возобнови» шлёт автоматика (конец созвона, `playerctl play`) — ни «замолчи», ни «я тут».
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Button {
    Toggle,
    Next,
}

type OnButton = Box<dyn Fn(Button) + Send>;

/// Сторона службы: виден ли плеер. Смена — один байт потоку шины.
pub struct Player {
    wake: UnixStream,
    pub on: bool,
}

impl Player {
    pub fn set(&mut self, on: bool) {
        if on != self.on {
            self.on = on;
            let _ = self.wake.write_all(&[on as u8]); // поток шины упал — кнопки нет, служба живёт (причина — в журнале)
        }
    }

    /// Без шины — для тестов службы: смены видны в `on`.
    #[cfg(test)]
    pub fn offline() -> Player {
        Player { wake: UnixStream::pair().unwrap().0, on: false }
    }
}

/// Плеер на сессионной шине пользователя; нажатия — в on_button (из потока шины).
pub fn spawn(on_button: impl Fn(Button) + Send + 'static) -> Player {
    spawn_on(|| Channel::get_private(BusType::Session), on_button)
}

fn spawn_on(bus: impl FnOnce() -> Result<Channel, dbus::Error> + Send + 'static, on_button: impl Fn(Button) + Send + 'static) -> Player {
    let (wake, rx) = UnixStream::pair().expect("socketpair");
    std::thread::spawn(move || {
        if let Err(e) = serve(bus, rx, Box::new(on_button)) {
            eprintln!("agent-speak: кнопка наушников недоступна: {e}");
        }
    });
    Player { wake, on: false }
}

fn serve(bus: impl FnOnce() -> Result<Channel, dbus::Error>, mut wake: UnixStream, on_button: OnButton) -> Result<(), Box<dyn std::error::Error>> {
    let mut ch = bus()?;
    ch.set_watch_enabled(true);
    let fd = ch.watch().fd;
    let conn = Connection::from(ch);
    let mut cr = Crossroads::new();
    // свойства, без которых Quickshell плеер не берёт (services/mpris/player.hpp, обязательные привязки), и корень по MPRIS
    let app = cr.register("org.mpris.MediaPlayer2", |b: &mut IfaceBuilder<OnButton>| {
        b.property("Identity").get(|_, _| Ok("Ома".to_string()));
        for p in ["CanQuit", "CanRaise", "HasTrackList"] {
            b.property(p).get(|_, _| Ok(false));
        }
        for p in ["SupportedUriSchemes", "SupportedMimeTypes"] {
            b.property(p).get(|_, _| Ok(Vec::<String>::new()));
        }
        for m in ["Raise", "Quit"] {
            b.method(m, (), (), |_, _, ()| Ok(()));
        }
    });
    let player = cr.register("org.mpris.MediaPlayer2.Player", |b: &mut IfaceBuilder<OnButton>| {
        b.property("PlaybackStatus").get(|_, _| Ok("Playing".to_string()));
        // трек «Ома»: пока она говорит, медиа-панель показывает её (без названия виджет пропадал бы), пауза там — «замолчи»
        b.property("Metadata").get(|_, _| {
            let mut m = PropMap::new();
            m.insert("mpris:trackid".into(), Variant(Box::new(dbus::Path::from("/org/mpris/MediaPlayer2/oma")) as Box<dyn RefArg>));
            m.insert("xesam:title".into(), Variant(Box::new("Ома".to_string()) as Box<dyn RefArg>));
            Ok(m)
        });
        for p in ["CanControl", "CanPlay", "CanPause", "CanGoNext"] {
            b.property(p).get(|_, _| Ok(true));
        }
        for p in ["CanGoPrevious", "CanSeek"] {
            b.property(p).get(|_, _| Ok(false));
        }
        // shortcut: Seek, SetPosition и OpenUri не объявлены (CanSeek false, схем нет) — по MPRIS вызывать их некому
        for (m, button) in [("PlayPause", Some(Button::Toggle)), ("Play", None), ("Pause", Some(Button::Toggle)),
            ("Stop", Some(Button::Toggle)), ("Next", Some(Button::Next)), ("Previous", None)]
        {
            b.method(m, (), (), move |_, on: &mut OnButton, ()| {
                if let Some(x) = button {
                    on(x);
                }
                Ok(())
            });
        }
    });
    cr.insert(PATH, &[app, player], on_button);
    conn.start_receive(MatchRule::new_method_call(), Box::new(move |msg, conn| {
        let _ = cr.handle_message(msg, conn);
        true
    }));
    let mut owned = false;
    loop {
        let mut fds = [libc::pollfd { fd, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        // SAFETY: два pollfd на стеке, длина — их число
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e.into());
        }
        if fds[1].revents != 0 {
            let mut b = [0u8; 64];
            let n = wake.read(&mut b)?;
            if n == 0 {
                return Ok(()); // служба ушла
            }
            let want = b[n - 1] == 1; // копились смены — важна последняя
            if want != owned {
                // имя на шине — и есть плеер для оболочки: появилось — она читает свойства, ушло — плеера нет
                let r = if want { conn.request_name(NAME, false, false, true).map(drop) } else { conn.release_name(NAME).map(drop) };
                if let Err(e) = r {
                    eprintln!("agent-speak: кнопка наушников: имя {NAME}: {e}");
                }
                owned = want;
            }
        }
        if fds[0].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            return Err("сессионная шина закрыла соединение".into());
        }
        // всё, что пришло (и пока ждали ответа шины на имя), — обработать сейчас: poll о прочитанном libdbus не скажет
        while conn.process(Duration::ZERO)? {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::Instant;

    struct Bus(Child);

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn player_is_on_the_bus_only_while_on_and_its_buttons_reach_the_service() {
        // своя шина (dbus-daemon), а не шина пользователя: оболочка не увидит плеер тестов
        let Ok(mut d) = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).spawn() else {
            eprintln!("нет dbus-daemon — тест шины пропущен");
            return;
        };
        let mut addr = String::new();
        std::io::BufReader::new(d.stdout.take().unwrap()).read_line(&mut addr).unwrap();
        let _bus = Bus(d);
        let addr = addr.trim().to_string();
        let (tx, rx) = mpsc::channel();
        let a = addr.clone();
        let mut p = spawn_on(
            move || {
                let mut c = Channel::open_private(&a)?;
                c.register()?;
                Ok(c)
            },
            move |b| {
                let _ = tx.send(b);
            },
        );
        let busctl = |args: &[&str]| {
            let o = Command::new("busctl").arg(format!("--address={addr}")).args(args).output().unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let owned = || busctl(&["call", "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner", "s", NAME]) == "b true";
        let wait = |want: bool| {
            let end = Instant::now() + Duration::from_secs(5);
            while owned() != want && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(20));
            }
            owned() == want
        };
        assert!(!owned(), "выключен — имени на шине нет");
        p.set(true);
        assert!(wait(true), "включён — плеер на шине");
        let player = "org.mpris.MediaPlayer2.Player";
        assert_eq!(busctl(&["get-property", NAME, PATH, player, "PlaybackStatus"]), "s \"Playing\"");
        assert_eq!(busctl(&["--json=short", "get-property", NAME, PATH, "org.mpris.MediaPlayer2", "Identity"]), r#"{"type":"s","data":"Ома"}"#);
        let all = busctl(&["call", NAME, PATH, "org.freedesktop.DBus.Properties", "GetAll", "s", player]);
        for p in ["PlaybackStatus", "Metadata", "CanControl", "CanPlay", "CanPause", "CanGoNext", "CanGoPrevious", "CanSeek"] {
            assert!(all.contains(p), "GetAll без {p}: {all}");
        }
        let meta = busctl(&["--json=short", "get-property", NAME, PATH, player, "Metadata"]);
        assert!(meta.contains(r#""xesam:title":{"type":"s","data":"Ома"}"#), "панель показывает её трек: {meta}");
        for (m, want) in [("PlayPause", Button::Toggle), ("Pause", Button::Toggle), ("Stop", Button::Toggle), ("Next", Button::Next)] {
            busctl(&["call", NAME, PATH, player, m]);
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(want), "{m}");
        }
        for m in ["Play", "Previous"] {
            busctl(&["call", NAME, PATH, player, m]);
            assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "{m} — ничего: «возобнови» автоматики не «замолчи»");
        }
        p.set(false);
        assert!(wait(false), "выключен — имени нет: кнопка снова у музыки");
        p.set(true);
        assert!(wait(true), "и снова");
    }
}
