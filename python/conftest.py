import pytest


@pytest.fixture(autouse=True)
def _no_desktop(monkeypatch):
    """Тесты не шлют уведомлений на стол пользователя: notify-send без сессионной шины молчит. 2026-11-12 ошибки
    поддельного мозга и слуха из тестов («'FakeBrain' object has no attribute 'send'», «pw-record не запустился»)
    сыпались в центр уведомлений: многие тесты создают Ому без своей заглушки notify."""
    monkeypatch.setenv("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/agent-speak-tests")
