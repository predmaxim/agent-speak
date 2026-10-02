pub fn notify(title: &str, body: &str) {
    let mut cmd = std::process::Command::new("notify-send");
    cmd.args(["-t", "2500", "-a", "Озвучка", title, body]);
    // ждём в отдельном потоке, чтобы не плодить зомби
    std::thread::spawn(move || {
        let _ = cmd.status();
    });
}
