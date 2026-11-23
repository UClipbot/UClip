"""Verify native hover preview reuse and dismissal on isolated Linux displays."""
import subprocess
import time
import uuid


def run(invoke, invoke_current, execute, switch_window, wait, env):
    session = invoke('bootstrap')['value']['platform']['sessionHint']
    if session == 'x11' and env.get('UCLIP_FIXTURE_DISPLAY') != '1':
        raise RuntimeError('Use scripts/isolated-copy-paste.py for X11 window checks.')
    assert session in ('x11', 'wayland'), session
    title = '^(\u5185\u5bb9\u9884\u89c8|Content preview)$'

    def xdotool(*args):
        return subprocess.run(['xdotool', *args], env=env, capture_output=True, text=True, timeout=5)

    def visible_window(name):
        result = xdotool('search', '--onlyvisible', '--name', name)
        return next(iter(result.stdout.splitlines()), None)

    def open_preview(clip):
        snapshot = {
            'id': clip,
            'token': str(uuid.uuid4()),
            'kind': 'text',
            'source': 'UClip',
            'savedAt': '1764000000000',
            'size': None,
            'anchor': {'x': 30, 'y': 88, 'width': 180, 'height': 24},
        }
        result = invoke('show_hover_preview', {'request': snapshot})
        assert result['ok'], result
        if session == 'x11':
            xid = wait(lambda: visible_window(title), 'native preview shown')
            assert xdotool('getactivewindow').stdout.strip() != xid
        handle = wait(lambda: switch_window('hover-preview'), 'preview WebView exists')
        wait(lambda: execute('return document.querySelector(".native-preview-scroll pre")?.textContent === "Preview lifecycle fixture"'), 'preview detail available')
        assert invoke_current('get_hover_preview')['value']['token'] == snapshot['token']
        return handle

    def wait_closed(description):
        if session == 'x11':
            wait(lambda: not visible_window(title), description)
        wait(lambda: invoke_current('get_hover_preview') == {'ok': True, 'value': None}, 'preview request cleared')
        switch_window('main')

    if session == 'x11':
        xdotool('mousemove', '30', '850').check_returncode()
        wait(lambda: execute('return document.hasFocus()'), 'main focus')
    record = invoke('create_text', {'text': 'Preview lifecycle fixture', 'collectionId': None})
    assert record['ok'], record
    clip = record['value']
    handle = open_preview(clip)

    if session == 'x11':
        xid = visible_window(title)
        subprocess.run(['wmctrl', '-ic', hex(int(xid))], env=env, check=True, timeout=5)
    else:
        execute('const preview = document.querySelector(".native-hover-preview"); preview.dispatchEvent(new KeyboardEvent("keydown", {key: "Escape", bubbles: true})); return true')
    wait_closed('preview closed by native close or Escape')
    if session == 'x11':
        wait(lambda: execute('return document.hasFocus()'), 'main focus restored')
        time.sleep(0.45)  # Match the normal hover delay before reopening.
    assert open_preview(clip) == handle, 'preview WebView must be reused'

    if session == 'x11':
        main = visible_window('^UClip$')
        xdotool('windowmove', main, '105', '115').check_returncode()
        wait_closed('main window movement dismisses preview')
        assert open_preview(clip) == handle, 'preview is reusable after moving main'
        xdotool('windowsize', main, '680', '620').check_returncode()
        wait_closed('main window resize dismisses preview')
        print('PASS X11: moving or resizing main dismisses preview.', flush=True)
    print(f'PASS {session}: native preview closes and reopens with the same WebView.', flush=True)
