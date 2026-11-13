"""Collection use cases through real WebKit UI/IPC on a disposable authenticated X11 session."""
import sqlite3
from pathlib import Path
from clipboard_native_check import wait
from pinned_native_check import settled, scope, select_value


def page(c, collection_id=None, offset=0):
    return c.action('history_query', request={'query':'','regex':False,'kind':None,'pinnedOnly':False,'collectionId':collection_id,'offset':offset,'expectedRevision':None})


def app_menu(c, action):
    c.click('[aria-label="更多选项"]')
    c.click(f'[data-testid="{action}"]')


def paste(c, selector, text):
    # WebKitWebDriver send-keys drops non-BMP characters; use actual native paste and verify the draft.
    c.set('text',text=text);c.click(selector)
    c.key('key','--clearmodifiers','ctrl+a');c.key('key','--clearmodifiers','ctrl+v')
    wait(lambda:c.execute('return document.querySelector(arguments[0]).value===arguments[1]',[selector,text]),'exact native input including Unicode and whitespace')


def category_ui(c, name):
    app_menu(c, 'new-collection')
    paste(c, '#collection-name', name)
    c.click('.new-form .primary-button')
    wait(lambda:not c.execute('return !!document.querySelector("#collection-name")'), 'collection form committed')
    value=next(i for i in c.action('collections_list')['items'] if i['name']==name.strip())
    wait(lambda:c.execute('return document.querySelector("[aria-label=历史范围]").value===arguments[0]',['collection:'+value['id']]), 'new collection becomes query scope')
    settled(c,0)
    return value


def transfer_form(c, clip_id, mode):
    expected=next(i['revision'] for i in page(c)['items'] if i['id']==clip_id)
    wait(lambda:c.execute('return document.querySelector(`[data-clip-id="${arguments[0]}"]`)?.dataset.clipRevision===arguments[1]',[clip_id,expected]),'current transfer source revision reaches native list')
    settled(c)
    c.click(f'[data-clip-id="{clip_id}"] [data-row-main]')
    c.click(f'[data-clip-id="{clip_id}"] .row-more')
    c.click('[data-testid="copy-to-collection"]' if mode=='copy' else '[data-testid="move-to-collection"]')


def transfer_ui(c, clip_id, mode, destination):
    before={i['id'] for i in page(c,destination)['items']} if destination else set()
    transfer_form(c,clip_id,mode)
    select_value(c,'#transfer-collection',destination or '')
    c.click('.new-form .primary-button')
    wait(lambda:not c.execute('return !!document.querySelector("#transfer-collection")'), 'transfer commits and closes form')
    settled(c)
    if mode=='move':
        assert any(i['id']==clip_id for i in page(c,destination)['items'])
        return clip_id
    return next(i['id'] for i in page(c,destination)['items'] if i['id'] not in before)


def run(c,capture):
    assert not c.action('clipboard_status')['policy']['enabled']
    c.activate(c.window('^UClip$'))
    a=category_ui(c,'工作 🦀');b=category_ui(c,'素材')
    c.category_a,c.category_b=a['id'],b['id']
    app_menu(c,'new-collection');paste(c,'#collection-name',a['name'])
    before=page(c)['revision'];c.click('.new-form .primary-button')
    wait(lambda:c.execute('return document.querySelector("[role=alert]")?.textContent.includes("分类名称已存在")'),'duplicate name leaves form visible')
    assert page(c)['revision']==before
    c.click('[aria-label="关闭面板"]')

    app_menu(c,'rename-collection');paste(c,'#collection-name','我的素材')
    c.action('rename_collection',request={'id':b['id'],'name':'外部重命名','expectedRevision':'0'})
    c.click('.new-form .primary-button')
    wait(lambda:c.execute('return document.querySelector("[role=alert]")?.textContent.includes("数据已发生变化")'),'stale rename rejected')
    c.click('[aria-label="关闭面板"]')
    wait(lambda:c.execute('return document.querySelector("[aria-label=历史范围]").selectedOptions[0].textContent==="外部重命名"'),'external rename refreshes label without changing scope')
    app_menu(c,'rename-collection');paste(c,'#collection-name','我的素材');c.click('.new-form .primary-button')
    wait(lambda:not c.execute('return !!document.querySelector("#collection-name")'),'rename commits')
    renamed=c.action('collections_list')['items'][1]
    assert renamed['id']==b['id'] and renamed['name']=='我的素材', renamed

    scope(c,'collection:'+a['id'])
    app_menu(c,'new-text');paste(c,'#clip-content','分类内新建 🦀\n  保留空白  ');c.click('.new-form .primary-button')
    wait(lambda:not c.execute('return !!document.querySelector("#clip-content")'),'manual text in current collection')
    source=page(c,a['id'])['items'][0]
    assert source['collectionId']==a['id']
    settled(c,1)
    transfer_ui(c,source['id'],'move',b['id'])
    assert page(c,a['id'])['total']==0
    assert page(c,b['id'])['items'][0]['capturedAt']==source['capturedAt']
    copied=transfer_ui(c,source['id'],'copy',a['id'])
    assert copied!=source['id'] and page(c,b['id'])['total']==1
    assert c.action('clip_detail',id=copied,thumbnail=False)==dict(c.action('clip_detail',id=source['id'],thumbnail=False),id=copied)
    c.manual_source=source['id'];c.manual_copy=copied

    transfer_form(c,copied,'move')
    c.action('copy_clip',id=copied,paste=False)
    c.click('.new-form .primary-button')
    wait(lambda:c.execute('return document.querySelector("[role=alert]")?.textContent.includes("数据已发生变化")'),'stale move keeps identity in old category')
    assert page(c,a['id'])['total']==1 and page(c,b['id'])['total']==1
    c.click('[aria-label="关闭面板"]');settled(c)
    transfer_ui(c,copied,'move',None)
    assert page(c,a['id'])['total']==0

    # Persistence failure must leave the user's draft and transaction revision untouched.
    database=next(Path(c.directory,'data').rglob('foundation.sqlite3'))
    with sqlite3.connect(database) as db:
        db.execute("CREATE TRIGGER fixture_category_failure BEFORE INSERT ON collection_changes BEGIN SELECT RAISE(ABORT,'fixture'); END")
    try:
        app_menu(c,'new-collection');c.enter('#collection-name','保留失败草稿')
        before=page(c)['revision'];c.click('.new-form .primary-button')
        wait(lambda:c.execute('return document.querySelector("[role=alert]")?.textContent.includes("本地保存失败")'),'safe category storage failure')
        assert c.execute('return document.querySelector("#collection-name").value')=='保留失败草稿'
        assert page(c)['revision']==before
        c.click('[aria-label="关闭面板"]')
    finally:
        with sqlite3.connect(database) as db:db.execute('DROP TRIGGER fixture_category_failure')
    print('PASS: native create/duplicate/rename/conflict, category-local text, move identity/copy identity, stale transfer and failed-save draft retention',flush=True)

    c.type_copies=[]
    for kind,fields,payload in [('text',{'text':'外部分类文本'},'外部分类文本'),('image',c.image,c.image),('files',{'files':c.files},c.files)]:
        c.policy(enabled=True)
        before={i['id'] for i in page(c)['items']};c.set(kind,**fields)
        source=wait(lambda:next((i for i in page(c)['items'] if i['id'] not in before and i['kind']==kind),None),'external '+kind+' source')
        c.policy(enabled=False);scope(c,'all');settled(c)
        transfer_ui(c,source['id'],'move',b['id'])
        new_id=transfer_ui(c,source['id'],'copy',a['id'])
        assert new_id!=source['id']
        assert c.action('copy_clip',id=new_id,paste=False)['copied'];c.received(kind,payload)
        c.type_copies.append((new_id,kind,payload))
    assert all(Path(path).exists() for path in c.files)

    # Only protected records remain: a failed extra copy cannot evict a source or another record.
    c.policy(maxItems=page(c)['total']);settled(c)
    target=c.type_copies[-1][0];transfer_form(c,target,'copy');c.click('.new-form .primary-button')
    wait(lambda:c.execute('return document.querySelector("[role=alert]")?.textContent.includes("容量超出限制")'),'native copy capacity refusal')
    assert c.execute('return !!document.querySelector("#transfer-collection")')
    c.click('[aria-label="关闭面板"]');c.policy(maxItems=500)
    print('PASS: actual text/image/files move and independent copy, exact OS writeback, source files preserved and capacity failure retains form/source',flush=True)

    for n in range(27):c.action('create_text',text=f'分页分类 fixture {n:02}',collectionId=b['id'])
    scope(c,'collection:'+b['id']);settled(c,25)
    c.click('.pagination button:last-child');settled(c,6)
    scope(c,'collection:'+a['id']);settled(c,3)
    assert not c.execute('return !!document.querySelector(".pagination")')
    c.enter('#history-search','分页分类 fixture 00');settled(c,0)
    scope(c,'all');settled(c,1)
    c.set('text',text='public wrong-selection sentinel');c.click('#history-search');c.key('key','--clearmodifiers','Return')
    wait(lambda:c.source.send({'op':'read','kind':'text'})['text']=='分页分类 fixture 00','global search Enter copies current valid result')
    c.click('[aria-label="清空搜索"]');scope(c,'collection:'+a['id']);settled(c,3)
    c.key('windowsize',c.window('^UClip$'),'480','650')
    for theme in ['light','dark']:
        c.click('[data-testid="open-settings"]');c.click(f'#theme option[value="{theme}"]')
        wait(lambda:c.execute('return document.documentElement.dataset.theme===arguments[0]',[theme]),'collection theme')
        c.click('[aria-label="关闭面板"]');capture('collections-'+theme)
        assert c.execute('return document.documentElement.scrollWidth<=innerWidth')
    c.expected_total=page(c)['total']
    print('PASS: real category pagination, scope change resets page/selection, category-to-global search, correct Enter action and both themes at 480px',flush=True)


def verify_restart(c):
    categories=c.action('collections_list')['items']
    assert [(i['id'],i['name']) for i in categories]==[(c.category_a,'工作 🦀'),(c.category_b,'我的素材')], categories
    assert page(c)['total']==c.expected_total
    assert any(i['id']==c.manual_source and i['collectionId']==c.category_b for i in page(c,c.category_b,25)['items'])
    assert c.action('clip_detail',id=c.manual_source,thumbnail=False)['text']=='分类内新建 🦀\n  保留空白  '
    assert page(c,c.category_b,25)['total']==31
    for clip_id,kind,payload in c.type_copies:
        assert any(i['id']==clip_id and i['collectionId']==c.category_a for i in page(c,c.category_a)['items'])
        assert c.action('copy_clip',id=clip_id,paste=False)['copied'];c.received(kind,payload)
    assert not c.action('clipboard_status')['policy']['enabled']
