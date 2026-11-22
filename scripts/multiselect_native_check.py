"""Native batch actions on disposable data and the isolated X11 clipboard."""
import subprocess
import tempfile
import json
import time
from pathlib import Path


def run(invoke, execute, click, wait, env):
    def action(command, **arguments):
        response = invoke(command, arguments)
        assert response['ok'], response
        return response['value']

    assert action('clipboard_status')['policy']['enabled'], 'recapture suppression must be tested with capture enabled'
    ids = [action('create_text', text=value) for value in ['batch first', 'batch second']]
    wait(lambda: execute('return document.querySelectorAll("[data-row-main]").length === 2'), 'created batch records')
    # Exercise the React handlers, then use real native clipboard publication.
    click('[data-row-main]')
    execute('document.querySelectorAll("[data-row-main]")[1].dispatchEvent(new MouseEvent("click", {bubbles:true,ctrlKey:true}))')
    wait(lambda: execute('return document.querySelectorAll(".clip-row.selected").length === 2'), 'native multiselect')
    expected = execute('return [...document.querySelectorAll(".clip-row.selected .clip-excerpt")].map(e=>e.textContent).join("\\n")')
    subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+c'], env=env, check=True)
    def copied():
        value = subprocess.run(['xclip', '-selection', 'clipboard', '-o'], env=env, capture_output=True, text=True)
        return value.returncode == 0 and value.stdout == expected
    wait(copied, 'one merged native clipboard write')
    wait(lambda: execute('return document.querySelector(".history-scroll").getAttribute("aria-busy") === "false"'), 'copy refresh')
    # Wait beyond collection polling to ensure internal clipboard content is never captured as a new row.
    time.sleep(1)
    page = action('history_query', request={'query':'', 'regex':False, 'kind':None, 'collectionId':None, 'offset':0, 'expectedRevision':None})
    assert page['total'] == 2, page
    execute('document.querySelector(".history-scroll").click()')
    assert execute('return document.querySelectorAll(".clip-row.selected").length') == 0
    subprocess.run(['xdotool', 'key', '--clearmodifiers', 'Delete'], env=env, check=True)
    assert execute('return document.querySelectorAll("[data-row-main]").length') == 2
    subprocess.run(['xdotool', 'key', '--clearmodifiers', 'space'], env=env, check=True)
    assert execute('return !document.querySelector(".detail-panel")')
    click('[data-row-main]')
    execute('document.querySelectorAll("[data-row-main]")[1].dispatchEvent(new MouseEvent("click", {bubbles:true,ctrlKey:true}))')
    subprocess.run(['xdotool', 'key', '--clearmodifiers', 'Delete'], env=env, check=True)
    wait(lambda: execute('return document.querySelectorAll("[data-row-main]").length === 0'), 'native batch deletion')
    click('.toast button')
    wait(lambda: execute('return document.querySelectorAll("[data-row-main]").length === 2'), 'native batch undo')
    # Reject an empty selection without modifying the system clipboard.
    click('[data-row-main]')
    execute('document.querySelectorAll("[data-row-main]")[1].dispatchEvent(new MouseEvent("click", {bubbles:true,ctrlKey:true}))')
    wait(lambda: execute('return document.querySelectorAll(".clip-row.selected").length === 2'), 'reselect restored records')
    invalid = invoke('copy_clip', {'id': ids[0], 'ids': [], 'paste': False})
    assert not invalid['ok'] and invalid['error']['code'] == 'invalid_input', invalid
    assert copied(), 'invalid batch changed clipboard'
    execute("""const row=document.querySelectorAll('[data-clip-id]')[1];
      window.__TAURI_INTERNALS__.invoke('change_clip_pin', {request:{id:row.dataset.clipId,expectedRevision:row.dataset.clipRevision,pinned:true}})
        .then(()=> {const list=document.querySelector('.history-scroll'); list.focus(); list.dispatchEvent(new KeyboardEvent('keydown',{key:'Delete',code:'Delete',bubbles:true}));});""")
    wait(lambda: execute('return document.querySelectorAll("[data-row-main]").length === 1 && (document.querySelector(".toast").textContent.includes("1/2") || document.querySelector(".toast").textContent.includes("1 of 2"))'), 'partial failure feedback')
    click('.toast button')
    wait(lambda: execute('return document.querySelectorAll("[data-row-main]").length === 2'), 'partial batch undo')
    with tempfile.TemporaryDirectory(prefix='uclip-batch-receiver-') as directory:
        result = Path(directory) / 'received.json'
        receiver = subprocess.Popen(['/usr/bin/python3', 'scripts/paste-receiver.py', str(result)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            def receiver_window():
                value = subprocess.run(['xdotool', 'search', '--name', '^UClip native fixture receiver$'], env=env, capture_output=True, text=True)
                return value.stdout.strip().splitlines() if value.returncode == 0 else None
            window = wait(receiver_window, 'batch paste receiver')[0]
            expected = execute('return [...document.querySelectorAll(".clip-excerpt")].map(e=>e.textContent).join("\\n")')
            click('[data-row-main]')
            execute('document.querySelectorAll("[data-row-main]")[1].dispatchEvent(new MouseEvent("click", {bubbles:true,ctrlKey:true}))')
            wait(lambda: execute('return document.querySelectorAll(".clip-row.selected").length === 2'), 'paste selection')
            subprocess.run(['xdotool', 'windowactivate', '--sync', window], env=env, check=True)
            subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+shift+v'], env=env, check=True)
            wait(lambda: action('clipboard_status')['pasteTarget'] and execute('return document.hasFocus()'), 'batch paste target')
            execute('document.querySelector("[data-clip-id]").dispatchEvent(new MouseEvent("contextmenu", {bubbles:true,cancelable:true}))')
            wait(lambda: execute('return document.querySelector(".action-menu button:first-child")?.disabled === false'), 'batch paste enabled in the menu')
            click('.action-menu button:first-child')
            def received():
                try: return json.loads(result.read_text())
                except (OSError, json.JSONDecodeError): return None
            value = wait(received, 'actual external batch text paste')
            assert value == {'kind': 'text', 'text': expected}, value
        finally:
            receiver.terminate()
            receiver.wait(timeout=5)
    print('PASS: native Ctrl+C merged text clipboard without history insertion, blank deselection, Delete/undo, rejected batch leaves clipboard intact, real second-delete revision conflict and partial undo; actual merged text received by external GTK app', flush=True)
