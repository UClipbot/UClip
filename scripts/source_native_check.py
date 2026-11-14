"""Real Google Chrome copy provenance; only run on an isolated X11 display."""
import subprocess
import time
from clipboard_native_check import Client


def run(invoke, execute, env, log, wait):
    if env.get('UCLIP_FIXTURE_DISPLAY') != '1' or env.get('DISPLAY') in (None, ':0', ':0.0'):
        raise RuntimeError('Source checks require an isolated X11 display.')

    def action(command, **arguments):
        result = invoke(command, arguments)
        assert result['ok'], result
        return result['value']

    def items():
        return action('history_query', request={'query': '', 'regex': False, 'kind': None, 'offset': 0, 'expectedRevision': None})['items']

    p = action('desktop_status')['preferences']
    action('update_desktop_preferences', request={**{k: v for k, v in p.items() if k != 'revision'}, 'autoHide': False, 'expectedRevision': p['revision']})
    policy = action('clipboard_status')['policy']
    action('update_capture_policy', request={**{k: v for k, v in policy.items() if k != 'revision'}, 'enabled': True, 'expectedRevision': policy['revision']})
    browser = Client(['node', 'scripts/browser-source-fixture.mjs', env.get('UCLIP_PLAYWRIGHT_MODULE', 'playwright')], env, log)
    anonymous = None
    try:
        browser.read()
        link = 'https://example.com/uclip-source-fixture'
        browser.send({'text': link})
        item = wait(lambda: next((item for item in items() if item['preview'] == link), None), 'real Chrome copy captured')
        assert (item['sourceApp'] or '').lower() in ('google-chrome', 'google-chrome-stable'), item['sourceApp']
        source = item['sourceApp']
        wait(lambda: execute('const r=document.querySelector("[data-clip-id=\\""+arguments[0]+"\\"] .clip-row-meta");return !!r && r.textContent.includes(arguments[1])', [item['id'], source]), 'source name rendered in native history')

        # Chrome stays the foreground app, but an anonymous X client owns this copy.
        # It must not inherit Chrome's identity or preserve the previous source.
        anonymous = subprocess.Popen(['xclip', '-selection', 'clipboard', '-quiet'], env=env, stdin=subprocess.PIPE, stdout=log, stderr=log, text=True)
        anonymous.stdin.write(link)
        anonymous.stdin.close()
        active = subprocess.check_output(['xdotool', 'getactivewindow'], env=env, text=True).strip()
        active_class = subprocess.check_output(['xprop', '-id', active, 'WM_CLASS'], env=env, text=True)
        assert 'google-chrome' in active_class.lower(), 'Chrome must remain foreground for the negative-source check'
        wait(lambda: (page := items()) and page[0]['id'] == item['id'] and page[0]['sourceApp'] is None, 'unknown owner clears stale Chrome attribution')
        browser.send({'text': link})
        wait(lambda: (page := items()) and page[0]['id'] == item['id'] and page[0]['sourceApp'] == source, 'Chrome recopy restores actual source')
        action('copy_clip', id=item['id'], paste=False)
        time.sleep(0.2)  # Allow the native watcher to observe and skip the internal write.
        page = items()
        assert len(page) == 1 and page[0]['sourceApp'] == source, 'internal clipboard write changed stored provenance'
        print('PASS source: real Google Chrome copy and native UI, anonymous owner fallback, recapture identity and internal-write preservation', flush=True)
    finally:
        browser.close()
        if anonymous and anonymous.poll() is None:
            anonymous.terminate()
            anonymous.wait(timeout=5)
