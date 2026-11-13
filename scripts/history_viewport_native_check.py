"""Native WebKit scroll checks against real history refreshes and controlled late resizes."""


def run(invoke, execute, click, wait, capture):
    for index in range(24):
        assert invoke('create_text', {'text': f'滚动稳定性合成记录 {index}', 'collectionId': None})['ok']
    wait(lambda: execute('return document.querySelectorAll(".clip-row").length===25 && document.querySelector(".history-scroll").getAttribute("aria-busy")==="false"'), 'full history page')
    assert execute('return [...document.querySelectorAll(".clip-row")].every(e=>e.getBoundingClientRect().height===56)')
    execute('''
      const viewport=document.querySelector('.history-scroll'), row=viewport.querySelectorAll('[data-clip-id]')[10];
      viewport.scrollTop+=row.getBoundingClientRect().top-viewport.getBoundingClientRect().top-18;
      row.querySelector('[data-row-main]').focus({preventScroll:true});
      window.__scrollFixture={id:row.dataset.clipId,offset:row.getBoundingClientRect().top-viewport.getBoundingClientRect().top,ready:false};
      requestAnimationFrame(()=>requestAnimationFrame(()=>window.__scrollFixture.ready=true)); return true;
    ''')
    wait(lambda: execute('return window.__scrollFixture.ready'), 'user scroll settled')

    def aligned():
        return execute('''const f=window.__scrollFixture,v=document.querySelector('.history-scroll'),r=[...v.querySelectorAll('[data-clip-id]')].find(e=>e.dataset.clipId===f.id);return !!r && Math.abs(r.getBoundingClientRect().top-v.getBoundingClientRect().top-f.offset)<=1''')

    added = invoke('create_text', {'text': '顶部插入的合成记录', 'collectionId': None})
    assert added['ok']
    wait(lambda: execute('return document.querySelector(".clip-row").dataset.clipId===arguments[0] && document.querySelector(".history-scroll").getAttribute("aria-busy")==="false"', [added['value']]), 'background insertion reached main UI')
    assert aligned(), 'prepending a record moved the record being read'
    assert execute('return document.querySelector(".clip-row.selected").dataset.clipId===window.__scrollFixture.id'), 'background insertion changed selection'

    # Resize an offscreen row as a delayed media layout would, without capture privileges.
    execute('document.querySelector(".clip-row").style.height="140px";return true')
    wait(aligned, 'late row expansion preserves viewport')
    execute('document.querySelector(".clip-row").style.height="";return true')
    wait(aligned, 'late row collapse preserves viewport')

    selector = '[data-clip-id="' + execute('return window.__scrollFixture.id') + '"]'
    click(selector + ' .row-more')
    click('.action-menu button:nth-child(2)')
    wait(lambda: execute('return document.querySelector(".history-panel").hidden'), 'preview opened')
    click('[aria-label="返回历史列表"]')
    wait(aligned, 'preview return preserves viewport')

    # Opening preview in the same task as a programmatic scroll must save the
    # pre-hide DOM snapshot even before the asynchronous scroll event fires.
    execute('''
      const v=document.querySelector('.history-scroll');v.scrollTop+=112;
      const r=[...v.querySelectorAll('[data-clip-id]')].find(e=>e.getBoundingClientRect().bottom>v.getBoundingClientRect().top);
      window.__scrollFixture={id:r.dataset.clipId,offset:r.getBoundingClientRect().top-v.getBoundingClientRect().top};
      v.dispatchEvent(new KeyboardEvent('keydown',{key:' ',code:'Space',bubbles:true,cancelable:true}));return true;
    ''')
    wait(lambda: execute('return document.querySelector(".history-panel").hidden'), 'preview opened before scroll event')
    click('[aria-label="返回历史列表"]')
    wait(aligned, 'pre-hide snapshot survives preview without a scroll event')

    execute('const s=document.querySelector("[aria-label=内容类型]");s.value="image";s.dispatchEvent(new Event("change",{bubbles:true}));return true')
    wait(lambda: execute('return document.querySelectorAll(".clip-row").length===0 && document.querySelector(".history-scroll").getAttribute("aria-busy")==="false"'), 'explicit filter completed')
    assert execute('return document.querySelector(".history-scroll").scrollTop===0')
    execute('const s=document.querySelector("[aria-label=内容类型]");s.value="all";s.dispatchEvent(new Event("change",{bubbles:true}));return true')
    wait(lambda: execute('return document.querySelectorAll(".clip-row").length===25 && document.querySelector(".history-scroll").getAttribute("aria-busy")==="false"'), 'filter cleared')
    assert execute('return document.querySelector(".history-scroll").scrollTop===0')

    added = invoke('create_text', {'text': '顶部浏览时的新记录', 'collectionId': None})
    assert added['ok']
    wait(lambda: execute('return document.querySelector(".clip-row").dataset.clipId===arguments[0]', [added['value']]), 'top follows newest record')
    assert execute('return document.querySelector(".history-scroll").scrollTop===0')
    capture('stable-scroll-main')
    print('PASS viewport: 56px text rows, background prepend and selection, late expansion/collapse, preview return, filter reset and top-follow', flush=True)
