"""Native WebKit language forms, rollback, limited IPC, and durable preference checks."""
from pathlib import Path
import sqlite3
import subprocess


def assert_collection_label(execute):
    execute('document.querySelector(".scope-control button").click()')
    try:
        assert execute('return [...document.querySelectorAll(".action-menu button")].some(e=>e.textContent===arguments[0])', ['\u8bbe\u7f6e'])
    finally:
        execute('document.querySelector(".scope-control button").click()')


def run(invoke, invoke_current, execute, execute_main, click, wait, capture, directory, env):
    print('Native system languages:', execute('return navigator.languages'), flush=True)
    initial = invoke('get_settings')['value']
    assert initial['language'] == 'system' and initial['revision'] == '0', initial
    wait(lambda: execute('return document.documentElement.lang === "zh-CN"'), 'system Chinese UI')
    text = '\u8bbe\u7f6e\nUntranslated user content {0}'
    assert invoke('create_text', {'text': text, 'collectionId': None})['ok']
    collection = invoke('create_collection', {'request': {'name': '\u8bbe\u7f6e'}})
    assert collection['ok'], collection
    click('[data-testid="footer-settings"]')
    click('[data-settings-page="appearance"]')
    wait(lambda: execute('return !document.querySelector("#language").disabled'), 'native language form')
    assert not invoke_current('set_ui_language', {'language': 'en'})['ok'], 'settings must not control resolved native locale'
    assert not invoke('set_ui_language', {'language': 'system'})['ok'], 'resolved locale must be explicit'

    def choose(language, locale, retry=False):
        wait(lambda: execute('return !document.querySelector("#language").disabled'), 'language save settled')
        if retry:
            # After a failed option selection, exercise the real native keyboard path.
            xid = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--name', '^UClip · Settings$'], env=env, text=True).splitlines()[-1]
            subprocess.run(['xdotool', 'windowactivate', '--sync', xid], env=env, check=True)
            execute('document.querySelector("#language").focus()')
            wait(lambda: execute('return document.hasFocus() && document.activeElement.id === "language"'), 'language keyboard focus')
            subprocess.run(['xdotool', 'key', '--clearmodifiers', 'Up'], env=env, check=True)
        else:
            click(f'#language option[value="{language}"]')
        wait(lambda: invoke('get_settings')['value']['language'] == language, 'language committed')
        wait(lambda: execute('return document.documentElement.lang === arguments[0]', [locale]), 'settings language applied')
        wait(lambda: execute_main('return document.documentElement.lang === arguments[0]', [locale]), 'main language refreshed without focus')

    choose('en', 'en')
    assert execute('return document.querySelector("#modal-title").textContent') == 'Settings'
    assert execute_main('return document.querySelector("#history-search").placeholder') == 'Search history…'
    assert execute_main('return document.querySelector(".clip-excerpt").textContent') == text
    assert_collection_label(execute_main)
    assert execute_main('return document.querySelector(".clip-row-meta").textContent.includes("Manually created")')
    assert subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', '^UClip · Settings$'], env=env, stdout=subprocess.DEVNULL).returncode == 0
    capture('language-english')

    database = next(Path(directory, 'data').rglob('foundation.sqlite3'))
    before = invoke('get_settings')['value']
    with sqlite3.connect(database) as connection:
        connection.execute("CREATE TRIGGER fixture_language_failure BEFORE UPDATE ON settings BEGIN SELECT RAISE(ABORT,'private fixture'); END")
    try:
        click('#language option[value="zh-CN"]')
        wait(lambda: execute('return document.querySelector("#language").closest("section").querySelector("[role=status]")?.textContent.includes("Could not save locally")'), 'English safe save failure')
        assert invoke('get_settings')['value'] == before
        assert execute('return document.documentElement.lang') == 'en'
        assert execute('return document.querySelector("#language").value') == 'en'
    finally:
        with sqlite3.connect(database) as connection:
            connection.execute('DROP TRIGGER fixture_language_failure')

    choose('zh-CN', 'zh-CN', retry=True)
    assert execute('return document.querySelector("#modal-title").textContent') == '\u8bbe\u7f6e'
    assert execute_main('return document.querySelector("#history-search").placeholder') == '\u641c\u7d22\u5386\u53f2…'
    assert subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', '^UClip · \u8bbe\u7f6e$'], env=env, stdout=subprocess.DEVNULL).returncode == 0
    capture('language-chinese')
    choose('system', 'zh-CN')
    choose('en', 'en')
    assert invoke('set_ui_language', {'language': 'en'})['ok']
    saved = invoke('get_settings')['value']
    print('PASS language: native form, system Chinese, English/Chinese switching, main/settings synchronization without focus, preserved user text and collection names, safe failed-save rollback, resolved-locale ACL', flush=True)
    click('.modal-heading .icon-button')
    return saved


def verify_restart(invoke, execute, wait, saved):
    wait(lambda: execute('return document.documentElement.lang === "en"'), 'saved language after process restart')
    assert invoke('get_settings')['value'] == saved
    assert execute('return document.querySelector("#history-search").placeholder') == 'Search history…'
    assert_collection_label(execute)
    print('PASS language: preference, revision and user content survive a native process restart', flush=True)
