#!/usr/bin/python3
"""Linux native WebKit smoke test. Uses only disposable data and a private session bus.
Requires tauri-driver, WebKitWebDriver, and an X11 display; never reads the clipboard.
Build first: npm run tauri:build -- --debug
"""
import argparse
import base64
import json
import os
from pathlib import Path
import socket
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
                    capture('moss-dark')
                    click('[data-testid="open-settings"]')
                    click('#theme option[value="light"]')
                    wait_for(lambda: execute('return document.documentElement.dataset.theme === "light"'), 'light theme render')
                    click('[aria-label="关闭面板"]')
                    execute('document.querySelector(".toast .icon-button")?.click(); return true')
                    capture('moss-light')
                    click('[data-testid="open-settings"]')
                    click('#theme option[value="dark"]')
                    wait_for(lambda: execute('return document.documentElement.dataset.theme === "dark"'), 'dark theme restore')
                    capture('moss-settings')
                    click('[aria-label="关闭面板"]')
                    saved = invoke('get_settings')['value']

                enter('#history-search', '防抖')
                wait_for(lambda: execute('return document.querySelectorAll(".clip-row").length === 1 && document.querySelector(".record-title")?.textContent === "一个简单的防抖函数"'), 'live search and preview')
                click('[aria-label="正则表达式搜索"]')
                enter('#history-search', '[')
                wait_for(lambda: execute('return !!document.querySelector("#search-error")'), 'invalid regex feedback')
                click('[aria-label="正则表达式搜索"]')
                click('[aria-label="清空搜索"]')
                wait_for(lambda: execute('return document.querySelectorAll(".clip-row").length === 12'), 'search reset')
                click('.collection-nav button:nth-child(2)')
                wait_for(lambda: execute('return document.querySelectorAll(".clip-row").length === 4'), 'collection filter')
                click('.type-filters button:nth-child(4)')
                wait_for(lambda: execute('return document.querySelectorAll(".clip-row").length === 1 && document.querySelector(".record-title")?.textContent === "Somewhere, slower."'), 'type filter composition')
                execute('document.querySelector(".toast .icon-button")?.click(); return true')
                capture('moss-image')
                click('.primary-nav button:first-child')
                click('[data-clip-id="note-1"]')
                click('[aria-label="取消置顶"]')
                wait_for(lambda: execute('return document.querySelector(".primary-nav button:nth-child(2) .nav-count").textContent === "2"'), 'pin count update')
                click('[aria-label="置顶内容"]')
                click('[aria-label="网格视图"]')
                assert execute('return document.querySelector(".content-workspace").classList.contains("grid-mode")')
                click('[aria-label="列表视图"]')
                click('.new-button')
                assert execute('return document.activeElement.id') == 'clip-title'
                enter('#clip-title', '原生交互测试片段')
                enter('#clip-content', '只保存在此次内存预览，不写入业务数据库。')
                click('.new-form .primary-button')
                wait_for(lambda: execute('return document.querySelector(".record-title")?.textContent === "原生交互测试片段"'), 'create in-memory example')
                click('[aria-label="更多内容操作"]')
                click('.action-menu .destructive')
                click('.sidebar > .nav-item')
                wait_for(lambda: execute('return document.querySelectorAll(".clip-row").length === 1'), 'trash example')
                click('[aria-label="更多内容操作"]')
                click('.action-menu > button:last-child')
                wait_for(lambda: execute('return document.querySelectorAll(".clip-row").length === 0'), 'restore example')
                print('PASS: search/regex, collection/type filters, pinning, view modes, focus, create/delete/restore interactions', flush=True)

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
                print('PASS: normal window close drains storage; restart retains theme and revision', flush=True)

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
                print('Native smoke completed; Windows/macOS/Wayland and clipboard behavior remain untested.', flush=True)
            except Exception:
                log.flush()
                log.seek(0)
                print(log.read())
                raise
            finally:
                if session:
                    try:
                        request('DELETE', f'/session/{session}')
                    except (OSError, RuntimeError):
                        pass
                # Terminate only this smoke test's process group.
                import signal
                try:
                    os.killpg(driver.pid, signal.SIGTERM)
                    driver.wait(timeout=5)
                except (ProcessLookupError, subprocess.TimeoutExpired):
                    pass


if __name__ == '__main__':
    main()
