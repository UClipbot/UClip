"""Filter focus regression checks on an isolated native Linux X11 display."""
from pathlib import Path
import subprocess
import time


def run(invoke, execute, click, wait, env, directory, log):
    def action(command, **arguments):
        response = invoke(command, arguments)
        assert response['ok'], response
        return response['value']

    def key(value):
        subprocess.run(['xdotool', 'key', '--clearmodifiers', value], env=env, check=True)

    def visible():
        result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', '^UClip$'],
                                env=env, capture_output=True, text=True)
        return bool(result.stdout.strip())

    preferences = action('desktop_status')['preferences']
    request = {key: preferences[key] for key in ['closeToTray', 'autoHide', 'shortcut', 'nativeTitlebar']}
    request.update(autoHide=True, expectedRevision=preferences['revision'])
    action('update_desktop_preferences', request=request)
    action('set_desktop_interaction', active=False)
    for theme in ['light', 'dark']:
        execute('document.documentElement.dataset.theme = arguments[0]', [theme])
        for control in ['scope', 'type']:
            selector = f'.{control}-control .filter-trigger'
            click(selector)
            time.sleep(0.8)
            assert visible(), 'filter popup triggered auto-hide'
            assert execute('return document.hasFocus() && !!document.querySelector(".action-menu")')
            key('Down')
            key('Return')
            wait(lambda: execute('return !document.querySelector(".action-menu")'), 'filter selection closes menu')
            assert execute('return document.activeElement.matches(arguments[0])', [selector])
            click(selector)
            click('.action-menu button:not(:disabled)')
            assert execute('return !document.querySelector(".action-menu")')
            click(selector)
            key('Escape')
            time.sleep(0.5)
            assert visible() and execute('return !document.querySelector(".action-menu")')
            click(selector)
            click('#history-search')
            assert execute('return !document.querySelector(".action-menu")')
    external = subprocess.Popen(['/usr/bin/python3', 'scripts/paste-receiver.py', str(Path(directory) / 'filter-receiver.json')],
                                env=env, stdin=subprocess.PIPE, stdout=log, stderr=log, text=True)
    try:
        def receiver():
            result = subprocess.run(['xdotool', 'search', '--name', 'UClip native fixture receiver'],
                                    env=env, capture_output=True, text=True)
            return result.stdout.strip().splitlines() if result.stdout.strip() else None
        target = wait(receiver, 'external focus target')[0]
        # Open a popup, then transfer native focus to another app.
        click('.type-control .filter-trigger')
        subprocess.run(['xdotool', 'windowactivate', '--sync', target], env=env, check=True)
        wait(lambda: not visible(), 'external app still triggers auto-hide')
    finally:
        external.terminate()
        external.wait(timeout=5)
    print('PASS: native filter menus retain focus in light/dark themes; mouse, keyboard, Escape, outside click and external auto-hide', flush=True)
