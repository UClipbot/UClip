"""Real WebKit pin and dedup checks, using only the isolated synthetic X11 session."""
from clipboard_native_check import wait


def history(c, pinned=False, offset=0, revision=None):
    return c.action('history_query', request={'query': '', 'regex': False, 'kind': None,
                    'pinnedOnly': pinned, 'offset': offset, 'expectedRevision': revision})


def settled(c, count=None):
    return wait(lambda: c.execute('return document.querySelector(".history-scroll")?.getAttribute("aria-busy")==="false" && (!arguments[0] || document.querySelectorAll(".clip-row").length===arguments[1])', [count is not None, count]), 'effective native history page')


def scope(c, value):
    c.click(f'[aria-label="历史范围"] option[value="{value}"]')
    wait(lambda: c.execute('return document.querySelector("[aria-label=历史范围]").value===arguments[0]', [value]), 'history scope')
    settled(c)


def pin_ui(c, clip_id):
    wait(lambda: c.execute('return !!document.querySelector(`[data-clip-id="${arguments[0]}"]`)', [clip_id]), 'pin target visible')
    settled(c)
    c.click(f'[data-clip-id="{clip_id}"] [data-row-main]')
    c.click(f'[data-clip-id="{clip_id}"] .row-more')
    c.click('[data-testid="toggle-pin"]')
    wait(lambda: c.execute('return document.querySelector(".toast")?.textContent.includes("已置顶")'), 'pin commit feedback')
    wait(lambda: any(i['id'] == clip_id and i['pinned'] for i in history(c, True)['items']), 'pin persisted')
    settled(c)


def run(c, capture):
    assert not c.action('clipboard_status')['policy']['enabled']
    c.activate(c.window('^UClip$'))
    c.pinned_text = '置顶合成样本 🦀\n' + '完整内容 '.ljust(330, '长') + '\n保留结尾空白  '
    c.pinned_id = c.action('create_text', text=c.pinned_text)
    pin_ui(c, c.pinned_id)
    duplicate = c.action('create_text', text=c.pinned_text)
    second_pin = c.action('create_text', text=c.pinned_text)
    pin_ui(c, second_pin)
    c.second_pin = second_pin
    c.survivor = c.action('create_text', text=c.pinned_text + '不同尾部')
    scope(c, 'pinned')
    settled(c, 2)
    c.enter('#history-search', '置顶合成样本')
    settled(c, 2)
    c.click('[aria-label="置顶页去重"]')
    assert c.execute('return document.querySelector("[role=dialog]").textContent.includes("当前搜索、类型筛选和页码不限制去重范围")')
    c.key('key', '--clearmodifiers', 'Escape')
    wait(lambda: not c.execute('return !!document.querySelector("[role=dialog]")'), 'cancel dedup with Escape')
    assert history(c)['total'] == 4

    c.click('[aria-label="置顶页去重"]')
    c.action('create_text', text='concurrent history change')
    c.click('.form-actions .primary-button')
    wait(lambda: c.execute('return document.querySelector("[role=dialog] [role=alert]")?.textContent.includes("数据已发生变化")'), 'dedup rejects changed history')
    assert history(c)['total'] == 5
    c.click('[aria-label="关闭面板"]')
    settled(c)
    c.click('[aria-label="置顶页去重"]')
    c.click('.form-actions .primary-button')
    wait(lambda: c.execute('return document.querySelector(".toast")?.textContent.includes("已删除 1 条")'), 'actual dedup count')
    assert history(c)['total'] == 4
    assert history(c, True)['total'] == 2
    missing = c.invoke('clip_detail', {'id': duplicate, 'thumbnail': False})
    assert not missing['ok'] and missing['error']['code'] == 'not_found', missing
    assert c.action('clip_detail', id=c.survivor, thumbnail=False)['text'].endswith('不同尾部')
    c.action('copy_clip', id=c.pinned_id, paste=False)
    c.received('text', c.pinned_text)
    settled(c)

    # Unpin from a real preview and return focus to the remaining effective list.
    c.click(f'[data-clip-id="{second_pin}"] [data-row-main]')
    c.key('key', '--clearmodifiers', 'space')
    wait(lambda: c.execute('return !!document.querySelector(".detail-panel")'), 'real pinned preview')
    c.click('[aria-label="取消置顶"]')
    wait(lambda: history(c, True)['total'] == 1, 'preview unpin')
    settled(c, 1)
    wait(lambda: c.execute('return !document.querySelector(".detail-panel") && document.activeElement===document.querySelector(".history-scroll")'), 'unpin exits preview with list focus')
    # This new non-pinned copy is removed by a second explicit dedup.
    c.click('[aria-label="置顶页去重"]')
    c.click('.form-actions .primary-button')
    wait(lambda: c.execute('return document.querySelector(".toast")?.textContent.includes("已删除 1 条")'), 'unpin then explicit dedup')
    c.click('[aria-label="清空搜索"]')
    settled(c, 1)

    # Verify both themes at the native minimum window width.
    c.key('windowsize', c.window('^UClip$'), '480', '650')
    wait(lambda: c.execute('return innerWidth===480'), 'minimum native window width')
    for theme in ['light', 'dark']:
        c.click('[data-testid="open-settings"]')
        c.click(f'#theme option[value="{theme}"]')
        wait(lambda: c.execute('return document.documentElement.dataset.theme===arguments[0]', [theme]), 'native theme')
        c.click('[aria-label="关闭面板"]')
        capture('pinned-' + theme)
        assert c.execute('return document.documentElement.scrollWidth<=innerWidth')
    print('PASS: native pin menu, pinned scope/search, full-content dedup, cancel/conflict/count, exact OS copy, preview unpin/focus and both themes', flush=True)


def verify_restart(c):
    page = history(c, True)
    assert page['total'] == 1 and page['items'][0]['id'] == c.pinned_id
    assert c.action('clip_detail', id=c.pinned_id, thumbnail=False)['text'] == c.pinned_text
    assert history(c)['total'] == 3
    assert not c.action('clipboard_status')['policy']['enabled']
