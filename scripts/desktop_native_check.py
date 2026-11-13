"""Native desktop checks. Invoked only from the authenticated disposable X11 fixture session."""
import json
from pathlib import Path
import sqlite3
import subprocess
import time
from clipboard_native_check import Client, wait


def run(checks):
    c = checks
    status = c.action('desktop_status')
    assert status['shortcutAvailable'], status
    assert not status['preferences']['closeToTray']
    window = c.window('^UClip$')

    pid=subprocess.check_output(['xdotool','getwindowpid',window],env=c.env,text=True).strip()
    def visible():
        nonlocal window
        # GTK may replace its native XID while hiding/remapping; bind to the same process and title.
        matches=subprocess.run(['xdotool','search','--all','--onlyvisible','--pid',pid,'--name','^UClip$'],env=c.env,text=True,capture_output=True)
        if matches.returncode==0 and matches.stdout.strip():
            window=matches.stdout.splitlines()[0]
            return True
        return False

    def save(**changes):
        p = c.action('desktop_status')['preferences']
        request = {key: p[key] for key in ['closeToTray', 'alwaysOnTop', 'autoHide', 'shortcut']}
        request['expectedRevision'] = p['revision']
        request.update(changes)
        return c.action('update_desktop_preferences', request=request)

    save(alwaysOnTop=True)
    wait(lambda: '_NET_WM_STATE_ABOVE' in subprocess.check_output(['xprop','-id',window,'_NET_WM_STATE'],env=c.env,text=True), 'window stays above')
    save(alwaysOnTop=False, closeToTray=True)
    subprocess.run(['wmctrl','-ic',hex(int(window))],env=c.env,check=True)
    wait(lambda:not visible(),'close hides without destroying the WebView')
    c.key('key','ctrl+shift+v')
    wait(visible,'shortcut restores closed-to-tray window')
    wait(lambda:c.action('desktop_status')['shortcutAvailable'],'shortcut remains registered')
    c.action('hide_main_window')
    wait(lambda:not visible(),'explicit hide')
    save(shortcut='Control+Alt+K')
    c.key('key','ctrl+shift+v')
    time.sleep(0.2)
    assert not visible(), 'old shortcut was not unregistered'
    c.key('key','ctrl+alt+k')
    wait(visible,'custom shortcut restores window')

    grabber=Client(['target/debug/examples/clipboard_fixture','--shortcut-fixture'],c.env,c.log)
    try:
        assert grabber.read()['grabbed']
        before=c.action('desktop_status')['preferences']
        request={key:before[key] for key in ['closeToTray','alwaysOnTop','autoHide','shortcut']}
        request.update(shortcut='Control+Alt+J',expectedRevision=before['revision'])
        conflict=c.invoke('update_desktop_preferences',{'request':request})
        assert not conflict['ok'] and conflict['error']['code']=='shortcut_conflict',conflict
        assert c.action('desktop_status')['preferences']==before
        c.action('hide_main_window')
        c.key('key','ctrl+alt+k')
        wait(visible,'conflict preserves prior shortcut')
    finally:
        grabber.close()

    database=next(Path(c.directory,'data').rglob('foundation.sqlite3'))
    before=c.action('desktop_status')['preferences']
    with sqlite3.connect(database) as connection:
        connection.execute("CREATE TRIGGER fixture_desktop_failure BEFORE UPDATE ON desktop_preferences BEGIN SELECT RAISE(ABORT,'fixture'); END")
    try:
        request={key:before[key] for key in ['closeToTray','alwaysOnTop','autoHide','shortcut']}
        request.update(alwaysOnTop=True,shortcut='Control+Alt+L',expectedRevision=before['revision'])
        failed=c.invoke('update_desktop_preferences',{'request':request})
        assert not failed['ok'] and failed['error']['code']=='storage',failed
        assert c.action('desktop_status')['preferences']==before
        assert '_NET_WM_STATE_ABOVE' not in subprocess.check_output(['xprop','-id',window,'_NET_WM_STATE'],env=c.env,text=True)
        c.action('hide_main_window')
        c.key('key','ctrl+alt+l')
        time.sleep(0.2)
        assert not visible(),'failed write left replacement shortcut enabled'
        c.key('key','ctrl+alt+k')
        wait(visible,'failed write preserves previous shortcut')
    finally:
        with sqlite3.connect(database) as connection:connection.execute('DROP TRIGGER fixture_desktop_failure')
    print('PASS P2: native failed preference transaction restores topmost and shortcut registration',flush=True)

    enabled=c.action('set_autostart',enabled=True)
    assert enabled and c.action('desktop_status')['autostartEnabled']
    entries=list(Path(c.env['XDG_CONFIG_HOME'],'autostart').glob('*.desktop'))
    assert len(entries)==1 and 'uclip-desktop' in entries[0].read_text()
    assert c.action('set_autostart',enabled=False) is False
    assert not entries[0].exists()
    print('PASS P2: native close/hide/recover, custom shortcut, actual X11 shortcut conflict, topmost, disposable OS autostart entry',flush=True)

    save(autoHide=True)
    c.action('set_desktop_interaction',active=False)
    external=subprocess.Popen(['/usr/bin/python3','scripts/paste-receiver.py',str(c.directory/'desktop-paste.json')],env=c.env,stdin=subprocess.PIPE,stdout=c.log,stderr=c.log,text=True)
    try:
        target=c.window('UClip native fixture receiver')
        c.activate(window)
        c.action('set_desktop_interaction',active=True)
        c.activate(target)
        time.sleep(0.6)
        assert visible(),'protected UI was hidden'
        c.activate(window)
        c.action('set_desktop_interaction',active=False)
        c.activate(target)
        wait(lambda:not visible(),'unprotected blur hides')
        c.key('key','ctrl+alt+k')
        wait(visible,'shortcut after blur hiding')
        save(alwaysOnTop=True)
        c.action('set_desktop_interaction',active=False)
        c.activate(target)
        time.sleep(0.6)
        assert visible(),'always-on-top was auto hidden'
        subprocess.run(['xprop','-id',target,'-f','_NET_WM_PID','32c','-set','_NET_WM_PID','0'],env=c.env,check=True,stdout=subprocess.DEVNULL)
        c.key('key','ctrl+alt+k')
        wait(visible,'wake from an unverifiable external window')
        wait(lambda:not c.action('clipboard_status')['pasteTarget'],'unknown foreground invalidates the old paste target')
        refused=c.invoke('copy_clip',{'id':c.ids['text'],'paste':True})
        assert not refused['ok'] and refused['error']['code']=='target_lost',refused
    finally:
        external.terminate();external.wait(timeout=5)
    save(closeToTray=False,alwaysOnTop=False,autoHide=False,shortcut='CommandOrControl+Shift+V')
    c.activate(window)
    subprocess.run(['wmctrl','-ir',hex(int(window)),'-e','0,120,90,700,600'],env=c.env,check=True)
    database=next(Path(c.directory,'data').rglob('foundation.sqlite3'))
    def geometry():
        with sqlite3.connect(database) as connection:
            row=connection.execute('SELECT width,height FROM window_placement WHERE singleton=1').fetchone()
            return row and row[0]==700 and row[1]==600
    wait(geometry,'debounced native geometry persists')
    print('PASS P2: native blur guard, automatic hiding, topmost exemption, window geometry persistence',flush=True)

    # Delay the actual storage worker behind bounded, deliberately stale writes.
    # WAL readers alone do not block; this tests real IPC delay without modifying Tauri internals.
    previous_policy=c.action('clipboard_status')['policy']
    c.policy(enabled=False)
    wait(lambda:c.execute('return document.querySelector(".capture-toggle")?.textContent.includes("开始采集") && document.querySelector(".history-scroll")?.getAttribute("aria-busy")==="false"'),'pause revision reaches the live UI before delay injection')
    connection=sqlite3.connect(database)
    connection.execute('BEGIN IMMEDIATE')
    try:
        c.execute('window.__fixtureCompleted=0;for(let i=0;i<8;i++)window.__TAURI_INTERNALS__.invoke("update_settings",{request:{theme:"dark",expectedRevision:"0"}}).then(()=>window.__fixtureCompleted++,()=>window.__fixtureCompleted++);return true')
        c.enter('#history-search','timeout fixture')
        wait(lambda:c.execute('return document.querySelector("#search-error")?.textContent.includes("超时")'),'native search timeout feedback',timeout=10)
        c.set('text',text='public timeout sentinel')
        c.key('key','Return')
        assert c.source.send({'op':'read','kind':'text'})['text']=='public timeout sentinel'
    finally:
        connection.rollback()
        connection.close()
    wait(lambda:c.execute('return window.__fixtureCompleted===8'),'bounded stale writes drain after lock release')
    time.sleep(0.2)
    assert c.execute('return document.querySelector("#search-error")?.textContent.includes("超时")'),'late history response replaced timeout'
    c.execute('delete window.__fixtureCompleted;return true')
    c.click('.empty-state .secondary-button')
    wait(lambda:c.execute('return !document.querySelector("#search-error") && document.querySelector(".history-scroll").getAttribute("aria-busy")==="false"'),'explicit read-only search retry')
    c.click('[aria-label="清空搜索"]')
    c.policy(enabled=previous_policy['enabled'])
    print('PASS P2: native query deadline, stale Enter guard, late response suppression and explicit retry',flush=True)


def run_manual(c):
    original=c.action('clipboard_status')['policy']
    c.activate(c.window('^UClip$'))
    c.policy(enabled=False)
    wait(lambda:c.execute('return document.querySelector(".capture-toggle")?.textContent.includes("开始采集")'),'paused manual create UI')
    text='  手工保存合成文本 🦀\n第二行\n\n末尾空白  '
    c.click('[aria-label="更多选项"]')
    c.click('[data-testid="new-text"]')
    c.set('text',text=text)
    c.click('#clip-content')
    wait(lambda:c.execute('return document.hasFocus() && document.activeElement===document.querySelector("#clip-content")'),'native manual editor has input focus')
    c.key('key','--clearmodifiers','ctrl+v')
    wait(lambda:c.execute('return document.querySelector("#clip-content").value===arguments[0]',[text]),'exact Unicode and multiline manual input')
    c.click('.new-form .primary-button')
    wait(lambda:not c.execute('return !!document.querySelector(".new-form")'),'manual form closes only after successful save')
    items=c.query(text)['items']
    assert len(items)==1 and items[0]['manual'],items
    c.manual_id=items[0]['id'];c.manual_text=text
    other=c.action('create_text',text=text)
    assert other!=c.manual_id and c.query(text)['total']==2
    c.action('copy_clip',id=c.manual_id,paste=False)
    c.received('text',text)
    c.policy(excludePattern='^private')
    before=c.query()['revision']
    c.click('[aria-label="更多选项"]');c.click('[data-testid="new-text"]');c.enter('#clip-content','private fixture')
    c.click('.new-form .primary-button')
    wait(lambda:c.execute('return document.querySelector(".toast")?.textContent.includes("排除规则")'),'manual exclusion is reported')
    assert c.execute('return !!document.querySelector(".new-form")')
    assert c.query()['revision']==before and c.query('private fixture')['total']==0
    c.click('[aria-label="关闭面板"]')
    c.policy(enabled=original['enabled'],excludePattern=original['excludePattern'])
    c.total=c.query()['total']
    print('PASS P1: real manual-create UI while paused, distinct identities, exact copy, exclusion failure retains form and revision',flush=True)


def run_keyboard(c):
    import copy
    c.activate(c.window('^UClip$'))
    status=c.action('desktop_status');snapshot=status['keyboard'];default=copy.deepcopy(snapshot['bindings'])
    duplicate=copy.deepcopy(default);duplicate['search']=duplicate['copy']
    refused=c.invoke('update_key_bindings',{'request':{'bindings':duplicate,'expectedRevision':snapshot['revision']}})
    assert not refused['ok'] and refused['error']['code']=='shortcut_conflict',refused
    assert c.action('desktop_status')['keyboard']==snapshot
    c.click('[data-testid="open-settings"]')
    c.click('.about-section:has([aria-label="修改复制快捷键"]) summary')
    c.click('[aria-label="修改复制快捷键"]')
    c.key('key','--clearmodifiers','ctrl+Return')
    c.click('[data-testid="save-window-keys"]')
    saved=wait(lambda:(p if (p:=c.action('desktop_status')['keyboard'])['bindings']['copy']['primary'] else None),'custom copy binding commits')
    wait(lambda:c.execute('return document.querySelector("[data-testid=save-window-keys]").closest("details").querySelector("[role=status]").textContent.includes("已保存")'),'saved binding reaches the active UI')
    c.click('[aria-label="关闭面板"]')
    original=c.action('clipboard_status')['policy'];c.policy(enabled=False)
    c.enter('#history-search','手工保存合成文本')
    wait(lambda:c.execute('return document.querySelectorAll(".clip-row").length===2 && document.querySelector(".history-scroll").getAttribute("aria-busy")==="false"'),'manual matches for key binding')
    c.set('text',text='public custom-key sentinel')
    c.key('key','--clearmodifiers','Return')
    assert c.source.send({'op':'read','kind':'text'})['text']=='public custom-key sentinel'
    c.key('key','--clearmodifiers','ctrl+Return')
    wait(lambda:c.source.send({'op':'read','kind':'text'})['text']==c.manual_text,'custom shortcut copies the current effective selection')
    c.click('[aria-label="清空搜索"]')
    stale=c.invoke('update_key_bindings',{'request':{'bindings':default,'expectedRevision':snapshot['revision']}})
    assert not stale['ok'] and stale['error']['code']=='conflict',stale
    c.click('[data-testid="open-settings"]')
    c.click('.about-section:has([aria-label="修改复制快捷键"]) summary')
    # The default action is a draft until the explicit save button succeeds.
    c.click('.about-section:has([aria-label="修改复制快捷键"]) .form-actions .secondary-button')
    c.click('[data-testid="save-window-keys"]')
    wait(lambda:c.action('desktop_status')['keyboard']['bindings']==default,'restore default bindings')
    wait(lambda:c.execute('return document.querySelector("[data-testid=save-window-keys]").closest("details").querySelector("[role=status]").textContent.includes("已保存")'),'default bindings reach the active UI')
    c.click('[aria-label="关闭面板"]')
    c.policy(enabled=original['enabled']);c.total=c.query()['total']
    print('PASS P2: actual keyboard recorder/save, duplicate/stale rejection, customized copy obeys current selection and old Enter is inactive',flush=True)
