"""Native batch actions on disposable data and the isolated X11 clipboard."""
import subprocess
import tempfile
import json
import sqlite3
import time
from pathlib import Path


def run(invoke, execute, click, wait, env):
    def action(command, **arguments):
        response = invoke(command, arguments)
        assert response['ok'], response
        return response['value']

    def key(keys):
        subprocess.run(['xdotool', 'key', '--clearmodifiers', keys], env=env, check=True)

    def clipboard_text():
        value = subprocess.run(['xclip', '-selection', 'clipboard', '-o'], env=env, capture_output=True, text=True)
        return value.stdout if value.returncode == 0 else None

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
        return clipboard_text() == expected
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
    database = next(Path(env['XDG_DATA_HOME']).rglob('foundation.sqlite3'))
    with sqlite3.connect(database, isolation_level=None) as writer:
        writer.execute('BEGIN IMMEDIATE')
        key('Delete')
        wait(lambda: execute('return !document.querySelector("[data-row-main]")'), 'batch is hidden before SQLite can commit', timeout=1.5)
        assert writer.execute('SELECT count(*) FROM clips WHERE deleted=0').fetchone()[0] == 2
        writer.rollback()
    wait(lambda: execute('return !document.querySelector("[data-row-main]") && !!document.querySelector(".toast .text-button")'), 'native batch deletion committed with undo')
    click('.toast .text-button')
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

    click('[data-row-main]')
    fallback_text = execute('return document.querySelector(".clip-row.selected .clip-excerpt").textContent')
    assert not action('clipboard_status')['pasteTarget'], 'fallback test requires no native target'
    key('Return')
    wait(lambda: clipboard_text() == fallback_text, 'Enter copies the selected item without a paste target')
    assert execute('return document.hasFocus()'), 'copy fallback hid the main window'

    with tempfile.TemporaryDirectory(prefix='uclip-batch-receiver-') as directory:
        result = Path(directory) / 'received.json'
        receiver = subprocess.Popen(['/usr/bin/python3', 'scripts/paste-receiver.py', str(result)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            def receiver_window():
                value = subprocess.run(['xdotool', 'search', '--name', '^UClip native fixture receiver$'], env=env, capture_output=True, text=True)
                return value.stdout.strip().splitlines() if value.returncode == 0 else None
            window = wait(receiver_window, 'batch paste receiver')[0]

            def open_from_receiver():
                subprocess.run(['xdotool', 'windowactivate', '--sync', window], env=env, check=True)
                key('ctrl+shift+v')
                wait(lambda: action('clipboard_status')['pasteTarget'] and execute('return document.hasFocus()'), 'history opened with the GTK paste target')

            def received():
                try: return json.loads(result.read_text())
                except (OSError, json.JSONDecodeError): return None

            open_from_receiver()
            execute('document.querySelector(".toast .icon-button")?.click()')
            click('.clip-row:nth-of-type(2) [data-row-main]')
            single_text = execute('return document.querySelector(".clip-row.selected .clip-excerpt").textContent')
            assert single_text != fallback_text
            key('ctrl+c')
            wait(lambda: clipboard_text() == single_text, 'Ctrl+C only copies even with a paste target')
            wait(lambda: execute('return !!document.querySelector(".toast") && document.querySelector(".history-scroll").getAttribute("aria-busy") === "false"'),
                 'copy result and refreshed history ready')
            assert execute('return document.hasFocus()') and not result.exists()
            click('.clip-row:nth-of-type(2) [data-row-main]')
            assert execute('return document.querySelector(".clip-row.selected .clip-excerpt")?.textContent') == single_text
            key('Return')
            pasted = wait(received, 'Enter delivered the single selection to GTK', timeout=5)
            assert pasted == {'kind': 'text', 'text': single_text}, pasted
            key('ctrl+a')
            key('BackSpace')
            result.unlink()
            print('PASS: Enter pasted the selected item to the external GTK window and fell back to copy without a target', flush=True)

            open_from_receiver()
            expected = execute('return [...document.querySelectorAll(".clip-excerpt")].map(e=>e.textContent).join("\\n")')
            click('[data-row-main]')
            execute('document.querySelectorAll("[data-row-main]")[1].dispatchEvent(new MouseEvent("click", {bubbles:true,ctrlKey:true}))')
            wait(lambda: execute('return document.querySelectorAll(".clip-row.selected").length === 2'), 'paste selection')
            selected = execute('return [...document.querySelectorAll(".clip-row.selected")].map(row => row.dataset.clipId)')
            outcome = action('copy_clip', id=selected[0], ids=selected, paste=True)
            assert outcome['pasteRequested'], outcome
            value = wait(received, 'actual external batch text paste')
            assert value == {'kind': 'text', 'text': expected}, value
        finally:
            receiver.terminate()
            receiver.wait(timeout=5)
    print('PASS: native Ctrl+C merged text clipboard without history insertion, blank deselection, Delete/undo, rejected batch leaves clipboard intact, real second-delete revision conflict and partial undo; actual merged text received by external GTK app', flush=True)
