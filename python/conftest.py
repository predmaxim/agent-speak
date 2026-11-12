import pytest


@pytest.fixture(autouse=True)
def _no_desktop(monkeypatch):
    """Тесты не шлют уведомлений на стол пользователя: notify-send без сессионной шины молчит. 2026-11-12 ошибки
    поддельного мозга и слуха из тестов («'FakeBrain' object has no attribute 'send'», «pw-record не запустился»)
    сыпались в центр уведомлений: многие тесты создают Ому без своей заглушки notify."""
    monkeypatch.setenv("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/agent-speak-tests")


@pytest.fixture(autouse=True)
def _no_ear_log(monkeypatch, tmp_path):
    """Диагностика слуха в тестах — выключена и не в живом каталоге: иначе тесты Ear и Омы писали бы свои фразы к
    настоящим записям пользователя и вытесняли их (держатся последние 10)."""
    from agent_voice import earlog
    monkeypatch.setattr(earlog, "DIR", str(tmp_path / "ear-off"))
    monkeypatch.setattr(earlog, "_checked", -10.0)
    monkeypatch.setattr(earlog, "_last", {})
    earlog.reset()
