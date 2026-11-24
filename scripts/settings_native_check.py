"""Independent native settings and integrated main-window interaction checks."""
from pathlib import Path
import sqlite3
import subprocess
import time


def run(invoke, invoke_current, execute, execute_main, click, close_settings, switch_window, wait, capture, env, directory):
    wait(lambda: execute('return !!document.querySelector(".topbar:not([hidden]) #history-search")'), 'search shown by default')
    assert execute('const search=document.querySelector(".search-box").getBoundingClientRect(), filters=document.querySelector(".filter-bar").getBoundingClientRect();return !document.querySelector(".window-titlebar input") && search.right <= filters.left && Math.abs(search.top-filters.top) <= 1 && !!document.querySelector(".statusbar .capture-toggle")')
    assert execute('return document.querySelector(".filter-bar").getBoundingClientRect().height <= 36')

    # Search and filters must remain blocked while a main-window dialog is open.
    click('[aria-label="更多选项"]')
    click('[data-testid="new-text"]')
    assert execute('return document.querySelector(".app-shell").inert && !!document.querySelector(".app-shell #history-search")')
    click('[aria-label="关闭面板"]')
    assert execute('return !document.querySelector(".app-shell").inert')

    assert execute('return !document.querySelector(".filter-bar .app-more") && !document.querySelector(".listening-dot")')
    assert not invoke('set_hardware_acceleration', {'enabled': True})['ok'], 'main can only read the graphics setting'
    click('[data-testid="footer-settings"]')
    assert execute('return document.body.classList.contains("settings-window") && document.querySelector(".app-shell").hidden && !document.querySelector(".window-titlebar")')
    assert execute('return !document.querySelector(".modal-heading button")')
    capture('settings-window')
    assert invoke_current('clipboard_status')['ok']
    for page in ['desktop', 'keyboard', 'appearance', 'about', 'capture']:
        click(f'[data-settings-page="{page}"]')
        assert execute('return document.querySelector(".settings-nav [aria-current=page]").dataset.settingsPage === arguments[0]', [page])
        capture('settings-' + page)
    limit_input = '[aria-describedby="history-tab-capacity-hint"]'
    wait(lambda: execute('return document.querySelector(arguments[0])?.value === "30000"', [limit_input]), 'default local tab capacity')
    execute("""
        const input = document.querySelector(arguments[0]);
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "35000");
        input.dispatchEvent(new Event("input", { bubbles: true }));
    """, [limit_input])
    wait(lambda: execute('return document.querySelector(arguments[0]).value === "30000" && document.querySelector("#history-tab-capacity-hint").textContent.includes("已将每个标签页的上限调整")', [limit_input]), 'clamped capacity and visible feedback')
    previous = invoke_current('clipboard_status')['value']['policy']['revision']
    click('section:has(#history-tab-capacity-hint) button.secondary-button')
    wait(lambda: (policy := invoke_current('clipboard_status')['value']['policy'])['revision'] != previous and policy['maxItems'] == 30000, 'the clamped capacity is saved')
    click('[data-settings-page="desktop"]')
    capture('settings-desktop')
    for command, arguments in [
        ('history_query', {'request': {'query': '', 'regex': False, 'pinnedOnly': False, 'collectionId': None, 'kind': None, 'offset': 0, 'expectedRevision': None}}),
        ('copy_clip', {'id': 'not-readable', 'paste': False}),
        ('set_desktop_interaction', {'active': True}),
    ]:
        assert not invoke_current(command, arguments)['ok'], command + ' unexpectedly granted to settings'

    # Neither theme nor decoration changes may depend on refocusing the main WebView.
    for theme in ['light', 'dark']:
        click(f'#theme option[value="{theme}"]')
        wait(lambda: execute_main('return document.documentElement.dataset.theme === arguments[0]', [theme]), 'main theme refresh')
    for shown in [False, True]:
        wait(lambda: execute('return !document.querySelector("#show-search-by-default").disabled'), 'search setting ready')
        click('#show-search-by-default')
        wait(lambda: invoke('get_settings')['value']['showSearchByDefault'] == shown, 'saved search default')
        wait(lambda: execute_main('return !document.querySelector("#history-search-panel").hidden') == shown, 'main search toolbar refresh')
    started_with_acceleration = invoke_current('desktop_status')['value']['hardwareAcceleration']['enabledAtStartup']
    for enabled in [not started_with_acceleration, started_with_acceleration, not started_with_acceleration]:
        wait(lambda: execute('return !document.querySelector("#hardware-acceleration").disabled'), 'hardware acceleration setting ready')
        click('#hardware-acceleration')
        wait(lambda: invoke_current('desktop_status')['value']['hardwareAcceleration'] == {
            'enabled': enabled, 'enabledAtStartup': started_with_acceleration,
        }, 'hardware acceleration setting saved for the next launch')
        wait(lambda: execute('return document.querySelector("#hardware-acceleration").checked') == enabled, 'hardware acceleration switch reflects the saved setting')
        assert execute('return document.querySelector("#hardware-acceleration-hint").textContent.includes("请退出并重新打开")') == (enabled != started_with_acceleration)
    capture('settings-appearance-dark')
    for native in [True, False]:
        wait(lambda: execute('return !document.querySelector("[aria-label=使用系统原生标题栏]").disabled'), 'titlebar preference ready')
        click('[aria-label="使用系统原生标题栏"]')
        wait(lambda: invoke('desktop_status')['value']['preferences']['nativeTitlebar'] == native, 'saved decoration preference')
        wait(lambda: execute_main('return !!document.querySelector(".window-titlebar")') != native, 'main decoration refresh')

    close_settings()
    assert invoke('bootstrap')['ok'], 'closing settings must keep the core and main window alive'

    # Two simultaneous open requests must both succeed and use the same window.
    execute('window.__settingsOpenResults=null; Promise.allSettled([window.__TAURI_INTERNALS__.invoke("open_settings_window"), window.__TAURI_INTERNALS__.invoke("open_settings_window")]).then(r=>window.__settingsOpenResults=r.map(v=>v.status)); return true')
    results = wait(lambda: execute_main('return window.__settingsOpenResults'), 'concurrent settings creation')
    assert results == ['fulfilled', 'fulfilled'], results
    switch_window('settings')
    wait(lambda: execute('return !!document.querySelector("#theme")'), 'recreated settings UI')
    close_settings()
    switch_window('main')

    for theme in ['light', 'dark']:
        selector = '[aria-label="切换为' + ('浅色' if theme == 'light' else '深色') + '主题"]'
        wait(lambda: execute('return !document.querySelector(arguments[0]).disabled', [selector]), 'footer theme ready')
        click(selector)
        wait(lambda: execute('return document.documentElement.dataset.theme===arguments[0]', [theme]), 'footer theme applied')
    def pin(desired):
        wait(lambda: execute('return !document.querySelector(".footer-tools [aria-label=固定窗口]").disabled'), 'footer pin ready')
        click('.footer-tools [aria-label="固定窗口"]')
        wait(lambda: invoke('desktop_status')['value']['preferences']['autoHide'] == (not desired), 'footer pin preference saved')
        wait(lambda: execute('return document.querySelector(".footer-tools [aria-label=固定窗口]").getAttribute("aria-pressed") === arguments[0]', [str(desired).lower()]), 'footer pin reflects committed state')

    receiver = subprocess.Popen(['/usr/bin/python3', 'scripts/paste-receiver.py', str(Path(directory, 'footer-receiver.json'))], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        def receiver_window():
            result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', '^UClip native fixture receiver$'], env=env, text=True, capture_output=True)
            return result.stdout.splitlines()[-1] if result.returncode == 0 and result.stdout.strip() else None
        receiver_xid = wait(receiver_window, 'real external window for pin blur')
        main_xid = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--name', '^UClip$'], env=env, text=True).splitlines()[-1]
        subprocess.run(['xdotool', 'windowactivate', '--sync', main_xid], env=env, check=True)
        wait(lambda: execute('return document.hasFocus()'), 'main ready for pin toggle')
        # Pinning is the inverse of blur hiding; topmost must remain enabled in both states.
        assert not invoke('desktop_status')['value']['preferences']['autoHide']
        pin(False)
        assert invoke('desktop_status')['value']['preferences']['autoHide']
        assert '_NET_WM_STATE_ABOVE' in subprocess.check_output(['xprop', '-id', main_xid, '_NET_WM_STATE'], env=env, text=True)
        subprocess.run(['xdotool', 'windowactivate', '--sync', receiver_xid], env=env, check=True)
        wait(lambda: not execute('return document.hasFocus()'), 'main actually loses focus')
        def visible():
            return subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', '^UClip$'], env=env, capture_output=True).returncode == 0
        wait(lambda: not visible(), 'unpinned footer hides the native window on blur')
        subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+shift+v'], env=env, check=True)
        wait(lambda: visible() and execute('return document.hasFocus()'), 'shortcut recovers unpinned window')
        pin(True)
        subprocess.run(['xdotool', 'windowactivate', '--sync', receiver_xid], env=env, check=True)
        wait(lambda: not execute('return document.hasFocus()'), 'pinned main actually loses focus')
        time.sleep(.5)
        assert visible(), 'pinned window must remain visible after blur'
        xid = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--name', '^UClip$'], env=env, text=True).splitlines()[-1]
        assert '_NET_WM_STATE_ABOVE' in subprocess.check_output(['xprop', '-id', xid, '_NET_WM_STATE'], env=env, text=True)
        subprocess.run(['xdotool', 'windowactivate', '--sync', xid], env=env, check=True)
        wait(lambda: execute('return document.hasFocus()'), 'main refocused')

        # Failed unpin keeps both the saved state and actual native topmost behavior.
        before = invoke('desktop_status')['value']['preferences']
        database = next(Path(directory, 'data').rglob('foundation.sqlite3'))
        with sqlite3.connect(database) as connection:
            connection.execute("CREATE TRIGGER fixture_footer_pin_failure BEFORE UPDATE ON desktop_preferences BEGIN SELECT RAISE(ABORT,'fixture'); END")
        try:
            wait(lambda: execute('return !document.querySelector(".footer-tools [aria-label=固定窗口]").disabled'), 'pin failure fixture ready')
            click('.footer-tools [aria-label="固定窗口"]')
            wait(lambda: execute('return document.querySelector(".toast")?.textContent.includes("本地保存失败")'), 'unpin save failure feedback')
            assert invoke('desktop_status')['value']['preferences'] == before
            wait(lambda: execute('const e=document.querySelector(".footer-tools [aria-label=固定窗口]");return !e.disabled && e.getAttribute("aria-pressed")==="true"'), 'failed unpin remains highlighted')
            assert '_NET_WM_STATE_ABOVE' in subprocess.check_output(['xprop', '-id', xid, '_NET_WM_STATE'], env=env, text=True)
        finally:
            with sqlite3.connect(database) as connection:
                connection.execute('DROP TRIGGER fixture_footer_pin_failure')
    finally:
        receiver.terminate()
        try:
            receiver.wait(timeout=5)
        except subprocess.TimeoutExpired:
            receiver.kill()
            receiver.wait()

    status = invoke('clipboard_status')['value']
    if status['available']:
        wait(lambda: execute('return !document.querySelector(".capture-toggle").disabled'), 'listener ready')
        initially_enabled = status['policy']['enabled']
        for enabled in [not initially_enabled, initially_enabled]:
            click('.capture-toggle')
            wait(lambda: invoke('clipboard_status')['value']['policy']['enabled'] == enabled, 'listener policy commit')
            wait(lambda: execute('return !document.querySelector(".capture-toggle").disabled'), 'listener mutation settled')
    else:
        assert execute('return document.querySelector(".capture-toggle").disabled')
        print('LIMIT: capture unavailable in this display; listener disabled state verified', flush=True)
    for text in ['明天 10:00 确认窗口交互\n完成主窗搜索与独立设置窗的体验。', 'npm run tauri:dev', 'https://example.com/design-notes']:
        assert invoke('create_text', {'text': text, 'collectionId': None})['ok']
    wait(lambda: execute('return document.querySelectorAll(".clip-row").length === 3'), 'main history remains usable after settings close')
    capture('compact-main-window')
    from history_viewport_native_check import run as check_scroll
    check_scroll(invoke, execute, click, wait, capture)
    print('PASS settings window: search/filter row, main modal guard, separate settings UI, limited IPC, cross-window updates, concurrent open, close/reopen, pin blur/hide/recovery and failed-save rollback, listener state', flush=True)
    return not started_with_acceleration
