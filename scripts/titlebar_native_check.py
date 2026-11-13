"""Real window controls and decoration checks on the disposable X11 display."""
import re
import sqlite3
import subprocess
import time
from pathlib import Path
from clipboard_native_check import wait


def run(invoke, execute, click, env, directory, capture):
    if env.get('UCLIP_FIXTURE_DISPLAY') != '1' or env.get('DISPLAY') in (None, ':0', ':0.0'):
        raise RuntimeError('Titlebar checks require an isolated display.')

    def action(command, **args):
        result = invoke(command, args)
        assert result['ok'], result
        return result['value']

    def output(*args):
        return subprocess.check_output(args, env=env, text=True).strip()

    def window():
        return output('xdotool', 'search', '--onlyvisible', '--name', '^UClip$').splitlines()[-1]

    def geometry():
        return {key: int(value) for key, value in (line.split('=', 1) for line in output('xdotool', 'getwindowgeometry', '--shell', window()).splitlines())}

    def layout():
        g = geometry()
        return execute('return [innerWidth,innerHeight]') == [g['WIDTH'], g['HEIGHT']]

    def decorations(native):
        text = output('xprop', '-id', window(), '_MOTIF_WM_HINTS')
        values = re.findall(r'0x[0-9a-f]+', text)
        return len(values) >= 3 and int(values[0], 16) & 2 and bool(int(values[2], 16)) == native

    def save(**changes):
        p = action('desktop_status')['preferences']
        request = {key: value for key, value in p.items() if key != 'revision'}
        return action('update_desktop_preferences', request={**request, **changes, 'expectedRevision': p['revision']})

    def settings():
        click('[data-testid="open-settings"]')
        wait(lambda: execute('const e=document.querySelector(arguments[0]); return e && !e.disabled', ['[aria-label="使用系统原生标题栏"]']), 'titlebar setting ready')

    initial = action('desktop_status')
    assert initial['preferences']['closeToTray'] and initial['preferences']['alwaysOnTop'], initial
    assert initial['autostartEnabled'] and not initial['preferences']['nativeTitlebar'], initial
    wait(lambda: execute('return !!document.querySelector(".window-titlebar")'), 'custom titlebar by default')
    wait(lambda: decorations(False), 'custom native window by default')
    wait(lambda: '_NET_WM_STATE_ABOVE' in output('xprop', '-id', window(), '_NET_WM_STATE'), 'topmost by default')
    settings()
    assert execute('const rows=Array.from(document.querySelectorAll(".setting-row")).filter(e=>["关闭窗口后常驻","窗口置顶","登录时启动"].some(t=>e.textContent.includes(t)));return rows.length===3 && rows.every(e=>e.querySelector("input").checked)')
    click('[aria-label="使用系统原生标题栏"]')
    wait(lambda: action('desktop_status')['preferences']['nativeTitlebar'], 'native titlebar can be enabled')
    wait(lambda: decorations(True), 'native decorations can be enabled')
    wait(lambda: not execute('return !!document.querySelector(".window-titlebar")'), 'custom controls removed')
    wait(lambda: execute('return !document.querySelector(arguments[0]).disabled', ['[aria-label="使用系统原生标题栏"]']), 'native setting settled')
    click('[aria-label="使用系统原生标题栏"]')
    wait(lambda: not action('desktop_status')['preferences']['nativeTitlebar'], 'custom titlebar committed')
    wait(lambda: execute('return !!document.querySelector(".window-titlebar")'), 'custom controls rendered')
    wait(lambda: decorations(False), 'OS decorations removed')
    for theme in ('light', 'dark'):
        click(f'#theme option[value="{theme}"]')
        wait(lambda: execute('return document.documentElement.dataset.theme===arguments[0]', [theme]), theme + ' titlebar theme')
        capture('titlebar-' + theme)

    # Stale writes cannot apply a window side effect; failed storage restores the real decorations.
    before = action('desktop_status')['preferences']
    request = {key: value for key, value in before.items() if key != 'revision'}
    failed = invoke('update_desktop_preferences', {'request': {**request, 'nativeTitlebar': True, 'expectedRevision': str(int(before['revision']) - 1)}})
    assert not failed['ok'] and failed['error']['code'] == 'conflict', failed
    assert action('desktop_status')['preferences'] == before and decorations(False)
    database = next(Path(directory, 'data').rglob('foundation.sqlite3'))
    with sqlite3.connect(database) as connection:
        connection.execute("CREATE TRIGGER fixture_titlebar_failure BEFORE UPDATE ON desktop_preferences BEGIN SELECT RAISE(ABORT,'fixture'); END")
    try:
        click('[aria-label="使用系统原生标题栏"]')
        wait(lambda: execute('return document.querySelector(".toast")?.textContent.includes("本地保存失败")'), 'titlebar save failure feedback')
        assert action('desktop_status')['preferences'] == before
        wait(lambda: decorations(False), 'failed save restores undecorated window')
        wait(lambda: execute('const e=document.querySelector(arguments[0]); return !e.disabled && !e.checked', ['[aria-label="使用系统原生标题栏"]']), 'failed save retains custom preference')
    finally:
        with sqlite3.connect(database) as connection:
            connection.execute('DROP TRIGGER fixture_titlebar_failure')

    # Window controls remain usable while the settings modal is open.
    wait(layout, 'custom window layout settled')
    click('[aria-label="最大化或还原窗口"]')
    wait(lambda: '_NET_WM_STATE_MAXIMIZED_VERT' in output('xprop', '-id', window(), '_NET_WM_STATE'), 'custom maximize button')
    wait(layout, 'maximized WebView layout settled')
    click('[aria-label="最大化或还原窗口"]')
    wait(lambda: '_NET_WM_STATE_MAXIMIZED_VERT' not in output('xprop', '-id', window(), '_NET_WM_STATE'), 'custom restore button')
    wait(layout, 'restored WebView layout settled')
    xid = window()
    click('[aria-label="最小化窗口"]')
    try:
        wait(lambda: '_NET_WM_STATE_HIDDEN' in output('xprop', '-id', xid, '_NET_WM_STATE'), 'custom minimize button')
    except RuntimeError:
        print('Minimize diagnostic:', output('xprop', '-id', xid, '_NET_WM_STATE', 'WM_STATE'), output('xwininfo', '-id', xid), execute('return {width:innerWidth,height:innerHeight,toast:document.querySelector(".toast")?.textContent}'), flush=True)
        raise
    subprocess.run(['xdotool', 'windowactivate', xid], env=env, check=True)
    wait(lambda: execute('return document.hasFocus()'), 'restore minimized window')
    click('[aria-label="关闭面板"]')

    # Exercise an actual pointer drag, including the native start-drag command.
    def position():
        values = geometry()
        return values['X'], values['Y']
    x, y = position()
    subprocess.run(['xdotool', 'mousemove', str(x + 140), str(y + 17), 'mousedown', '1'], env=env, check=True)
    try:
        time.sleep(.15)
        subprocess.run(['xdotool', 'mousemove', str(x + 220), str(y + 72)], env=env, check=True)
        time.sleep(.15)
    finally:
        subprocess.run(['xdotool', 'mouseup', '1'], env=env, check=True)
    wait(lambda: position() != (x, y), 'custom titlebar moves the native window')
    x, y = position()
    subprocess.run(['xdotool', 'mousemove', str(x + 140), str(y + 17), 'click', '--repeat', '2', '--delay', '100', '1'], env=env, check=True)
    wait(lambda: '_NET_WM_STATE_MAXIMIZED_VERT' in output('xprop', '-id', window(), '_NET_WM_STATE'), 'titlebar double-click maximizes')
    wait(layout, 'double-click maximized layout settled')
    click('[aria-label="最大化或还原窗口"]')
    wait(lambda: '_NET_WM_STATE_MAXIMIZED_VERT' not in output('xprop', '-id', window(), '_NET_WM_STATE'), 'restore after titlebar double-click')
    wait(layout, 'double-click restored layout settled')
    capture('titlebar-window')

    # Return to native and back using the setting, proving both directions update the OS.
    settings()
    click('[aria-label="使用系统原生标题栏"]')
    wait(lambda: action('desktop_status')['preferences']['nativeTitlebar'], 'native titlebar committed')
    wait(lambda: not execute('return !!document.querySelector(".window-titlebar")'), 'custom controls removed')
    wait(lambda: decorations(True), 'native OS decorations restored')
    wait(lambda: execute('return !document.querySelector(arguments[0]).disabled', ['[aria-label="使用系统原生标题栏"]']), 'native setting settled')
    click('[aria-label="使用系统原生标题栏"]')
    wait(lambda: execute('return !!document.querySelector(".window-titlebar")'), 'custom controls restored')
    click('[aria-label="关闭面板"]')
    save(closeToTray=True)
    xid = window()
    click('[aria-label="关闭窗口"]')
    wait(lambda: 'IsUnMapped' in output('xwininfo', '-id', xid), 'custom close respects stay resident')
    assert action('desktop_status')['preferences']['closeToTray']
    subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+shift+v'], env=env, check=True)
    wait(lambda: execute('return document.hasFocus()'), 'shortcut restores custom window')
    save(closeToTray=False)
    print('PASS titlebar: native/custom UI, real decorations, dark/light, conflict/failure rollback, maximize/restore/minimize/drag/double-click and close-to-tray recovery', flush=True)


def verify_restart(invoke, execute, env):
    result = invoke('desktop_status')
    assert result['ok'] and not result['value']['preferences']['nativeTitlebar'], result
    wait(lambda: execute('return !!document.querySelector(".window-titlebar")'), 'custom titlebar survives restart')
    xid = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--name', '^UClip$'], env=env, text=True).splitlines()[-1]
    hints = subprocess.check_output(['xprop', '-id', xid, '_MOTIF_WM_HINTS'], env=env, text=True)
    values = re.findall(r'0x[0-9a-f]+', hints)
    assert len(values) >= 3 and int(values[0], 16) & 2 and int(values[2], 16) == 0, hints
