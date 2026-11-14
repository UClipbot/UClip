#!/usr/bin/python3
"""Linux native WebKit smoke test. Uses only disposable data and a private session bus.
Requires tauri-driver, WebKitWebDriver, X11 and the zh_CN.UTF-8 locale. Clipboard fixtures are opt-in and isolated.
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
    parser.add_argument('--language-only', action='store_true', help='Native language forms, cross-window updates, errors and restart persistence.')
    parser.add_argument('--manual-only',action='store_true',help='Isolated real manual text creation.')
    parser.add_argument('--titlebar-only',action='store_true',help='Isolated native/custom titlebar and window controls.')
    parser.add_argument('--settings-only',action='store_true',help='Independent settings window, IPC grants and integrated main chrome.')
    parser.add_argument('--source-only',action='store_true',help='Real Google Chrome copy source attribution on isolated X11.')
    parser.add_argument('--desktop-defaults-only',action='store_true',help='Fresh desktop defaults and one-time OS autostart, with optional failure fixture.')
    parser.add_argument('--pinned-only',action='store_true',help='Isolated pin, pinned page and explicit deduplication checks.')
    parser.add_argument('--collections-only',action='store_true',help='Isolated real collection forms, transfers, query scopes and persistence.')
    parser.add_argument('--paste-performance',action='store_true',help='Twenty actual automatic paste receives per payload type.')
    parser.add_argument('--wake-only',action='store_true',help='Twenty isolated shortcut hide/wake cycles.')
    parser.add_argument('--startup-performance',action='store_true',help='Ten disposable process launches, no clipboard fixtures.')
    parser.add_argument('--wayland-restrictions', action='store_true', help='Read-only/restricted capability checks in isolated native Wayland.')
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
        # Existing fixtures address Chinese labels explicitly; do not depend on host language.
        env['LANGUAGE'] = 'zh_CN:zh'
        env['LC_ALL'] = 'zh_CN.UTF-8'
        for name, suffix in [('XDG_DATA_HOME', 'data'), ('XDG_CONFIG_HOME', 'config'), ('XDG_CACHE_HOME', 'cache')]:
            env[name] = str(Path(directory, suffix))
        autostart_failure = args.desktop_defaults_only and env.get('UCLIP_FIXTURE_AUTOSTART_FAILURE') == '1'
        if autostart_failure:
            Path(env['XDG_CONFIG_HOME']).mkdir()
            Path(env['XDG_CONFIG_HOME'], 'autostart').write_text('blocked fixture')
        with open(Path(directory, 'driver.log'), 'w+') as log:
            driver = subprocess.Popen(['dbus-run-session', '--', 'tauri-driver', '--port', str(port), '--native-port', str(native_port)], env=env, stdout=log, stderr=log, start_new_session=True)
            session = None
            clipboard_checks = None
            try:
                wait_for(lambda: request('GET', '/status'), 'WebDriver startup')

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


                launch_started=0.0
                def start():
                    nonlocal launch_started
                    launch_started=time.monotonic()
                    return request('POST', '/session', {'capabilities': {'alwaysMatch': {'tauri:options': {'application': binary}}}})['sessionId']

                session = start()

                def execute(script, arguments=None):
                    return request('POST', f'/session/{session}/execute/sync', {'script': script, 'args': arguments or []})

                def switch_window(label):
                    for handle in request('GET', f'/session/{session}/window/handles'):
                        request('POST', f'/session/{session}/window', {'handle': handle})
                        if execute('return window.__TAURI_INTERNALS__.metadata.currentWindow.label') == label:
                            return handle
                    raise RuntimeError('Missing native window: ' + label)

                def on_main(action, *arguments):
                    previous = request('GET', f'/session/{session}/window')
                    if execute('return window.__TAURI_INTERNALS__.metadata.currentWindow.label') == 'main':
                        return action(*arguments)
                    switch_window('main')
                    try:
                        return action(*arguments)
                    finally:
                        if previous in request('GET', f'/session/{session}/window/handles'):
                            request('POST', f'/session/{session}/window', {'handle': previous})

                def invoke_current(command, arguments=None):
                    return request('POST', f'/session/{session}/execute/async', {
                        'script': 'const done=arguments[arguments.length-1]; window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1]).then(value=>done({ok:true,value}),error=>done({ok:false,error}));',
                        'args': [command, arguments or {}],
                    })

                def invoke(command, arguments=None):
                    # Existing business fixtures run against the main window's ACL.
                    return on_main(invoke_current, command, arguments)

                def execute_main(script, arguments=None):
                    return on_main(execute, script, arguments)

                def element(selector):
                    value = request('POST', f'/session/{session}/element', {'using': 'css selector', 'value': selector})
                    return value['element-6066-11e4-a52e-4f735466cecf']

                def click(selector):
                    if execute('return document.body.classList.contains("settings-window")'):
                        page = None
                        if selector.startswith('#theme') or '使用系统原生标题栏' in selector:
                            page = 'appearance'
                        elif '修改' in selector or 'save-window-keys' in selector or '.about-section:has(' in selector:
                            page = 'keyboard'
                        if page:
                            request('POST', f'/session/{session}/element/{element("[data-settings-page=" + page + "]")}/click', {})
                        if selector.startswith('#theme'):
                            wait_for(lambda: execute('return !document.querySelector("#theme").disabled'), 'theme form ready')
                    if selector == '[data-testid="open-settings"]':
                        switch_window('main')
                        click('[aria-label="更多选项"]')
                    closing = request('GET', f'/session/{session}/window') if selector in ['[aria-label="关闭面板"]', '.modal-heading .icon-button'] and execute('return document.body.classList.contains("settings-window")') else None
                    request('POST', f'/session/{session}/element/{element(selector)}/click', {})
                    if selector in ['[data-testid="open-settings"]', '[data-testid="footer-settings"]']:
                        wait_for(lambda: len(request('GET', f'/session/{session}/window/handles')) == 2, 'settings window created')
                        switch_window('settings')
                        wait_for(lambda: execute('return !!document.querySelector("#theme") && !document.querySelector("#theme").disabled'), 'settings form ready')
                    if closing:
                        wait_for(lambda: closing not in request('GET', f'/session/{session}/window/handles'), 'settings window closed')
                        switch_window('main')

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
                if args.language_only:
                    from language_native_check import run, verify_restart
                    saved = run(invoke, invoke_current, execute, execute_main, click, wait_for, capture, directory, env)
                    assert invoke('quit_app')['ok']
                    wait_for(lambda: not owned_processes(), 'language test process exits')
                    try: request('DELETE', f'/session/{session}')
                    except (OSError, RuntimeError): pass
                    session = start()
                    wait_for(lambda: execute('return !!document.querySelector("#history-search")'), 'language restart UI')
                    verify_restart(invoke, execute, wait_for, saved)
                    return
                if args.settings_only:
                    from settings_native_check import run
                    run(invoke, invoke_current, execute, execute_main, click, switch_window, wait_for, capture, env, directory)
                    return
                if args.source_only:
                    from source_native_check import run
                    run(invoke, execute, env, log, wait_for)
                    return
                if args.desktop_defaults_only:
                    desktop = invoke('desktop_status')['value']
                    preferences = desktop['preferences']
                    assert preferences['closeToTray'] and not preferences['autoHide'] and not preferences['nativeTitlebar'], desktop
                    wait_for(lambda: execute('return !!document.querySelector(".window-titlebar")'), 'default custom titlebar')
                    directory_path = Path(env['XDG_CONFIG_HOME'], 'autostart')
                    if autostart_failure:
                        assert desktop['error']['code'] == 'restricted' and desktop['autostartEnabled'] is None, desktop
                        assert directory_path.read_text() == 'blocked fixture'
                        directory_path.unlink()
                        click('[data-testid="open-settings"]')
                        click('[data-settings-page="desktop"]')
                        wait_for(lambda: execute('return document.querySelector(".settings-feedback[role=status]")?.textContent.includes("当前平台或权限不支持此操作")'), 'default OS registration failure visible')
                        click('[aria-label="关闭面板"]')
                        assert invoke('set_autostart', {'enabled': False}) == {'ok': True, 'value': False}
                        assert invoke('desktop_status')['value']['error'] is None
                    else:
                        assert desktop['autostartEnabled'] and desktop['error'] is None, desktop
                        entries = list(directory_path.glob('*.desktop'))
                        assert len(entries) == 1 and binary in entries[0].read_text(), entries
                        assert invoke('set_autostart', {'enabled': False}) == {'ok': True, 'value': False}
                    assert invoke('quit_app')['ok']
                    wait_for(lambda: not owned_processes(), 'defaults process exits')
                    try: request('DELETE', f'/session/{session}')
                    except (OSError, RuntimeError): pass
                    session = start()
                    wait_for(lambda: execute('return !!document.querySelector("#history-search")'), 'defaults restart')
                    restored = invoke('desktop_status')['value']
                    assert restored['preferences'] == preferences and restored['autostartEnabled'] is False and restored['error'] is None, restored
                    assert not list(directory_path.glob('*.desktop')), 'startup retried or re-enabled disabled autostart'
                    assert invoke('set_autostart', {'enabled': True}) == {'ok': True, 'value': True}
                    assert invoke('desktop_status')['value']['error'] is None
                    assert invoke('set_autostart', {'enabled': False}) == {'ok': True, 'value': False}
                    print('PASS desktop defaults: resident/topmost/custom titlebar; OS autostart initialized once, failed registration visible, manual changes and restart respected', flush=True)
                    return
                if args.titlebar_only:
                    from titlebar_native_check import run, verify_restart
                    run(invoke,execute,click,env,directory,capture,execute_main,lambda name:on_main(capture,name))
                    for abrupt in [False,True]:
                        if abrupt:os.kill(owned_processes()[0][0],signal.SIGKILL)
                        else:click('[aria-label="关闭窗口"]')
                        wait_for(lambda:not owned_processes(),'custom close exits or crash terminates process')
                        try:request('DELETE',f'/session/{session}')
                        except (RuntimeError,OSError):pass
                        session=start()
                        wait_for(lambda:execute('return !!document.querySelector("#history-search")'),'titlebar restart')
                        verify_restart(invoke,execute,env)
                    print('PASS titlebar: custom close drains storage; preference and controls survive normal exit and SIGKILL',flush=True)
                    return
                if args.collections_only:
                    from clipboard_native_check import ClipboardChecks
                    from collections_native_check import run, verify_restart
                    clipboard_checks=ClipboardChecks(invoke,execute,click,enter,env,directory,log)
                    run(clipboard_checks,capture)
                    for abrupt in [False,True]:
                        if abrupt:os.kill(owned_processes()[0][0],signal.SIGKILL)
                        else:invoke('quit_app')
                        wait_for(lambda:not owned_processes(),'collection process termination')
                        try:request('DELETE',f'/session/{session}')
                        except (RuntimeError,OSError):pass
                        session=start()
                        wait_for(lambda:execute('return !!document.querySelector("#history-search")'),'collections restart')
                        verify_restart(clipboard_checks)
                    print('PASS: collections, membership and independent copies survive normal exit and SIGKILL/WAL recovery',flush=True)
                    return
                if args.pinned_only:
                    from clipboard_native_check import ClipboardChecks
                    from pinned_native_check import run, verify_restart
                    clipboard_checks=ClipboardChecks(invoke,execute,click,enter,env,directory,log)
                    run(clipboard_checks,capture)
                    for abrupt in [False,True]:
                        if abrupt:os.kill(owned_processes()[0][0],signal.SIGKILL)
                        else:invoke('quit_app')
                        wait_for(lambda:not owned_processes(),'pinned history process termination')
                        try:request('DELETE',f'/session/{session}')
                        except (RuntimeError,OSError):pass
                        session=start()
                        wait_for(lambda:execute('return !!document.querySelector("#history-search")'),'pinned history restart')
                        verify_restart(clipboard_checks)
                    print('PASS: pins and deduplication survive graceful exit and SIGKILL/WAL recovery',flush=True)
                    return
                if args.manual_only:
                    from clipboard_native_check import ClipboardChecks
                    from desktop_native_check import run_manual
                    clipboard_checks=ClipboardChecks(invoke,execute,click,enter,env,directory,log)
                    run_manual(clipboard_checks)
                    for abrupt in [False,True]:
                        if abrupt:os.kill(owned_processes()[0][0],signal.SIGKILL)
                        else:invoke('quit_app')
                        wait_for(lambda:not owned_processes(),'manual history process termination')
                        try:request('DELETE',f'/session/{session}')
                        except (RuntimeError,OSError):pass
                        session=start()
                        wait_for(lambda:execute('return !!document.querySelector("#history-search")'),'manual history restart')
                        assert clipboard_checks.query(clipboard_checks.manual_text)['total']==2
                        clipboard_checks.action('copy_clip',id=clipboard_checks.manual_id,paste=False)
                        clipboard_checks.received('text',clipboard_checks.manual_text)
                    print('PASS P1: manual identities and exact text survive graceful exit and SIGKILL/WAL recovery',flush=True)
                    return
                if args.paste_performance:
                    from clipboard_native_check import ClipboardChecks,wait
                    from experience_native_check import measure_continuous_paste
                    clipboard_checks=ClipboardChecks(invoke,execute,click,enter,env,directory,log)
                    clipboard_checks.policy(enabled=True)
                    for kind,fields in [('text',{'text':clipboard_checks.text}),('image',clipboard_checks.image),('files',{'files':clipboard_checks.files})]:
                        clipboard_checks.set(kind,**fields)
                        page=wait(lambda:clipboard_checks.query(kind=kind)['items'],'performance fixture capture')
                        clipboard_checks.ids[kind]=page[0]['id']
                    measure_continuous_paste(clipboard_checks)
                    return
                if args.wake_only:
                    from clipboard_native_check import ClipboardChecks
                    from experience_native_check import measure_wake,distribution
                    clipboard_checks=ClipboardChecks(invoke,execute,click,enter,env,directory,log)
                    receiver=subprocess.Popen(['/usr/bin/python3','scripts/paste-receiver.py',str(Path(directory)/'wake-receiver.json')],env=env,stdout=log,stderr=log)
                    clipboard_checks.processes.append(receiver)
                    clipboard_checks.activate(clipboard_checks.window('UClip native fixture receiver'))
                    print('WAKE PROBE '+json.dumps(distribution(measure_wake(clipboard_checks))),flush=True)
                    return
                if args.startup_performance:
                    times=[]
                    for iteration in range(10):
                        wait_for(lambda:execute('return document.hasFocus() && !!document.querySelector(".capture-toggle:not(:disabled)") && document.querySelector(".history-scroll")?.getAttribute("aria-busy")==="false"'),'interactive native startup')
                        times.append((time.monotonic()-launch_started)*1000)
                        invoke('quit_app')
                        wait_for(lambda:not owned_processes(),'benchmark process exit')
                        try:request('DELETE',f'/session/{session}')
                        except (RuntimeError,OSError):pass
                        session=None
                        if iteration<9:
                            session=start()
                            wait_for(lambda:execute('return !!document.querySelector("#history-search")'),'benchmark restart')
                    from experience_native_check import distribution
                    report={'processStartup':distribution(times),'firstMs':times[0],'notes':['new native process and WebView every sample','includes WebDriver session and 100ms observation overhead','OS page cache is not flushed; debug binary; empty isolated database']}
                    output=Path('docs/previews/copy-paste/performance-startup.json');output.parent.mkdir(parents=True,exist_ok=True);output.write_text(json.dumps(report,indent=2)+'\n')
                    print('PASS P2 process startup observations: '+json.dumps(report),flush=True)
                    assert report['processStartup']['p95Ms']<=2000,report
                    return
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
                click('#theme option[value="dark"]')
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
                if args.wayland_restrictions:
                    assert env.get('GDK_BACKEND')=='wayland' and not env.get('DISPLAY')
                    assert snapshot['value']['platform']['sessionHint']=='wayland'
                    status=invoke('clipboard_status')
                    assert status['ok'] and not status['value']['available'] and status['value']['error']['code']=='restricted',status
                    policy=status['value']['policy']
                    refused=invoke('update_capture_policy',{'request':{'enabled':True,'excludePattern':'','maxItems':500,'expectedRevision':policy['revision']}})
                    assert not refused['ok'] and refused['error']['code']=='restricted',refused
                    desktop=invoke('desktop_status')['value']
                    assert not desktop['shortcutAvailable']
                    preferences=desktop['preferences']
                    refused=invoke('update_desktop_preferences',{'request':{'closeToTray':True,'autoHide':False,'nativeTitlebar':preferences['nativeTitlebar'],'shortcut':preferences['shortcut'],'expectedRevision':preferences['revision']}})
                    assert not refused['ok'] and refused['error']['code']=='restricted',refused
                    assert invoke('desktop_status')['value']['preferences']==preferences
                    history=invoke('history_query',{'request':{'query':'中文','regex':False,'kind':None,'offset':0,'expectedRevision':None}})
                    assert history['ok'] and history['value']['total']==0,history
                    print('PASS Wayland: native GDK Wayland WebKit, no DISPLAY/XWayland; local history/theme work; capture/global shortcut/hide-without-recovery explicitly restricted',flush=True)
                    request('DELETE', f'/session/{session}')
                    session=None
                    return
                if args.clipboard_fixtures:
                    from clipboard_native_check import ClipboardChecks
                    clipboard_checks = ClipboardChecks(invoke, execute, click, enter, env, directory, log)
                    clipboard_checks.run()
                    capture('local-history')
                    from desktop_native_check import run as desktop_checks,run_manual,run_keyboard
                    run_manual(clipboard_checks)
                    run_keyboard(clipboard_checks)
                    desktop_checks(clipboard_checks)
                    from experience_native_check import run as experience_checks
                    experience_checks(clipboard_checks)

                saved_keyboard=invoke('desktop_status').get('value',{}).get('keyboard')
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
                desktop = invoke('desktop_status')['value']['preferences']
                if desktop['closeToTray']:
                    request_preferences = {key: value for key, value in desktop.items() if key != 'revision'}
                    assert invoke('update_desktop_preferences', {'request': {**request_preferences, 'closeToTray': False, 'expectedRevision': desktop['revision']}})['ok']
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
                assert invoke('desktop_status')['value']['keyboard']==saved_keyboard
                if clipboard_checks:
                    assert execute_main('return innerWidth===700 && innerHeight===600'), execute_main('return {width:innerWidth,height:innerHeight}')
                    print('PASS P2: restored native window size after process restart',flush=True)
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
                print('Native smoke completed; Windows/macOS remain untested. Native Wayland restrictions have a separate isolated check. Clipboard paths run only with --clipboard-fixtures.', flush=True)
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
