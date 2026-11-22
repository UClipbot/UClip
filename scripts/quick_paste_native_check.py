"""Global shortcut, native focus and clipboard checks on the isolated X11 display."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import time
from clipboard_native_check import Client


def viewport(execute):
    return execute('return [innerWidth, innerHeight]')


def drag_corner(execute, wait, xdo, window):
    old = viewport(execute)
    grip = wait(lambda: execute("""const el = document.querySelector('.window-resize-grip');
      if (!el) return null;
      const rect = el.getBoundingClientRect();
      return {x: rect.left + 10, y: rect.top + 10, cursor: getComputedStyle(el).cursor,
              scale: devicePixelRatio};"""), 'resize grip rendered')
    assert grip['cursor'] == 'nwse-resize', grip
    xdo('mousemove', '--window', window, str(round(grip['x'] * grip['scale'])),
        str(round(grip['y'] * grip['scale'])))
    xdo('mousedown', '1')
    try:
        time.sleep(0.15)
        xdo('mousemove_relative', '--', '75', '45')
        time.sleep(0.15)
    finally:
        xdo('mouseup', '1')
    wait(lambda: (new := viewport(execute)) and new[0] > old[0] + 20 and new[1] > old[1] + 15,
         'native corner drag changed width and height')
    return viewport(execute)


def run(invoke, invoke_current, execute, click, close_settings, switch_window, wait, env, directory, log):
    assert env.get('UCLIP_FIXTURE_DISPLAY') == '1'
    assert os.environ.get('DISPLAY') == env['DISPLAY']
    source = None
    receiver_path = Path(directory) / 'quick-paste-result.json'
    receiver = subprocess.Popen(
        ['/usr/bin/python3', 'scripts/paste-receiver.py', str(receiver_path)],
        env=env, stdout=log, stderr=log,
    )

    def xdo(*args):
        return subprocess.check_output(['xdotool', *args], env=env, text=True).strip()

    def find_window(title):
        result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', title],
                                env=env, capture_output=True, text=True)
        return result.stdout.strip().splitlines() if result.returncode == 0 else []

    def action(command, **args):
        result = invoke(command, args)
        assert result['ok'], result
        return result['value']

    def set_quick(binding, pinned=False):
        prefs = action('quick_paste_settings')['preferences']
        return invoke('update_quick_paste_settings', {
            'request': {'shortcut': binding, 'includePinned': pinned, 'expectedRevision': prefs['revision']}
        })

    def popup(chord, count=5, at_edge=True):
        xdo('windowactivate', '--sync', target)
        width, height = map(int, xdo('getdisplaygeometry').split())
        xdo('mousemove', str(width - 24 if at_edge else 160), str(height - 24 if at_edge else 100))
        if isinstance(chord, str):
            xdo('key', chord)
        else:
            xdo('keydown', *chord)
            time.sleep(0.65)
            xdo('keyup', chord[-1])
        wait(lambda: xdo('getactivewindow') != target, 'window manager moved focus to quick paste', timeout=4)
        switch_window('quick-paste')
        wait(lambda: execute('return document.hasFocus() && document.querySelectorAll(".quick-paste-items button").length === arguments[0]', [count]),
             'focused quick panel and stable slots')
        window = xdo('getactivewindow')
        geometry = subprocess.check_output(['xwininfo', '-id', window], env=env, text=True)
        def value(label):
            return int(next(line for line in geometry.splitlines() if label in line).split(':')[-1])
        assert 0 <= value('Absolute upper-left X:') <= width - value('Width:'), geometry
        assert 0 <= value('Absolute upper-left Y:') <= height - value('Height:'), geometry
        return window

    def main_focused():
        wait(lambda: xdo('getactivewindow') == main_xid, 'window manager focused main')
        switch_window('main')
        wait(lambda: execute('return document.hasFocus()'), 'full main window focused')

    try:
        main_xid = wait(lambda: find_window('^UClip$'), 'main X11 window')[0]
        target = wait(lambda: find_window('^UClip native fixture receiver$'), 'external paste fixture')[0]
        main_binding = action('desktop_status')['preferences']['shortcut']
        restored_main_binding = 'Control+Shift+V'
        initial = action('quick_paste_settings')
        assert initial['preferences']['shortcut'] is None
        assert not initial['preferences']['includePinned']
        denied_resize = invoke('resize_auxiliary_corner')
        assert not denied_resize['ok'] and 'not allowed' in str(denied_resize['error']), denied_resize
        denied_recording = invoke('set_global_shortcut_recording', {'token': 1, 'recording': True})
        assert not denied_recording['ok'] and 'not allowed' in str(denied_recording['error']), denied_recording
        assert not set_quick(main_binding)['ok']
        # Exercise the actual settings form, including disabled-by-default and separate binding.
        click('[data-testid="footer-settings"]')
        wait(lambda: execute('return document.hasFocus()'), 'settings window focused')
        settings_size = drag_corner(execute, wait, xdo, xdo('getactivewindow'))
        xdo('windowsize', xdo('getactivewindow'), '700', '560')
        wait(lambda: viewport(execute) != settings_size, 'programmatic settings resize')
        close_settings()
        click('[data-testid="footer-settings"]')
        wait(lambda: viewport(execute) == settings_size, 'settings size restored on reopen')
        click('[data-settings-page="desktop"]')
        assert execute('return !document.querySelector("#main-window-shortcut").getClientRects().length && !document.querySelector("#quick-paste-shortcut").getClientRects().length')
        pinned_control = '#quick-paste-include-pinned'
        wait(lambda: execute('return document.querySelector(arguments[0]).matches(":enabled")', [pinned_control]), 'Quick Paste preferences loaded')
        click(pinned_control)
        wait(lambda: invoke_current('quick_paste_settings')['value']['preferences']['includePinned'], 'pinned option saved from Desktop')

        click('[data-settings-page="keyboard"]')
        assert execute('return ["global-shortcuts-heading", "window-shortcuts-heading"].every(id => document.getElementById(id)?.getClientRects().length) && !document.querySelector("#quick-paste-include-pinned").getClientRects().length')
        main_recorder = '#main-window-shortcut'
        quick_recorder = '#quick-paste-shortcut'
        main_save = 'form:has(#main-window-shortcut) [type=submit]'
        quick_save = 'form:has(#quick-paste-shortcut) [type=submit]'
        quick_clear = 'form:has(#quick-paste-shortcut) button:not(#quick-paste-shortcut)[type=button]'
        settings_xid = xdo('getactivewindow')

        def begin(selector):
            wait(lambda: execute('return document.querySelector(arguments[0]).matches(":enabled")', [selector]),
                 'shortcut recorder ready')
            click(selector)
            wait(lambda: execute('return document.activeElement?.matches(arguments[0]) && document.activeElement.getAttribute("aria-pressed") === "true"', [selector]),
                 'focused shortcut recorder')

        def binding_is(selector, label):
            return execute('return document.querySelector(arguments[0]).textContent === arguments[1]', [selector, label])

        def record(selector, keys, label):
            begin(selector)
            xdo('key', '--clearmodifiers', keys)
            wait(lambda: binding_is(selector, label), f'recorded {label}')
            assert xdo('getactivewindow') == settings_xid

        assert binding_is(main_recorder, 'Ctrl + Shift + V')
        assert binding_is(quick_recorder, '未设置')
        begin(main_recorder)
        xdo('key', 'Control_L')
        assert binding_is(main_recorder, '请按键…')
        xdo('key', 'j')
        wait(lambda: execute('return document.querySelector("#main-window-shortcut-feedback").textContent.includes("请按住")'), 'unmodified key rejected')
        xdo('key', 'Escape')
        wait(lambda: binding_is(main_recorder, 'Ctrl + Shift + V'), 'Escape cancelled and left Settings open')
        assert execute('return document.querySelector(arguments[0]).disabled', [main_save])

        begin(main_recorder)
        xdo('key', 'Tab')
        wait(lambda: binding_is(main_recorder, 'Ctrl + Shift + V'), 'Tab moved focus and cancelled')
        assert execute('return !document.activeElement.matches(arguments[0])', [main_recorder])

        begin(main_recorder)
        xdo('windowactivate', '--sync', target)
        wait(lambda: binding_is(main_recorder, 'Ctrl + Shift + V'), 'external focus cancelled recording')
        xdo('windowactivate', '--sync', settings_xid)
        wait(lambda: execute('return document.hasFocus()'), 'settings refocused after leaving recorder')

        record(main_recorder, 'ctrl+alt+j', 'Ctrl + Alt + J')
        assert invoke_current('desktop_status')['value']['preferences']['shortcut'] == main_binding
        click(main_save)
        wait(lambda: invoke_current('desktop_status')['value']['preferences']['shortcut'] == 'Control+Alt+J', 'main global binding saved from Shortcuts')
        record(quick_recorder, 'ctrl+alt+j', 'Ctrl + Alt + J')
        click(quick_save)
        wait(lambda: execute('return document.querySelector("#quick-paste-shortcut").form.querySelector("[role=status]").textContent.includes("系统保留")'), 'shared global shortcut rejected in UI')
        assert invoke_current('quick_paste_settings')['value']['preferences']['shortcut'] is None
        record(quick_recorder, 'ctrl+alt+v', 'Ctrl + Alt + V')
        click(quick_save)
        saved = wait(lambda: (value := invoke_current('quick_paste_settings')['value'])['available'] and value, 'Quick Paste global binding saved from Shortcuts')
        assert saved['preferences']['includePinned']
        click(quick_clear)
        assert binding_is(quick_recorder, '未设置')
        assert invoke_current('quick_paste_settings')['value']['available']
        click(quick_save)
        wait(lambda: not invoke_current('quick_paste_settings')['value']['available'], 'cleared Quick Paste binding')
        assert binding_is(quick_recorder, '未设置')
        record(quick_recorder, 'ctrl+alt+v', 'Ctrl + Alt + V')
        click(quick_save)
        wait(lambda: invoke_current('quick_paste_settings')['value']['available'], 're-registered Quick Paste binding')

        record(main_recorder, 'ctrl+alt+v', 'Ctrl + Alt + V')
        click(main_save)
        wait(lambda: execute('return document.querySelector("#main-window-shortcut-feedback").textContent.includes("系统保留")'), 'Quick Paste conflict rejected for main binding')
        record(main_recorder, 'ctrl+shift+v', 'Ctrl + Shift + V')
        click(main_save)
        wait(lambda: invoke_current('desktop_status')['value']['preferences']['shortcut'] == restored_main_binding, 'original main combination restored')

        copy_keys = '[aria-label="修改复制快捷键"]'
        click(copy_keys)
        wait(lambda: execute('return document.activeElement?.matches(arguments[0]) && document.activeElement.getAttribute("aria-pressed") === "true"', [copy_keys]), 'window shortcut recording started')
        xdo('key', '--clearmodifiers', 'ctrl+Return')
        click('[data-testid="save-window-keys"]')
        wait(lambda: invoke_current('desktop_status')['value']['keyboard']['bindings']['copy']['primary'], 'window binding saved on the same page')

        click('[data-settings-page="desktop"]')
        click(pinned_control)
        prefs = wait(lambda: (p := invoke_current('quick_paste_settings')['value']['preferences']) and not p['includePinned'] and p, 'pinned option cleared from Desktop')
        assert prefs['shortcut'] == saved['preferences']['shortcut']
        print('PASS shortcut recording: both global bindings, Escape and Tab, conflicting registered keys, Quick Paste clear, window binding and separate pinned option', flush=True)
        close_settings()
        switch_window('main')
        ids = [action('create_text', text=f'quick sample {number}') for number in range(6)]
        pinned = action('create_text', text='quick pinned')
        action('change_clip_pin', request={'id': pinned, 'expectedRevision': '0', 'pinned': True})

        panel = popup('ctrl+alt+v', at_edge=False)
        assert viewport(execute)[0] >= 520, viewport(execute)
        panel_size = drag_corner(execute, wait, xdo, panel)
        xdo('windowsize', panel, '430', '310')
        wait(lambda: viewport(execute) != panel_size, 'programmatic panel resize')
        xdo('key', 'Escape')
        wait(lambda: xdo('getactivewindow') == target, 'closed resized quick panel')
        popup('ctrl+alt+v')
        wait(lambda: viewport(execute) == panel_size, 'quick panel restored resized dimensions')
        assert execute('return [...document.querySelectorAll(".quick-paste-items button span")].map(e=>e.textContent)') == [
            f'quick sample {number}' for number in range(5, 0, -1)
        ]
        denied = invoke_current('history_query', {'request': {
            'query': '', 'regex': False, 'kind': None, 'collectionId': None,
            'offset': 0, 'expectedRevision': None,
        }})
        assert not denied['ok'] and 'not allowed' in str(denied['error']), denied
        token = invoke_current('quick_paste_snapshot')['value']['token']
        assert not invoke_current('select_quick_paste', {'token': 'stale', 'slot': 0})['ok']
        assert not invoke_current('select_quick_paste', {'token': token, 'slot': 99})['ok']
        xdo('key', '2')
        def received():
            try:
                return json.loads(receiver_path.read_text())
            except (OSError, json.JSONDecodeError):
                return None
        assert wait(received, 'native receiver received selected text') == {'kind': 'text', 'text': 'quick sample 4'}
        wait(lambda: xdo('getactivewindow') == target, 'receiver focus restored')

        try:
            # Holding the opening letter did not expand the panel. Select with Ctrl+Alt still held.
            popup(('ctrl', 'alt', 'v'))
            assert execute('return document.hasFocus()'), 'held opening key expanded'
            xdo('key', '3')
        finally:
            xdo('keyup', 'v', 'alt', 'ctrl')
        assert wait(lambda: (received() or {}).get('text', '').endswith('quick sample 3'),
                    'modified digit waited for native modifier release')

        # Releasing the opening letter then pressing it locally expands without typing in search.
        popup('ctrl+alt+v')
        xdo('key', 'v')
        main_focused()
        assert execute('return document.querySelector("#history-search").value') == ''

        assert set_quick('Control+1', True)['ok']
        popup('ctrl+1')
        assert execute('return document.querySelector(".quick-paste-items button span").textContent') == 'quick pinned'
        xdo('key', 'ctrl+1')
        time.sleep(0.15)
        assert execute('return document.hasFocus()'), 'numeric Quick Paste chord expanded'
        xdo('key', 'ctrl+shift+v')
        main_focused()

        # An external focus change abandons the live session, without writing.
        popup('ctrl+1')
        xdo('windowactivate', '--sync', target)
        wait(lambda: invoke_current('quick_paste_snapshot')['value'] is None, 'external focus cancelled quick paste')
        assert received() == {'kind': 'text', 'text': 'quick sample 4quick sample 3'}

        popup('ctrl+1')
        xdo('key', '1')
        wait(lambda: (received() or {}).get('text', '').endswith('quick pinned'), 'bare numeric final key pasted slot 1')
        assert action('quick_paste_settings')['preferences']['includePinned']
        assert action('desktop_status')['preferences']['shortcut'] == restored_main_binding
        assert len(ids) == 6

        source = Client(['target/debug/examples/clipboard_fixture'], env, log)
        image = {'width': 2, 'height': 2, 'rgba': [34, 120, 200, 255] * 4}
        source.send({'op': 'set', 'kind': 'image', **image})
        def query_kind(kind):
            return action('history_query', request={
                'query': '', 'regex': False, 'kind': kind, 'collectionId': None,
                'offset': 0, 'expectedRevision': None,
            })['total']
        wait(lambda: query_kind('image') == 1, 'native image captured')
        popup('ctrl+1')
        wait(lambda: execute('return !!document.querySelector(".quick-paste-items button img")'), 'image thumbnail')
        xdo('key', '1')
        assert wait(lambda: (value := received()) and value.get('kind') == 'image' and value,
                    'external image paste') == {'kind': 'image', **image}

        file_path = Path(directory) / 'quick file with spaces.txt'
        file_path.write_text('synthetic file')
        source.send({'op': 'set', 'kind': 'files', 'files': [str(file_path)]})
        wait(lambda: query_kind('files') == 1, 'native file reference captured')
        popup('ctrl+1')
        xdo('key', '1')
        assert wait(lambda: (value := received()) and value.get('kind') == 'files' and value,
                    'external file paste') == {'kind': 'files', 'uris': [file_path.as_uri()]}

        popup('ctrl+1')
        xdo('key', 'Escape')
        wait(lambda: xdo('getactivewindow') == target, 'Escape restored external focus')
        wait(lambda: invoke_current('quick_paste_snapshot')['value'] is None, 'Escape cleared session')
        assert received() == {'kind': 'files', 'uris': [file_path.as_uri()]}

        # Hold the usage write after clipboard publication and move focus before dispatch.
        feedback_text = 'quick paste copied before external focus change'
        action('create_text', text=feedback_text)
        panel = popup('ctrl+1')
        database_path, = Path(directory, 'data').rglob('foundation.sqlite3')
        with sqlite3.connect(database_path) as database:
            database.execute('BEGIN IMMEDIATE')
            xdo('key', '1')
            wait(lambda: source.send({'op': 'read', 'kind': 'text'}).get('text') == feedback_text,
                 'quick paste published before usage write', timeout=1.5)
            xdo('windowactivate', '--sync', target)
            wait(lambda: not execute('return document.hasFocus()'), 'external focus during usage write')
        feedback = wait(lambda: (value := invoke_current('quick_paste_snapshot')['value'])
                        and value['feedbackOnly'] and value, 'non-activating completion feedback')
        assert not feedback['items'] and feedback['result']['copied'] and not feedback['result']['pasteRequested'], feedback
        assert feedback['result']['warning']['code'] == 'target_lost', feedback
        wait(lambda: execute(r'return document.querySelector(".quick-paste-status").textContent.includes("\u5df2\u590d\u5236")'),
             'manual paste fallback displayed')
        assert panel in find_window('^UClip$') and xdo('getactivewindow') == target
        assert received() == {'kind': 'files', 'uris': [file_path.as_uri()]}
        assert invoke_current('cancel_quick_paste', {'token': feedback['token']})['ok']

        assert set_quick(None, True)['ok']
        subprocess.run(['setxkbmap', '-layout', 'us', '-variant', 'dvorak'], env=env, check=True)
        try:
            assert set_quick('Control+Alt+J', True)['ok']
            popup('ctrl+alt+j')
            xdo('key', 'j')
            main_focused()
            assert set_quick(None, True)['ok']
        finally:
            subprocess.run(['setxkbmap', '-layout', 'us'], env=env, check=True)
        assert set_quick('Control+1', True)['ok']
        print('PASS Quick Paste regressions: Dvorak final key, visible manual paste feedback after focus loss and user-only size persistence', flush=True)

        print('PASS Quick Paste: native settings, three actual paste formats, held modifiers, numeric and letter expansion, slot ACL, pinned filtering, Escape/focus cancellation and edge placement', flush=True)
        return {
            'preferences': action('quick_paste_settings')['preferences'],
            'settingsSize': settings_size,
            'panelSize': panel_size,
        }
    finally:
        if source:
            source.close()
        receiver.terminate()
        receiver.wait(timeout=5)


def verify_restart(invoke, execute, click, close_settings, switch_window, wait, env, saved):
    def preferences_ready():
        result = invoke('quick_paste_settings')
        return result.get('ok') and result['value']['available'] and result['value']['preferences'] == saved['preferences']
    wait(preferences_ready, 'separate shortcut and include-pinned preference reloaded and registered')
    assert invoke('open_settings_window')['ok']
    switch_window('settings')
    wait(lambda: viewport(execute) == saved['settingsSize'], 'settings dimensions restored after restart')
    click('[data-settings-page="keyboard"]')
    assert execute('return document.querySelector("#main-window-shortcut").textContent === "Ctrl + Shift + V" && document.querySelector("#quick-paste-shortcut").textContent === "Ctrl + 1"')
    assert invoke('desktop_status')['value']['keyboard']['bindings']['copy']['primary']
    close_settings()
    switch_window('main')
    wait(lambda: execute('return document.hasFocus()'), 'main refocused after settings')
    subprocess.run(['xdotool', 'mousemove', '160', '100', 'key', 'ctrl+1'], env=env, check=True)
    switch_window('quick-paste')
    wait(lambda: execute('return document.hasFocus()') and viewport(execute) == saved['panelSize'],
         'quick panel dimensions restored after restart')
    subprocess.run(['xdotool', 'key', 'Escape'], env=env, check=True)
    print('PASS Quick Paste preferences and both resized windows restored after native restart', flush=True)
