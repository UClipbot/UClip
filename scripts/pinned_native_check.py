"""Real WebKit pin and dedup checks, using only the isolated synthetic X11 session."""
from pathlib import Path
from clipboard_native_check import wait


def history(c, pinned=False, offset=0, revision=None):
    return c.action('history_query', request={'query': '', 'regex': False, 'kind': None,
                    'pinnedOnly': pinned, 'offset': offset, 'expectedRevision': revision})


def settled(c, count=None):
    return wait(lambda: c.execute('return document.querySelector(".history-scroll")?.getAttribute("aria-busy")==="false" && (!arguments[0] || document.querySelectorAll(".clip-row").length===arguments[1])', [count is not None, count]), 'effective native history page')


def select_value(c, selector, value):
    if c.execute('return document.querySelector(arguments[0]).matches(".filter-trigger")', [selector]):
        c.click(selector)
        c.click(f'.action-menu button[value="{value}"]')
        wait(lambda:c.execute('return document.querySelector(arguments[0]).value===arguments[1]',[selector,value]), 'filter value')
        return
    index=c.execute('return Array.from(document.querySelector(arguments[0]).options).findIndex(option=>option.value===arguments[1])',[selector,value])
    assert index >= 0, (selector,value)
    c.click(selector)
    c.key('key','--clearmodifiers','Home')
    for _ in range(index):c.key('key','--clearmodifiers','Down')
    c.key('key','--clearmodifiers','Return')
    wait(lambda:c.execute('return document.querySelector(arguments[0]).value===arguments[1]',[selector,value]),'native select value')


def scope(c, value):
    select_value(c,'[aria-label="历史范围"]',value)
    settled(c)
    assert c.execute('return document.querySelector("[aria-label=历史范围]").value===arguments[0]', [value])


def pin_ui(c, clip_id):
    wait(lambda: c.execute('return !!document.querySelector(`[data-clip-id="${arguments[0]}"]`)', [clip_id]), 'pin target visible')
    settled(c)
    c.execute('document.querySelector(".toast .icon-button")?.click();return true')
    c.click(f'[data-clip-id="{clip_id}"] [data-row-main]')
    c.click(f'[data-clip-id="{clip_id}"] .row-more')
    c.click('[data-testid="toggle-pin"]')
    wait(lambda: c.execute('return document.querySelector(".toast")?.textContent.includes("已置顶")'), 'pin commit feedback')
    wait(lambda: any(i['id'] == clip_id and i['pinned'] for i in history(c, True)['items']), 'pin persisted')
    settled(c)


def dedup_ui(c, removed):
    settled(c)
    c.execute('document.querySelector(".toast .icon-button")?.click();return true')
    c.click('[aria-label="更多选项"]')
    c.click('[aria-label="置顶页去重"]')
    c.click('.form-actions .primary-button')
    wait(lambda: not c.execute('return !!document.querySelector("[role=dialog]")'), 'dedup confirmation closes after commit')
    expected = f'已删除 {removed} 条' if removed else '没有需要删除'
    wait(lambda: c.execute('return document.querySelector(".toast")?.textContent.includes(arguments[0])', [expected]), 'fresh actual dedup count')


def run(c, capture):
    c.policy(enabled=False)
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
    c.click('[aria-label="更多选项"]')
    c.click('[aria-label="置顶页去重"]')
    assert c.execute('return document.querySelector("[role=dialog]").textContent.includes("当前搜索、类型筛选和页码不限制去重范围")')
    c.key('key', '--clearmodifiers', 'Escape')
    wait(lambda: not c.execute('return !!document.querySelector("[role=dialog]")'), 'cancel dedup with Escape')
    assert history(c)['total'] == 4

    c.click('[aria-label="更多选项"]')
    c.click('[aria-label="置顶页去重"]')
    c.action('create_text', text='concurrent history change')
    c.click('.form-actions .primary-button')
    wait(lambda: c.execute('return document.querySelector("[role=dialog] [role=alert]")?.textContent.includes("数据已发生变化")'), 'dedup rejects changed history')
    assert history(c)['total'] == 5
    c.click('[aria-label="关闭面板"]')
    settled(c)
    dedup_ui(c, 1)
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
    dedup_ui(c, 1)
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
    run_capture_chain(c)


def run_capture_chain(c):
    scope(c, 'all')
    c.policy(enabled=True)
    c.capture_pins = []
    duplicates = []
    for kind, fields, payload in [
        ('text', {'text': '系统置顶链路 🦀\n exact text  '}, '系统置顶链路 🦀\n exact text  '),
        ('image', c.image, c.image),
        ('files', {'files': c.files}, c.files),
    ]:
        before = {i['id'] for i in history(c)['items']}
        c.set(kind, **fields)
        def fresh():
            return next((i for i in history(c)['items'] if i['kind'] == kind and i['id'] not in before), None)
        first = wait(fresh, 'external ' + kind + ' capture')
        pin_ui(c, first['id'])
        c.capture_pins.append((first['id'], kind, payload))
        spacer = 'different generation before ' + kind
        c.set('text', text=spacer)
        wait(lambda: c.query(spacer)['total'] == 1, 'separate clipboard generation')
        before = {i['id'] for i in history(c)['items']}
        c.set(kind, **fields)
        duplicate = wait(fresh, 'same external ' + kind + ' gets independent unpinned identity')
        assert not duplicate['pinned'] and duplicate['id'] != first['id']
        duplicates.append(duplicate['id'])
    # Different payloads with the same dimensions/paths are not duplicates.
    different_image = dict(c.image, rgba=[x ^ 1 for x in c.image['rgba']])
    c.set('image', **different_image)
    wait(lambda: c.query(kind='image')['total'] == 3, 'different image pixels retained')
    c.set('files', files=list(reversed(c.files)))
    wait(lambda: c.query(kind='files')['total'] == 3, 'different file order retained')
    c.policy(enabled=False)
    before = history(c)['total']
    scope(c, 'pinned')
    c.enter('#history-search', '置顶合成样本')
    select_value(c, '[aria-label="内容类型"]', 'text')
    settled(c, 1)
    dedup_ui(c, 3)
    assert history(c)['total'] == before - 3
    assert history(c, True)['total'] == 4
    for clip_id in duplicates:
        result = c.invoke('clip_detail', {'id': clip_id, 'thumbnail': False})
        assert not result['ok'] and result['error']['code'] == 'not_found', result
    for clip_id, kind, payload in c.capture_pins:
        result = c.action('copy_clip', id=clip_id, paste=False)
        assert result['copied'] and result['usageSaved']
        c.received(kind, payload)
    assert all(Path(p).exists() for p in c.files)
    c.expected_active = history(c)['total']
    c.click('[aria-label="清空搜索"]')
    select_value(c, '[aria-label="内容类型"]', 'all')
    settled(c, 4)
    dedup_ui(c, 0)
    print('PASS: real external text/image/files capture → pin → recapture → full-history dedup under text/search filters; fresh feedback, distinct pixels/path order and exact native writeback', flush=True)


def verify_restart(c):
    page = history(c, True)
    assert page['total'] == 4 and any(i['id'] == c.pinned_id for i in page['items'])
    assert c.action('clip_detail', id=c.pinned_id, thumbnail=False)['text'] == c.pinned_text
    assert history(c)['total'] == c.expected_active
    assert not c.action('clipboard_status')['policy']['enabled']
    for clip_id, kind, payload in c.capture_pins:
        assert any(i['id'] == clip_id and i['pinned'] for i in page['items'])
        assert c.action('copy_clip', id=clip_id, paste=False)['copied']
        c.received(kind, payload)
