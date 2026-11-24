"""Measure actual native UI visibility and wake-up, using public synthetic data only."""
import json
from pathlib import Path
import platform
import subprocess
import time
from clipboard_native_check import wait


def distribution(values):
    values=sorted(values)
    return {'samples':len(values),'p50Ms':round(values[(len(values)-1)//2],2),'p95Ms':round(values[max(0,(95*len(values)+99)//100-1)],2),'maxMs':round(values[-1],2)}


def run(c):
    c.policy(enabled=True)
    c.execute('document.querySelector(".toast .icon-button")?.click();return true')
    c.action('set_desktop_interaction',active=False)
    visible=[]
    for i in range(30):
        label=f'public performance fixture {i:02}'
        started=c.set('text',text=label)
        wait(lambda:c.execute('return [...document.querySelectorAll(".clip-row")].some(e=>e.textContent.includes(arguments[0]))',[label]),'capture visible in real native UI')
        visible.append((time.monotonic()-started)*1000)
    wake=measure_wake(c)
    c.execute('''
      window.__frameSamples=[];window.__frameDone=false;window.__frameStable=true;
      const list=document.querySelector(".history-scroll"), height=list.scrollHeight;
      let previous=performance.now(),end=previous+5000,direction=1;
      function frame(now){
        window.__frameSamples.push(now-previous);previous=now;
        window.__frameStable &&= list.scrollHeight===height;
        list.scrollTop+=direction*8;
        if(list.scrollTop+list.clientHeight>=list.scrollHeight)direction=-1;
        if(list.scrollTop<=0)direction=1;
        if(now<end)requestAnimationFrame(frame);else window.__frameDone=true;
      }
      requestAnimationFrame(frame);return true
    ''')
    wait(lambda:c.execute('return window.__frameDone'),'native frame sample')
    frames=c.execute('const now=[...document.querySelectorAll(".clip-row")].map(e=>e.dataset.clipId);return {samples:window.__frameSamples,stable:window.__frameStable,unique:new Set(now).size===now.length}')
    assert frames['stable'] and frames['unique'],frames
    c.execute('delete window.__frameSamples;delete window.__frameDone;delete window.__frameStable;return true')
    window=c.window('^UClip$')
    root=int(subprocess.check_output(['xdotool','getwindowpid',window],env=c.env,text=True).strip())
    seen=set()
    proportional=[]
    def rss(pid):
        if pid in seen:return 0
        seen.add(pid)
        try:
            status=Path(f'/proc/{pid}/status').read_text()
            amount=next(int(l.split()[1]) for l in status.splitlines() if l.startswith('VmRSS:'))
            proportional.append(next(int(l.split()[1]) for l in Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines() if l.startswith('Pss:')))
            children=Path(f'/proc/{pid}/task/{pid}/children').read_text().split()
            return amount+sum(rss(int(child)) for child in children)
        except (OSError,StopIteration):return 0
    result={
        'platform':platform.platform(),'session':'isolated authenticated X11, 96 DPI, real WebKit',
        'captureToUi':distribution(visible),'hotWake':distribution(wake),'scrollFrame':distribution(frames['samples']),
        'processFamilyRssKiB':rss(root),
        'notes':['50ms observation interval and xdotool/WebDriver overhead included','capture includes product status polling (750ms)','RSS sums application and descendant processes; shared pages can be counted more than once','debug binary; not a release performance guarantee'],
    }
    result['processFamilyPssKiB']=sum(proportional)
    result['processCount']=len(seen)
    # The acceptance dataset now includes the newly acknowledged performance captures.
    c.total=c.query()['total']
    output=Path('docs/previews/copy-paste/performance-native.json')
    output.parent.mkdir(parents=True,exist_ok=True)
    output.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print('PASS P2 native performance observations: '+json.dumps(result,ensure_ascii=False),flush=True)
    assert result['captureToUi']['p95Ms']<=1000,result
    assert result['hotWake']['p95Ms']<=200,result


def measure_wake(c):
    initial=c.window('^UClip$')
    pid=subprocess.check_output(['xdotool','getwindowpid',initial],env=c.env,text=True).strip()
    def mapped():
        result=subprocess.run(['xdotool','search','--all','--onlyvisible','--pid',pid,'--name','^UClip$'],env=c.env,text=True,capture_output=True)
        return result.stdout.splitlines() if result.returncode==0 else []
    def focused():
        windows=mapped()
        return windows and subprocess.check_output(['xdotool','getactivewindow'],env=c.env,text=True).strip() in windows and c.execute('return document.hasFocus()')
    wake=[]
    for iteration in range(20):
        c.action('hide_main_window')
        wait(lambda:not mapped(),'native hide completes before wake sample')
        started=time.monotonic()
        c.key('key','ctrl+shift+v')
        try:
            wait(focused,'native shortcut wake and focus')
        except Exception:
            active=subprocess.run(['xprop','-root','_NET_ACTIVE_WINDOW'],env=c.env,text=True,capture_output=True).stdout.strip()
            print('WAKE FAILURE '+json.dumps({'iteration':iteration,'mapped':mapped(),'pid':pid,'active':active,'document':c.execute('return {focus:document.hasFocus(),visibility:document.visibilityState}'),'desktop':c.action('desktop_status')},ensure_ascii=False),flush=True)
            raise
        wake.append((time.monotonic()-started)*1000)
    return wake


def measure_continuous_paste(c):
    from urllib.parse import unquote, urlparse
    output=c.directory/'continuous-paste.json'
    receiver=subprocess.Popen(['/usr/bin/python3','scripts/paste-receiver.py',str(output)],env=c.env,stdout=c.log,stderr=c.log)
    c.processes.append(receiver)
    target=c.window('^UClip native fixture receiver$')
    reports={}
    for kind in ['text','image','files']:
        times=[]
        for _ in range(20):
            output.unlink(missing_ok=True)
            c.activate(target)
            c.key('key','--clearmodifiers','ctrl+a')
            c.key('key','--clearmodifiers','BackSpace')
            c.key('key','--clearmodifiers','ctrl+shift+v')
            wait(lambda:c.action('clipboard_status')['pasteTarget'] and c.execute('return document.hasFocus()'),'continuous paste target and focus')
            started=time.monotonic()
            copied=c.action('copy_clip',id=c.ids[kind],paste=True)
            assert copied['copied'] and copied['pasteRequested'] and copied['usageSaved'],copied
            def result():
                try:return json.loads(output.read_text())
                except (OSError,json.JSONDecodeError):return None
            value=wait(result,'actual continuous paste receiver')
            times.append((time.monotonic()-started)*1000)
            assert value['kind']==kind,value
            if kind=='text':assert value['text']==c.text,value
            if kind=='image':assert {key:value[key] for key in ['width','height','rgba']}==c.image,value
            if kind=='files':assert [unquote(urlparse(uri).path) for uri in value['uris']]==c.files,value
        reports[kind]=distribution(times)
        assert reports[kind]['p95Ms']<=1000,reports
    Path('docs/previews/copy-paste').mkdir(parents=True,exist_ok=True)
    Path('docs/previews/copy-paste/performance-paste.json').write_text(json.dumps(reports,indent=2)+'\n')
    print('PASS P2 continuous paste to real GTK receiver: '+json.dumps(reports),flush=True)
    return reports
