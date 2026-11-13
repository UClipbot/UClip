"""Independent native settings and integrated main-window interaction checks."""


def run(invoke, invoke_current, execute, execute_main, click, switch_window, wait, capture):
    wait(lambda: execute('return !!document.querySelector(".window-titlebar #history-search")'), 'integrated search')
    assert execute('return !document.querySelector(".window-titlebar-brand").textContent.trim()')
    assert execute('return document.querySelector(".filter-bar").getBoundingClientRect().height <= 36')

    # A main-window dialog must block the newly interactive titlebar controls.
    click('[aria-label="更多选项"]')
    click('[data-testid="new-text"]')
    assert execute('return document.querySelector(".titlebar-search").inert && document.querySelector(".titlebar-listening").disabled')
    click('[aria-label="关闭面板"]')
    assert execute('return !document.querySelector(".titlebar-search").inert')

    click('[data-testid="open-settings"]')
    assert execute('return document.body.classList.contains("settings-window") && document.querySelector(".app-shell").hidden && !document.querySelector(".window-titlebar")')
    capture('settings-window')
    assert invoke_current('clipboard_status')['ok']
    for page in ['desktop', 'keyboard', 'appearance', 'about', 'capture']:
        click(f'[data-settings-page="{page}"]')
        assert execute('return document.querySelector(".settings-nav [aria-current=page]").dataset.settingsPage === arguments[0]', [page])
        capture('settings-' + page)
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
    capture('settings-appearance-dark')
    for native in [True, False]:
        wait(lambda: execute('return !document.querySelector("[aria-label=使用系统原生标题栏]").disabled'), 'titlebar preference ready')
        click('[aria-label="使用系统原生标题栏"]')
        wait(lambda: invoke('desktop_status')['value']['preferences']['nativeTitlebar'] == native, 'saved decoration preference')
        wait(lambda: execute_main('return !!document.querySelector(".window-titlebar")') != native, 'main decoration refresh')

    click('[aria-label="关闭面板"]')
    assert invoke('bootstrap')['ok'], 'closing settings must keep the core and main window alive'

    # Two simultaneous open requests must both succeed and use the same window.
    execute('window.__settingsOpenResults=null; Promise.allSettled([window.__TAURI_INTERNALS__.invoke("open_settings_window"), window.__TAURI_INTERNALS__.invoke("open_settings_window")]).then(r=>window.__settingsOpenResults=r.map(v=>v.status)); return true')
    results = wait(lambda: execute_main('return window.__settingsOpenResults'), 'concurrent settings creation')
    assert results == ['fulfilled', 'fulfilled'], results
    switch_window('settings')
    wait(lambda: execute('return !!document.querySelector("#theme")'), 'recreated settings UI')
    click('[aria-label="关闭面板"]')
    switch_window('main')

    status = invoke('clipboard_status')['value']
    if status['available']:
        wait(lambda: execute('return !document.querySelector(".titlebar-listening").disabled'), 'listener ready')
        initially_enabled = status['policy']['enabled']
        for enabled in [not initially_enabled, initially_enabled]:
            click('.titlebar-listening')
            wait(lambda: invoke('clipboard_status')['value']['policy']['enabled'] == enabled, 'listener policy commit')
            wait(lambda: execute('return !document.querySelector(".titlebar-listening").disabled'), 'listener mutation settled')
    else:
        assert execute('return document.querySelector(".titlebar-listening").disabled')
        print('LIMIT: capture unavailable in this display; listener disabled state verified', flush=True)
    for text in ['明天 10:00 确认窗口交互\n完成主窗搜索与独立设置窗的体验。', 'npm run tauri:dev', 'https://example.com/design-notes']:
        assert invoke('create_text', {'text': text, 'collectionId': None})['ok']
    wait(lambda: execute('return document.querySelectorAll(".clip-row").length === 3'), 'main history remains usable after settings close')
    capture('compact-main-window')
    print('PASS settings window: main modal guard, separate settings UI, limited IPC, cross-window updates, concurrent open, close/reopen, compact filters and listener state', flush=True)
