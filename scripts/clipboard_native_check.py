"""Opt-in synthetic A/B checks used by native-smoke.py on a dedicated display."""
import json
import hashlib
import os
import select
import re
import sqlite3
import subprocess
import time
from pathlib import Path
from urllib.parse import unquote, urlparse

def wait(check, label, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = check()
        if value: return value
        time.sleep(0.05)
    raise RuntimeError('timeout: ' + label)

class Client:
    def __init__(self, command, env, stderr):
        self.buffer = b''
        self.process = subprocess.Popen(command, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr, text=True, bufsize=1)
    def read(self):
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            if b'\n' not in self.buffer:
                if not select.select([self.process.stdout], [], [], max(0, deadline-time.monotonic()))[0]: break
                chunk = os.read(self.process.stdout.fileno(), 65536)
                if not chunk: raise RuntimeError('fixture process exited')
                self.buffer += chunk
                continue
            line, self.buffer = self.buffer.split(b'\n', 1)
            if line.startswith(b'FIXTURE:'):
                value = json.loads(line[len(b'FIXTURE:'):])
                assert value['ok'], value
                return value['value']
        raise RuntimeError('fixture response timeout')
    def send(self, value):
        self.process.stdin.write(json.dumps(value, ensure_ascii=False) + '\n')
        self.process.stdin.flush()
        return self.read()
    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try: self.process.wait(timeout=5)
            except subprocess.TimeoutExpired: self.process.terminate(); self.process.wait(timeout=5)

class ClipboardChecks:
    def __init__(self, invoke, execute, click, enter, env, directory, log):
        if env.get('UCLIP_FIXTURE_DISPLAY') != '1' or env.get('DISPLAY') in (':0', ':0.0', None):
            raise RuntimeError('Use scripts/isolated-copy-paste.py; refusing the normal desktop clipboard.')
        self.invoke, self.execute, self.click, self.enter = invoke, execute, click, enter
        self.env, self.directory, self.log = env, Path(directory), log
        self.source = Client(['target/debug/examples/clipboard_fixture'], env, log)
        self.processes = []
        self.browser = None
        self.timings = []
        self.text = '  首版文本 · 中文 🦀\n第二行\n\n保留空白  '
        self.image = {'width': 2, 'height': 2, 'rgba': [12,34,56,0,255,0,99,128,7,8,9,255,100,120,140,255]}
        self.files = [str(self.directory / '源 你好#%.txt'), str(self.directory / '文件夹 空格'), str(self.directory / '第二份.txt')]
        Path(self.files[0]).write_text('synthetic file A')
        Path(self.files[1]).mkdir(); Path(self.files[1], '内部.txt').write_text('synthetic folder')
        Path(self.files[2]).write_text('synthetic file B')
        self.ids = {}
    def action(self, command, **arguments):
        result = self.invoke(command, arguments)
        assert result['ok'], result
        return result['value']
    def query(self, text='', kind=None):
        return self.action('history_query', request={'query': text, 'regex': False, 'kind': kind, 'offset': 0, 'expectedRevision': None})
    def policy(self, **changes):
        p = self.action('clipboard_status')['policy']
        request = {'enabled': p['enabled'], 'excludePattern': p['excludePattern'], 'maxItems': p['maxItems'], 'expectedRevision': p['revision']}
        request.update(changes)
        return self.action('update_capture_policy', request=request)
    def set(self, kind, **fields):
        started = time.monotonic()
        self.source.send({'op': 'set', 'kind': kind, **fields})
        return started
    def received(self, kind, expected):
        started = time.monotonic()
        if kind == 'text': assert self.source.send({'op':'read','kind':kind})['text'] == expected
        if kind == 'image': assert self.source.send({'op':'read','kind':kind}) == expected
        if kind == 'files':
            paths = self.source.send({'op':'read','kind':kind})['files']
            assert [unquote(urlparse(p).path) if p.startswith('file:') else p for p in paths] == expected
        self.timings.append({'action':'system readback ' + kind,'ms':round((time.monotonic()-started)*1000,2)})
    def key(self, *args): subprocess.run(['xdotool', *args], env=self.env, check=True, stdout=subprocess.DEVNULL, timeout=5)
    def window(self, title):
        def find():
            p = subprocess.run(['xdotool','search','--onlyvisible','--name',title],env=self.env,text=True,capture_output=True)
            return p.stdout.splitlines()[-1] if p.returncode == 0 and p.stdout.strip() else None
        return wait(find, 'window ' + title)
    def activate(self, window):
        self.key('windowactivate',str(window))
        wait(lambda: subprocess.check_output(['xdotool','getactivewindow'],env=self.env,text=True).strip()==str(window), 'active window')
    def run(self):
        assert self.query()['total'] == 0
        self.set('text', text='既有内容不补采')
        self.click('.capture-toggle')
        wait(lambda: self.action('clipboard_status')['policy']['enabled'], 'capture enabled')
        time.sleep(0.2)
        assert self.query()['total'] == 0
        started = self.set('text', text=self.text)
        item = wait(lambda: next(iter(self.query('首版文本')['items']), None), 'text capture')
        self.ids['text'] = item['id']
        self.timings.append({'action':'text capture commit','ms':round((time.monotonic()-started)*1000,2)})
        assert self.action('clip_detail', id=item['id'], thumbnail=False)['text'] == self.text
        wait(lambda: self.execute('return document.querySelectorAll(".clip-row").length===1'), 'real history visible')
        self.click('.row-more')
        self.click('[data-testid="copy-history-item"]')
        wait(lambda: self.execute('return document.querySelector(".toast")?.textContent.includes("已复制")'), 'UI copy')
        self.received('text',self.text)
        time.sleep(0.2); assert self.query()['total']==1
        original = self.query()['revision']
        self.set('text',text='中间内容 B')
        wait(lambda:self.query('中间内容 B')['total']==1,'B capture')
        self.set('text',text=self.text)
        wait(lambda:self.query()['items'][0]['id']==self.ids['text'],'A B A identity')
        assert self.query()['total']==2 and self.query()['revision']!=original
        self.set('image',textAlternative=True,**self.image)
        image = wait(lambda:next(iter(self.query(kind='image')['items']),None),'image capture')
        self.ids['image']=image['id']
        self.action('copy_clip',id=image['id'],paste=False);self.received('image',self.image)
        self.set('image',width=1920,height=1080,pattern=True)
        large_image=wait(lambda:next((item for item in self.query(kind='image')['items'] if item['width']==1920),None),'large image capture')
        self.ids['large_image']=large_image['id']
        self.action('copy_clip',id=large_image['id'],paste=False)
        digest=self.source.send({'op':'read','kind':'image_digest'})
        expected=hashlib.sha256(b'image\0'+(1920).to_bytes(4,'little')+(1080).to_bytes(4,'little')+bytes([30,70,100,128])*(1920*1080)).digest()
        assert digest['width']==1920 and digest['height']==1080 and bytes(digest['fingerprint'])==expected

        self.set('files',files=self.files,cut=True)
        files=wait(lambda:next(iter(self.query(kind='files')['items']),None),'files capture')
        self.ids['files']=files['id'];assert files['fileCount']==3
        assert self.action('clip_detail',id=files['id'],thumbnail=False)['text']== '\n'.join(self.files)
        self.action('copy_clip',id=files['id'],paste=False);self.received('files',self.files)
        for index in range(12):
            self.set('text', text=f'连续复制 fixture {index}')
            time.sleep(0.08)
        wait(lambda:self.query('连续复制 fixture')['total']==12,'consecutive 80ms text copies')
        large='大文本 fixture\n' + ('x' * (1024 * 1024))
        large_source=subprocess.Popen(['xclip','-selection','clipboard','-in','-quiet'],env=self.env,stdin=subprocess.PIPE,stdout=self.log,stderr=self.log);self.processes.append(large_source)
        large_source.stdin.write(large.encode());large_source.stdin.close()
        try:
            large_item=wait(lambda:next(iter(self.query('大文本 fixture')['items']),None),'xclip INCR large text capture')
        except Exception:
            print('INCR DIAGNOSTIC '+json.dumps({'sourceExit':large_source.poll(),'status':self.action('clipboard_status')},ensure_ascii=False),flush=True)
            raise
        assert self.action('clip_detail',id=large_item['id'],thumbnail=False)['text']==large
        print('PASS A: text/image/ordered file and folder capture, real system writeback, no internal recapture, A-B-A identity',flush=True)

        # Pause acknowledgement, sensitive marker, exclusion and invalid rule preservation.
        self.policy(enabled=False)
        count=self.query()['total'];self.set('text',text='暂停不保存');time.sleep(0.2);assert self.query()['total']==count
        self.policy(enabled=True);time.sleep(0.2);assert self.query()['total']==count
        self.set('text',text='敏感格式 fixture',sensitive=True);time.sleep(0.2);assert self.query()['total']==count
        self.policy(excludePattern='^'+re.escape(self.files[0])+'$')
        revision=self.query()['revision']
        self.set('files',files=self.files)
        time.sleep(0.3)
        assert self.query()['revision']==revision, 'Anchored exclusion must match an actual file path before deduplication'
        self.policy(excludePattern='排除');self.set('text',text='排除 fixture');time.sleep(0.2);assert self.query()['total']==count
        old=self.action('clipboard_status')['policy'];bad=self.invoke('update_capture_policy',{'request':{'enabled':True,'excludePattern':'[','maxItems':500,'expectedRevision':old['revision']}})
        assert not bad['ok'] and bad['error']['code']=='invalid_pattern'
        assert self.action('clipboard_status')['policy']['revision']==old['revision']
        self.policy(enabled=False,excludePattern='')
        Path(self.files[0]).unlink();self.set('text',text='failure sentinel')
        failed=self.invoke('copy_clip',{'id':self.ids['files'],'paste':False})
        assert not failed['ok'] and failed['error']['code']=='missing_file';self.received('text','failure sentinel')
        Path(self.files[0]).write_text('synthetic file A')
        self.policy(enabled=True)
        self.set('text',text='恢复后新复制');wait(lambda:self.query('恢复后新复制')['total']==1,'resume new external copy')
        print('PASS A: pause/resume boundary, sensitive formats, persisted exclusion, invalid pattern rejection, missing-file atomic refusal',flush=True)
        database = next(self.directory.joinpath('data').rglob('foundation.sqlite3'))
        connection = sqlite3.connect(database)
        connection.execute("CREATE TRIGGER fail_usage BEFORE UPDATE OF last_used_at ON clips BEGIN SELECT RAISE(ABORT,'fixture'); END;")
        connection.commit()
        partial = self.action('copy_clip', id=self.ids['text'], paste=False)
        assert partial['copied'] and not partial['usageSaved'] and partial['warning']['code']=='storage'
        self.received('text',self.text)
        connection.execute('DROP TRIGGER fail_usage');connection.commit()
        connection.execute('BEGIN IMMEDIATE')
        self.set('text',text='数据库忙后恢复 fixture')
        wait(lambda:(self.action('clipboard_status').get('error') or {}).get('code')=='busy','busy feedback',timeout=8)
        self.set('text',text='数据库忙期间新复制 fixture')
        connection.rollback();connection.close()
        wait(lambda:self.query('数据库忙后恢复 fixture')['total']==1 and self.query('数据库忙期间新复制 fixture')['total']==1,'original pending snapshot and latest clipboard both survive busy')
        print('PASS A: OS copy succeeds independently of usage-save failure; database busy does not acknowledge loss and same snapshot retries once recovered',flush=True)
        count=self.query()['total']
        oversized=self.source.process
        self.set('text',text='x'*(4*1024*1024+1))
        try:
            wait(lambda:(self.action('clipboard_status').get('error') or {}).get('code')=='limit_exceeded','native over-limit refusal')
        except RuntimeError:
            print('Over-limit diagnostic:', {'error':self.action('clipboard_status')['error'], 'sourceExit':oversized.poll(), 'total':self.query()['total']}, flush=True)
            raise
        assert self.query()['total']==count
        self.set('text',text='超限后新复制 fixture')
        wait(lambda:self.query('超限后新复制 fixture')['total']==1,'fresh copy after rejected generation')
        print('PASS A: OS over-limit content rejected without truncation; next external copy is captured',flush=True)



        # Native editor receives an actual Ctrl+V, then saves its buffer for comparison.
        editor_file=self.directory/'editor-receive.txt';editor_file.touch()
        editor=subprocess.Popen(['xed','--standalone','--new-window',str(editor_file)],env=self.env,stdout=self.log,stderr=self.log);self.processes.append(editor)
        editor_window=self.window('editor-receive.txt')
        self.action('copy_clip',id=self.ids['text'],paste=False);self.activate(editor_window);self.key('key','--clearmodifiers','ctrl+v')
        wait(lambda: '*' in subprocess.check_output(['xdotool','getwindowname',str(editor_window)],env=self.env,text=True),'editor buffer changed')
        self.key('key','--clearmodifiers','ctrl+s')
        wait(lambda:editor_file.read_text() in (self.text, self.text+'\n'),'native editor actual paste/save')
        editor_revision=self.query()['revision']
        self.activate(editor_window)
        self.key('key','--clearmodifiers','ctrl+a');self.key('key','--clearmodifiers','ctrl+c')
        editor_copy = wait(lambda: self.source.send({'op':'read','kind':'text'}).get('text'), 'editor clipboard text')
        assert editor_copy in (self.text, self.text+'\n'), {'editor_clipboard_length': len(editor_copy)}
        wait(lambda:self.query()['revision'] != editor_revision and bool(self.query('首版文本')['items']) and any(self.action('clip_detail',id=item['id'],thumbnail=False)['text']==editor_copy for item in self.query('首版文本')['items']),'native editor actual source copy')
        subprocess.run(['wmctrl','-ic',hex(int(editor_window))],env=self.env,check=True)

        # Native file manager must create real files only under the disposable receive directory.
        receive=self.directory/'file-receive-fixture';receive.mkdir()
        manager=subprocess.Popen(['nemo','--no-desktop','--no-default-window',str(receive)],env=self.env,stdout=self.log,stderr=self.log);self.processes.append(manager)
        manager_window=self.window('file-receive-fixture')
        self.action('copy_clip',id=self.ids['files'],paste=False);self.activate(manager_window);self.key('key','--clearmodifiers','ctrl+v')
        wait(lambda:(receive/Path(self.files[0]).name).exists() and (receive/Path(self.files[1]).name/'内部.txt').exists() and (receive/Path(self.files[2]).name).exists(),'native file manager receive')
        assert (receive/Path(self.files[0]).name).read_bytes()==Path(self.files[0]).read_bytes()
        assert all(Path(p).exists() for p in self.files)
        subprocess.run(['wmctrl','-ic',hex(int(manager_window))],env=self.env,check=True)
        print('PASS A: actual xed copy/paste and Nemo multi-file/folder receive, source files preserved',flush=True)

        module=self.env.get('UCLIP_PLAYWRIGHT_MODULE')
        if module:
            self.browser=Client(['node','scripts/browser-paste-receiver.mjs',module],self.env,self.log);self.browser.read()
            self.action('copy_clip',id=self.ids['text'],paste=False);assert self.browser.send({'op':'paste'})['text']==self.text
            self.action('copy_clip',id=self.ids['image'],paste=False);actual=self.browser.send({'op':'paste'})
            assert actual['kind']=='image' and actual['width']==2 and actual['height']==2
            for index,value in enumerate(self.image['rgba']):
                if index%4==3: assert actual['rgba'][index]==value
                elif self.image['rgba'][index//4*4+3]: assert abs(actual['rgba'][index]-value)<=2
            self.browser.send({'op':'copy','text':'浏览器实际复制 fixture'});wait(lambda:self.query('浏览器实际复制 fixture')['total']==1,'headed browser actual source copy')
            print('PASS A: headed Chromium actual native text/image paste and textarea source copy',flush=True)

        # Controlled GTK receiver independently decodes native image/file formats when Ctrl+V arrives.
        result=self.directory/'paste-result.json'
        receiver=subprocess.Popen(['/usr/bin/python3','scripts/paste-receiver.py',str(result)],env=self.env,stdout=self.log,stderr=self.log);self.processes.append(receiver)
        window=self.window('^UClip native fixture receiver$')
        assert self.action('clipboard_status')['shortcutAvailable']
        for kind in ['text','image','files']:
            if result.exists():result.unlink()
            self.activate(window);self.key('key','--clearmodifiers','ctrl+a');self.key('key','--clearmodifiers','BackSpace')
            self.key('key','--clearmodifiers','ctrl+shift+v')
            wait(lambda:self.action('clipboard_status')['pasteTarget'],'target recorded')
            wait(lambda: self.execute('return document.hasFocus()'),'app globally activated')
            copied=self.action('copy_clip',id=self.ids[kind],paste=True)
            assert copied['copied'] and copied['pasteRequested'],copied
            def received_result():
                try: return json.loads(result.read_text())
                except (OSError, json.JSONDecodeError): return None
            value=wait(received_result,'GTK automatic '+kind+' paste')
            assert value['kind']==kind,value
            if kind=='text': assert value['text']==self.text,value
            if kind=='image': assert {k:value[k] for k in ['width','height','rgba']}==self.image,value
            if kind=='files': assert [unquote(urlparse(uri).path) for uri in value['uris']]==self.files,value
        # Change the actual server map after startup, then require GTK to receive
        # the original text through the newly mapped V key. Only this nested display is touched.
        from Xlib import X, XK, display as xdisplay
        from Xlib.ext import xtest
        import concurrent.futures
        import signal
        display = xdisplay.Display()
        try:
            self.activate(window);self.key('key','--clearmodifiers','ctrl+a');self.key('key','--clearmodifiers','BackSpace')
            self.key('key','--clearmodifiers','ctrl+shift+v')
            wait(lambda:self.execute('return document.hasFocus()'),'main focus before key remap')
            v = display.keysym_to_keycode(XK.string_to_keysym('v'))
            x = display.keysym_to_keycode(XK.string_to_keysym('x'))
            original_v = display.get_keyboard_mapping(v, 1)
            original_x = display.get_keyboard_mapping(x, 1)
            try:
                display.change_keyboard_mapping(v, original_x)
                display.change_keyboard_mapping(x, original_v)
                display.sync();time.sleep(0.2)
                result.unlink(missing_ok=True)
                copied = self.action('copy_clip',id=self.ids['text'],paste=True)
                assert copied['copied'] and copied['pasteRequested'],copied
                value = wait(received_result,'GTK actual paste after live V/X remap')
                assert value['kind']=='text' and value['text']==self.text,value
            finally:
                display.change_keyboard_mapping(v, original_v)
                display.change_keyboard_mapping(x, original_x)
                display.sync();time.sleep(0.2)
            print('PASS B: live X11 V/X remapping resolves fresh keycodes and GTK receives exact text',flush=True)

            # Pause only the isolated WM to control activation and modifier races.
            wm_pid = int(self.env['UCLIP_FIXTURE_WM_PID'])
            root = display.screen().root
            root.change_attributes(event_mask=X.SubstructureNotifyMask)
            active_atom = display.intern_atom('_NET_ACTIVE_WINDOW')
            shift = display.keysym_to_keycode(XK.string_to_keysym('Shift_L'))
            # Model a WM publishing the active window before it moves input focus.
            # The app must allow that transition to finish outside its server guard.
            from Xlib import Xatom
            self.activate(window);self.key('key','--clearmodifiers','ctrl+a');self.key('key','--clearmodifiers','BackSpace')
            self.key('key','--clearmodifiers','ctrl+shift+v')
            wait(lambda:self.execute('return document.hasFocus()'),'main focus before delayed input focus')
            display.sync()
            while display.pending_events():display.next_event()
            result.unlink(missing_ok=True)
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                os.kill(wm_pid,signal.SIGSTOP)
                try:
                    pending = pool.submit(self.action,'copy_clip',id=self.ids['text'],paste=True)
                    def activation_requested():
                        while display.pending_events():
                            event = display.next_event()
                            if event.type == X.ClientMessage and event.client_type == active_atom and event.window.id == int(window):return True
                        return False
                    wait(activation_requested,'activation before delayed input focus',timeout=2)
                    root.change_property(active_atom,Xatom.WINDOW,32,[int(window)])
                    display.sync();time.sleep(.1)
                    assert not pending.done(),'activation was rejected before the WM could move input focus'
                    os.kill(wm_pid,signal.SIGCONT)
                    copied = pending.result(timeout=6)
                    assert copied['copied'] and copied['pasteRequested'],copied
                    value = wait(received_result,'GTK receive after delayed WM focus')
                    assert value['kind']=='text' and value['text']==self.text,value
                finally:
                    os.kill(wm_pid,signal.SIGCONT)
            print('PASS B: delayed input focus after active-window publication is awaited before guarded paste',flush=True)

            # Hold Shift after activation is requested to exercise the final check.
            self.activate(window);self.key('key','--clearmodifiers','ctrl+shift+v')
            wait(lambda:self.execute('return document.hasFocus()'),'main focus before modifier race')
            display.sync()
            while display.pending_events():display.next_event()
            result.unlink(missing_ok=True)
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                os.kill(wm_pid,signal.SIGSTOP)
                try:
                    pending = pool.submit(self.action,'copy_clip',id=self.ids['text'],paste=True)
                    def activation_requested():
                        while display.pending_events():
                            event = display.next_event()
                            if event.type == X.ClientMessage and event.client_type == active_atom and event.window.id == int(window):return True
                        return False
                    wait(activation_requested,'paste activation sent while isolated WM paused',timeout=2)
                    xtest.fake_input(display,X.KeyPress,shift);display.sync()
                    os.kill(wm_pid,signal.SIGCONT)
                    copied = pending.result(timeout=6)
                    assert copied['copied'] and not copied['pasteRequested'],copied
                    assert copied['warning']['code']=='target_lost',copied
                    time.sleep(0.15)
                    assert not result.exists(),'unexpected paste during held Shift'
                finally:
                    os.kill(wm_pid,signal.SIGCONT)
                    xtest.fake_input(display,X.KeyRelease,shift);display.sync()
            print('PASS B: Shift pressed during WM activation prevents final key injection',flush=True)
        finally:
            display.close()
        self.activate(window);self.key('key','--clearmodifiers','ctrl+shift+v');wait(lambda:self.execute('return document.hasFocus()'),'app focus before closed target')
        subprocess.run(['wmctrl','-ic',hex(int(window))],env=self.env,check=True);wait(lambda:receiver.poll() is not None,'target closed')
        failed=self.invoke('copy_clip',{'id':self.ids['text'],'paste':True});assert not failed['ok'] and failed['error']['code']=='target_lost',failed
        print('PASS B (X11): Ctrl+Shift+V target capture, focus restoration and actual GTK text/image/files receive; closed target safely refused',flush=True)
        # A valid recorded target must not authorize paste after the user moves elsewhere.
        other_result=self.directory/'other-result.json'
        other=subprocess.Popen(['/usr/bin/python3','scripts/paste-receiver.py',str(other_result)],env=self.env,stdout=self.log,stderr=self.log);self.processes.append(other)
        other_window=self.window('^UClip native fixture receiver$')
        self.activate(other_window);self.key('key','--clearmodifiers','ctrl+shift+v')
        wait(lambda:self.execute('return document.hasFocus()'),'main focus before user switch')
        self.activate(other_window)
        switched=self.invoke('copy_clip',{'id':self.ids['text'],'paste':True})
        assert not switched['ok'] and switched['error']['code']=='target_lost',switched
        assert not other_result.exists()
        print('PASS B: user foreground change refuses automatic paste instead of stealing focus',flush=True)
        # Exercise the actual row menu, committed deletion, undo and durable re-deletion.
        self.set('text',text='删除交互验证 fixture')
        removed=wait(lambda:next(iter(self.query('删除交互验证 fixture')['items']),None),'deletion fixture capture')
        self.deleted_id=removed['id']
        self.enter('#history-search','删除交互验证 fixture')
        wait(lambda:self.execute('return document.querySelectorAll(".clip-row").length===1 && document.querySelector(".history-scroll").getAttribute("aria-busy")==="false"'),'delete row visible')
        def delete_visible():
            self.click('[data-row-main]');self.click('.row-more');self.click('.action-menu .destructive')
            wait(lambda:self.query('删除交互验证 fixture')['total']==0,'committed deletion')
            wait(lambda:self.execute('return document.querySelectorAll(".clip-row").length===0'),'deleted row disappears')
        delete_visible()
        self.click('.toast .text-button')
        wait(lambda:self.query('删除交互验证 fixture')['total']==1,'undo commits')
        wait(lambda:self.execute('return document.querySelectorAll(".clip-row").length===1 && document.querySelector(".history-scroll").getAttribute("aria-busy")==="false"'),'undo row visible')
        delete_visible()
        missing=self.invoke('copy_clip',{'id':removed['id'],'paste':False})
        assert not missing['ok'] and missing['error']['code']=='not_found'
        self.click('[aria-label="清空搜索"]')
        print('PASS: native row deletion/undo/re-deletion commits, deleted content cannot be copied',flush=True)
        self.total=self.query()['total']
        return self
    def replay(self):
        assert self.query()['total']==self.total
        if hasattr(self,'deleted_id'):
            missing=self.invoke('clip_detail',{'id':self.deleted_id,'thumbnail':False})
            assert not missing['ok'] and missing['error']['code']=='not_found'
        if hasattr(self,'manual_id'):
            self.action('copy_clip',id=self.manual_id,paste=False);self.received('text',self.manual_text)
        self.action('copy_clip',id=self.ids['text'],paste=False);self.received('text',self.text)
        self.action('copy_clip',id=self.ids['image'],paste=False);self.received('image',self.image)
        self.action('copy_clip',id=self.ids['files'],paste=False);self.received('files',self.files)
        self.action('copy_clip',id=self.ids['large_image'],paste=False)
        digest=self.source.send({'op':'read','kind':'image_digest'})
        expected=hashlib.sha256(b'image\0'+(1920).to_bytes(4,'little')+(1080).to_bytes(4,'little')+bytes([30,70,100,128])*(1920*1080)).digest()
        assert bytes(digest['fingerprint'])==expected
        print('PASS A: all three persisted types survive actual process close/restart and write back unchanged',flush=True)
        print('Fixture timings:',json.dumps(self.timings),flush=True)
    def close(self):
        if self.browser:self.browser.close()
        self.source.close()
        for process in self.processes:
            if process.poll() is None:
                process.terminate()
                try:process.wait(timeout=5)
                except subprocess.TimeoutExpired:process.kill();process.wait()
