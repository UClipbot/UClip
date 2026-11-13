#!/usr/bin/python3
"""Linux native WebKit smoke test. Uses only disposable data and a private session bus.
Requires tauri-driver, WebKitWebDriver and X11. Clipboard fixtures are opt-in and isolated.
Build first: npm run tauri:build -- --debug
"""
import argparse
import base64
import json
import os
from pathlib import Path
import socket
import signal
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def wait_for(check, description, timeout=20):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except (OSError, RuntimeError, KeyError) as error:
            last = error
        time.sleep(0.1)
    raise RuntimeError(f'Timed out: {description}; {last}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default='target/debug/uclip-desktop')
    parser.add_argument('--screenshots', help='Optional folder for actual native UI PNG captures.')
    parser.add_argument('--clipboard-fixtures', action='store_true', help='Synthetic three-type and automatic paste checks; requires isolated-copy-paste.py.')
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve())
    if not Path(binary).is_file():
        raise RuntimeError('Build the desktop binary first.')
    port, native_port = free_port(), free_port()
    base = f'http://127.0.0.1:{port}'

    def request(method, path, payload=None):
        data = None if payload is None else json.dumps(payload).encode()
        req = urllib.request.Request(base + path, data=data, method=method, headers={'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(req, timeout=25) as response:
                result = json.load(response)
        except urllib.error.HTTPError as error:
            raise RuntimeError(error.read().decode()) from error
        value = result.get('value')
        if isinstance(value, dict) and 'ok' not in value and isinstance(value.get('error'), str):
            raise RuntimeError(str(value))
        return value

    with tempfile.TemporaryDirectory(prefix='uclip-native-') as directory:
        env = os.environ.copy()
        for name, suffix in [('XDG_DATA_HOME', 'data'), ('XDG_CONFIG_HOME', 'config'), ('XDG_CACHE_HOME', 'cache')]:
            env[name] = str(Path(directory, suffix))
        with open(Path(directory, 'driver.log'), 'w+') as log:
            driver = subprocess.Popen(['dbus-run-session', '--', 'tauri-driver', '--port', str(port), '--native-port', str(native_port)], env=env, stdout=log, stderr=log, start_new_session=True)
            session = None
            clipboard_checks = None
            try:
                wait_for(lambda: request('GET', '/status'), 'WebDriver startup')

                def start():
                    return request('POST', '/session', {'capabilities': {'alwaysMatch': {'tauri:options': {'application': binary}}}})['sessionId']

                session = start()

                def execute(script, arguments=None):
                    return request('POST', f'/session/{session}/execute/sync', {'script': script, 'args': arguments or []})

                def invoke(command, arguments=None):
                    return request('POST', f'/session/{session}/execute/async', {
                        'script': 'const done=arguments[arguments.length-1]; window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1]).then(value=>done({ok:true,value}),error=>done({ok:false,error}));',
                        'args': [command, arguments or {}],
                    })

                def element(selector):
                    value = request('POST', f'/session/{session}/element', {'using': 'css selector', 'value': selector})
                    return value['element-6066-11e4-a52e-4f735466cecf']

                def click(selector):
                    if selector == '[data-testid="open-settings"]':
                        click('[aria-label="更多选项"]')
                    request('POST', f'/session/{session}/element/{element(selector)}/click', {})

                def enter(selector, text):
                    key = element(selector)
                    request('POST', f'/session/{session}/element/{key}/clear', {})
                    if text:
                        request('POST', f'/session/{session}/element/{key}/value', {'text': text})

                def capture(name):
                    if args.screenshots:
                        target = Path(args.screenshots)
                        target.mkdir(parents=True, exist_ok=True)
                        time.sleep(0.2)
                        (target / f'{name}.png').write_bytes(base64.b64decode(request('GET', f'/session/{session}/screenshot')))

                wait_for(lambda: execute('return !!document.querySelector("#history-search")'), 'workspace UI ready')
                wait_for(lambda: execute('const e=document.querySelector(".app-more");return e && e.getBoundingClientRect().width>0 && innerWidth>0'), 'window layout ready')
                capture('startup')
                print('Initial UI:', execute('return {width:innerWidth,height:innerHeight,rect:document.querySelector(".app-more").getBoundingClientRect().toJSON(),inert:document.querySelector(".app-shell").inert}'), flush=True)
                click('[data-testid="open-settings"]')
                print('Native viewport:', execute('return {width:innerWidth,height:innerHeight,pixelRatio:devicePixelRatio}'), flush=True)
                wait_for(lambda: execute('return !!document.querySelector("#theme") && !document.querySelector("#theme").disabled'), 'native UI ready')
                snapshot = invoke('bootstrap')
                assert snapshot['ok'], snapshot
                assert snapshot['value']['settings']['revision'] == '0', snapshot
                assert snapshot['value']['platform']['os'] == 'linux', snapshot
                print('PASS: native WebKit startup, bootstrap ACL, fresh isolated settings', flush=True)

                # Real form interaction, not a mocked invoke or a direct database write.
                option = request('POST', f'/session/{session}/element', {'using': 'css selector', 'value': '#theme option[value="dark"]'})
                element_id = option['element-6066-11e4-a52e-4f735466cecf']
                request('POST', f'/session/{session}/element/{element_id}/click', {})
                wait_for(lambda: invoke('get_settings').get('value', {}).get('theme') == 'dark', 'theme commit from UI')
                wait_for(lambda: execute('return document.documentElement.dataset.theme === "dark"'), 'saved theme applied')
                saved = invoke('get_settings')['value']
                assert saved['revision'] == '1', saved
                print('PASS: native appearance form commits and renders saved state', flush=True)

                click('[aria-label="关闭面板"]')
                if args.screenshots:
                    execute('document.querySelector(".toast .icon-button")?.click(); return true')
                    capture('simple-dark')
                    click('[data-testid="open-settings"]')
                    click('#theme option[value="light"]')
                    wait_for(lambda: execute('return document.documentElement.dataset.theme === "light"'), 'light theme render')
                    click('[aria-label="关闭面板"]')
                    execute('document.querySelector(".toast .icon-button")?.click(); return true')
                    capture('simple-light')
                    click('[data-testid="open-settings"]')
                    click('#theme option[value="dark"]')
                    wait_for(lambda: execute('return document.documentElement.dataset.theme === "dark"'), 'dark theme restore')
                    capture('simple-settings')
                    click('[aria-label="关闭面板"]')
                    saved = invoke('get_settings')['value']

                # Native mode never falls back to in-memory examples.
                wait_for(lambda: execute('return document.querySelectorAll(".clip-row").length === 0 && !document.querySelector(".history-scroll").getAttribute("aria-busy").includes("true")'), 'empty real history')
                click('[aria-label="正则表达式搜索"]')
                enter('#history-search', '[')
                wait_for(lambda: execute('return !!document.querySelector("#search-error")'), 'native regex input validation')
                click('[aria-label="正则表达式搜索"]')
                click('[aria-label="清空搜索"]')
                print('PASS: real empty history never shows examples; native regex validation works', flush=True)
                if args.clipboard_fixtures:
                    from clipboard_native_check import ClipboardChecks
                    clipboard_checks = ClipboardChecks(invoke, execute, click, enter, env, directory, log)
                    clipboard_checks.run()
                    capture('local-history')

                stale = invoke('update_settings', {'request': {'theme': 'light', 'expectedRevision': '0'}})
                assert not stale['ok'] and stale['error']['code'] == 'conflict', stale
                assert not invoke('update_settings', {'request': {'theme': 'invalid', 'expectedRevision': '1'}})['ok']
                assert not invoke('plugin:window|set_title', {'label': 'main', 'title': 'unauthorized'})['ok']
                print('PASS: stale writes, invalid input and ungranted core command rejected', flush=True)

                before = execute('return location.href')
                execute('location.href="https://example.invalid/blocked"; return true')
                time.sleep(0.3)
                assert execute('return location.href') == before
                print('PASS: remote navigation blocked by native policy', flush=True)

                def owned_processes():
                    result = []
                    for entry in Path('/proc').iterdir():
                        if not entry.name.isdigit():
                            continue
                        try:
                            if str((entry / 'exe').resolve()) == binary:
                                values = dict(item.split('=', 1) for item in (entry / 'environ').read_bytes().decode().split('\0') if '=' in item)
                                if values.get('XDG_DATA_HOME') == env['XDG_DATA_HOME']:
                                    result.append((int(entry.name), values))
                        except (OSError, ValueError):
                            pass
                    return result

                owned = owned_processes()
                assert len(owned) == 1, [value[0] for value in owned]
                pid, process_env = owned[0]
                second_env = env.copy()
                second_env['DBUS_SESSION_BUS_ADDRESS'] = process_env['DBUS_SESSION_BUS_ADDRESS']
                second = subprocess.run([binary], env=second_env, stdout=log, stderr=log, timeout=10)
                assert second.returncode == 0
                assert [value[0] for value in owned_processes()] == [pid]
                print('PASS: second launch reuses the existing desktop instance', flush=True)

                # Send an ordinary window-manager close request to this test's exact PID.
                window_ids = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--pid', str(pid)], text=True).split()
                assert window_ids
                subprocess.run(['wmctrl', '-ic', hex(int(window_ids[0]))], check=True)
                wait_for(lambda: not owned_processes(), 'graceful desktop exit')
                try:
                    request('DELETE', f'/session/{session}')
                except (OSError, RuntimeError):
                    pass
                session = None
                session = start()
                wait_for(lambda: execute('return !!document.querySelector("#history-search")'), 'restarted workspace ready')
                click('[data-testid="open-settings"]')
                wait_for(lambda: execute('return !!document.querySelector("#theme") && !document.querySelector("#theme").disabled'), 'restarted UI ready')
                assert invoke('get_settings')['value'] == saved
                assert execute('return document.documentElement.dataset.theme') == 'dark'
                click('[aria-label="关闭面板"]')
                if clipboard_checks:
                    clipboard_checks.replay()
                print('PASS: normal window close drains storage; restart retains theme and revision', flush=True)
                if clipboard_checks:
                    main_pid = owned_processes()[0][0]
                    os.kill(main_pid, signal.SIGKILL)
                    wait_for(lambda: not owned_processes(), 'abrupt process termination')
                    try: request('DELETE', f'/session/{session}')
                    except RuntimeError: pass
                    session = start()
                    wait_for(lambda: execute('return !!document.querySelector("#history-search")'), 'crash recovery workspace')
                    clipboard_checks.replay()
                    assert invoke('get_settings')['value'] == saved
                    print('PASS A: acknowledged text/image/files and settings survive SIGKILL/WAL recovery', flush=True)


                request('DELETE', f'/session/{session}')
                session = None
                wait_for(lambda: not owned_processes(), 'restart fixture cleanup')
                databases = list(Path(directory, 'data').rglob('foundation.sqlite3'))
                assert len(databases) == 1
                with sqlite3.connect(databases[0]) as database:
                    database.execute('PRAGMA user_version = 999')
                session = start()
                wait_for(lambda: execute('return !!document.querySelector("[data-testid=service-error]")'), 'visible startup error')
                newer = invoke('bootstrap')
                assert not newer['ok'] and newer['error']['code'] == 'unsupported_schema', newer
                with sqlite3.connect(databases[0]) as database:
                    assert database.execute('PRAGMA user_version').fetchone()[0] == 999
                    assert database.execute('SELECT theme, revision FROM settings').fetchone() == ('dark', int(saved['revision']))
                print('PASS: newer schema shows a safe startup error without resetting saved data', flush=True)
                print('Native smoke completed; Windows/macOS/Wayland remain untested. Clipboard paths run only with --clipboard-fixtures.', flush=True)
            except Exception:
                if session:
                    try: capture('failure')
                    except Exception: pass
                log.flush()
                log.seek(0)
                print(log.read())
                raise
            finally:
                if clipboard_checks:
                    clipboard_checks.close()
                if session:
                    try:
                        request('DELETE', f'/session/{session}')
                    except (OSError, RuntimeError):
                        pass
                # Terminate only this smoke test's process group.
                try:
                    os.killpg(driver.pid, signal.SIGTERM)
                    driver.wait(timeout=5)
                except (ProcessLookupError, subprocess.TimeoutExpired):
                    pass


if __name__ == '__main__':
    main()
