"""Verify native hover preview reuse and dismissal on isolated Linux displays."""
import re
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

    def window_origin(xid):
        info = subprocess.check_output(['xwininfo', '-id', xid], env=env, text=True, timeout=5)
        return tuple(int(re.search(rf'Absolute upper-left {axis}:\s*(-?\d+)', info).group(1)) for axis in 'XY')

    def open_preview(clip):
        if session == 'x11':
            xdotool('mousemove', '2', '2').check_returncode()
            xdotool('windowactivate', '--sync', main).check_returncode()
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
    if session == 'x11':
        main = visible_window('^UClip$')
        xdotool('windowmove', main, '90', '100').check_returncode()

        def enter_preview_from_row():
            xdotool('windowactivate', '--sync', main).check_returncode()
            xdotool('mousemove', '2', '2').check_returncode()
            row = wait(lambda: execute('''
                const row = document.querySelector('[data-clip-id="' + arguments[0] + '"] .row-main');
                if (!row) return null;
                const r = row.getBoundingClientRect();
                return [(r.x + r.width / 2) * devicePixelRatio, (r.y + r.height / 2) * devicePixelRatio];
            ''', [clip]), 'history row rendered')
            x, y = window_origin(main)
            xdotool('mousemove', str(round(x + row[0])), str(round(y + row[1]))).check_returncode()
            xid = wait(lambda: visible_window(title), 'row hover opens preview')
            assert xdotool('getactivewindow').stdout.strip() == main, 'hover preview took X11 focus'
            switch_window('hover-preview')
            points = wait(lambda: execute('''
                const text = document.querySelector('.native-preview-scroll pre')?.firstChild;
                if (text?.textContent !== 'Preview lifecycle fixture') return null;
                const range = document.createRange();
                return [0, 8].map((offset) => {
                    range.setStart(text, offset);
                    range.setEnd(text, offset);
                    const r = range.getBoundingClientRect();
                    return [r.x * devicePixelRatio, (r.y + r.height / 2) * devicePixelRatio];
                });
            '''), 'preview text ready for pointer selection')
            x, y = window_origin(xid)
            start, end = [[str(round(x + px)), str(round(y + py))] for px, py in points]
            xdotool('mousemove', *start).check_returncode()
            time.sleep(0.65)  # Outlast the history row's dismissal timer before clicking.
            assert visible_window(title) == xid, 'preview closed with the pointer over its text'
            assert xdotool('getactivewindow').stdout.strip() == main, 'pointer entry took X11 focus'
            return start, end

        time.sleep(1)  # Let the history poll settle after the fixture's out-of-band IPC write.
        enter_preview_from_row()
        xdotool('mousemove', '2', '2').check_returncode()
        wait_closed('leaving the unfocused preview dismisses it')
        start, end = enter_preview_from_row()
        xdotool('mousemove', *start, 'mousedown', '1', 'mousemove', *end, 'mouseup', '1').check_returncode()
        wait(lambda: execute('return window.getSelection()?.toString().trim() === "Preview"'), 'drag selected preview text')
        xdotool('key', 'Escape').check_returncode()
        wait_closed('Escape dismisses the preview after selecting text')
        print('PASS X11: hover transit, text drag selection and dismissal.', flush=True)

    handle = open_preview(clip)

    if session == 'x11':
        xid = visible_window(title)
        subprocess.run(['wmctrl', '-ic', hex(int(xid))], env=env, check=True, timeout=5)
    else:
        execute('const preview = document.querySelector(".native-hover-preview"); preview.dispatchEvent(new KeyboardEvent("keydown", {key: "Escape", bubbles: true})); return true')
    wait_closed('preview closed by native close or Escape')
    if session == 'x11':
        wait(lambda: execute('return document.hasFocus()'), 'main focus restored')
    assert open_preview(clip) == handle, 'preview WebView must be reused'

    if session == 'x11':
        xdotool('windowmove', main, '105', '115').check_returncode()
        wait_closed('main window movement dismisses preview')
        assert open_preview(clip) == handle, 'preview is reusable after moving main'
        xdotool('windowsize', main, '680', '620').check_returncode()
        wait_closed('main window resize dismisses preview')
        print('PASS X11: moving or resizing main dismisses preview.', flush=True)

        for index in range(240):
            assert invoke('create_text', {'text': f'Hover extent fixture {index}', 'collectionId': None})['ok']
        wait(lambda: execute('return document.querySelector(".capture-toggle span")?.textContent === "241"'), 'full history extent available')
        time.sleep(1)  # Let the history poll settle after the fixture's out-of-band IPC writes.
        xdotool('windowactivate', '--sync', main).check_returncode()
        height = execute('const list=document.querySelector(".history-scroll");list.scrollTop=list.scrollHeight;return list.scrollHeight')
        origin = window_origin(main)
        for index in (240, 239, 240):
            xdotool('mousemove', '2', '2').check_returncode()
            wait(lambda: not visible_window(title), 'last bottom hover dismissed')
            wait(lambda: execute('return !!document.querySelector(\'[data-history-index="\' + arguments[0] + \'"] .row-main\')', [index]), 'bottom row ready')
            box = execute('''
                const row=document.querySelector('[data-history-index="' + arguments[0] + '"] .row-main');
                const r=row.getBoundingClientRect();
                return {id:row.closest('[data-clip-id]').dataset.clipId,x:(r.x+r.width/2)*devicePixelRatio,y:(r.y+r.height/2)*devicePixelRatio};
            ''', [index])
            wait(lambda: execute('return document.hasFocus()'), 'main focused before pointer preview')
            xdotool('mousemove', str(round(origin[0] + box['x'])), str(round(origin[1] + box['y']))).check_returncode()
            wait(lambda: visible_window(title), 'bottom pointer opens preview')
            assert xdotool('getactivewindow').stdout.strip() == main, 'hover preview took X11 focus'
            assert execute('''
                const list=document.querySelector('.history-scroll');
                return list.scrollHeight===arguments[0] && list.scrollTop+list.clientHeight>=list.scrollHeight-1;
            ''', [height]), 'main renderer stalled or moved while preview opened'
            switch_window('hover-preview')
            assert invoke_current('get_hover_preview')['value']['id'] == box['id']
            switch_window('main')
        print('PASS X11: repeated pointer previews at the bottom preserve focus and scroll geometry.', flush=True)
    print(f'PASS {session}: native preview closes and reopens with the same WebView.', flush=True)
