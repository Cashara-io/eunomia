import { test, expect, type Page } from '@playwright/test';
import { mockConnections, neighbourhood, richNeighbourhood } from './fixtures/connections';
const url = '/ontology/connections?space=business&focus=house';

async function protectedCache(page: Page, retry = false) {
  return page.locator("main").evaluate((el, retry) => {
    // Inspect the actual provider cache without a production test API.
    const key = Object.keys(el).find(k => k.startsWith('__reactFiber$'))!;
    let fiber = Reflect.get(el, key);
    while (fiber) {
      const client = fiber.memoizedProps?.client;
      if (client?.getQueryCache) {
        if (retry) client.setQueryDefaults(['object-profile'], {retry:3,retryDelay:10000});
        return client.getQueryCache().getAll()
        .filter((q: {queryKey: unknown[]}) => q.queryKey.includes('business'))
        .map((q: {state: {data: unknown}}) => q.state.data).filter(Boolean);
      }
      fiber = fiber.return;
    }
    throw new Error('Query provider not found');
  }, retry);
}
for (const source of ['neighbourhood', 'search', 'inspector', 'inspector-retrying', 'suggestions', 'ontology']) {
  test(`${source} denial removes every protected view and cache`, async ({page}) => {
    await mockConnections(page, 'populated', neighbourhood());
    await page.routeWebSocket('**/api/events/ws*', ws => ws.send(JSON.stringify({type:'hello',vaults:{business:0}})));
    await page.goto(url);
    await page.locator('[data-object="house"]').first().click();
    await expect(page.getByLabel('Recently viewed objects')).toContainText('House blend');
    await expect(page.getByLabel('Selected object')).toContainText('Values and their sources');
    expect((await protectedCache(page, source === "inspector-retrying")).length).toBeGreaterThan(0);
    if (source.startsWith('inspector')) {
      await page.route('**/ontology/objects/*', r => r.fulfill({status:403,json:{code:'vault.forbidden',detail:'Denied'}}));
      await page.locator('[data-object="n0"]').click();
    } else if (source === 'ontology') {
      await page.route('**/ontology', r => r.fulfill({status:403,json:{code:'vault.forbidden',detail:'Denied'}}));
      await page.getByLabel('Refresh ontology').click();
    } else {
      await page.route(`**/api/tools/${source === 'neighbourhood' ? 'entities_neighbourhood' : 'entities_search'}`, r =>
        r.fulfill({status:200,json:{error:'Denied',code:'vault.forbidden'}}));
      if (source === 'search') await page.getByRole('searchbox').fill('North');
      else if (source === 'suggestions') await page.getByLabel('Refresh ontology').click();
      else await page.getByLabel('Depth', {exact:true}).selectOption('2');
    }
    await expect(page.getByRole('alert').filter({hasText:'unavailable to you'})).toBeVisible({timeout:12000});
    await expect(page.locator('[data-camera], [data-object], [aria-label="Selected object"], [aria-label="Recently viewed objects"], [aria-label="Suggested objects"], [aria-label="Object search results"]')).toHaveCount(0);
    await expect(page.getByRole('searchbox')).toHaveCount(0);
    expect(await protectedCache(page)).toEqual([]);
  });
}
for (const trigger of ['refresh', 'refresh-fit', 'relation', 'ontology', 'resync']) {
  test(`${trigger} updates links and search without moving camera, selection or typing`, async ({page}) => {
    const data = neighbourhood();
    await mockConnections(page, 'populated', data);
    let send: (value: string) => void = () => {};
    await page.routeWebSocket('**/api/events/ws*', ws => {
      send = value => ws.send(value);
      send(JSON.stringify({type:'hello',vaults:{business:0}}));
    });
    await page.goto(url);
    await page.locator('[data-object="house"]').first().click();
    if (trigger !== 'refresh-fit') await page.getByLabel('Pan right', {exact:true}).click();
    const camera = await page.locator('[data-camera]').getAttribute('data-camera');
    const topFace = page.locator('[data-form="house"] [class*="faceTop"]');
    const originalHeight = await topFace.getAttribute('d');
    const box = page.getByRole('searchbox');
    await box.fill('North');
    await expect(page.getByLabel('Object search results')).toContainText('North Coast Beans');
    if (trigger === 'refresh-fit') data.nodes.push({id:'new-node',name:'Added product',kind:'product',facts:[]});
    data.edges.push({id:'new',from:'house',to:trigger === 'refresh-fit'?'new-node':'n0',label:'ordered_from'});
    await page.route('**/api/tools/entities_search', r => r.fulfill({json:{results:[{...data.nodes[1],name:'New supplier'}]}}));
    async function refresh(seq: number) {
    if (trigger.startsWith('refresh')) {
      await box.evaluate(el => {
        document.querySelector<HTMLButtonElement>('[aria-label="Refresh ontology"]')?.click();
        el.focus();
      });
    } else send(JSON.stringify(trigger === 'relation' ? {type:'change',vault:'business',seq,kind:'relation'} : trigger === 'ontology' ? {type:'ontology',vault:'business',event:{seq}} : {type:'resync'}));
    }
    await refresh(1);
    await expect(page.getByRole('status').filter({hasText:'objects'})).toContainText('5 links');
    await expect(page.getByLabel('Object search results')).toContainText('New supplier');
    await expect(page.locator('[data-camera]')).toHaveAttribute('data-camera', camera!);
    await expect(page.locator('[data-object="house"]').first()).toHaveAttribute('aria-pressed','true');
    await expect(topFace).not.toHaveAttribute('d', originalHeight!);
    const addedHeight = await topFace.getAttribute('d');
    data.edges.splice(1, 2);
    await refresh(2);
    await expect(page.getByRole('status').filter({hasText:'objects'})).toContainText('3 links');
    await expect(topFace).not.toHaveAttribute('d', addedHeight!);
    await expect(page.locator('[data-camera]')).toHaveAttribute('data-camera', camera!);
    await expect(page.locator('[data-object="house"]').first()).toHaveAttribute('aria-pressed','true');
    await expect(box).toHaveValue('North');
    await expect(box).toBeFocused();
  });
}

test('Fit text is legible, zoom reduces detail, routes fan out and labels do not stack', async ({page}) => {
  await mockConnections(page);
  await page.goto(url);
  await expect(page.locator('[data-edge-path]')).toHaveCount(13);
  await expect(page.locator('[data-edge-label]:visible')).toHaveCount(13);
  const sizes = await page.locator('[data-camera]').evaluate(el => {
    const zoom = Number(el.getAttribute('data-camera')?.split(',')[2]);
    return [...el.querySelectorAll('[class*="markKind"], [data-edge-label] text')].map(n => {
      const matrix = n instanceof SVGTextElement ? n.getScreenCTM() : null;
      const scale = matrix ? Math.hypot(matrix.a, matrix.b) : zoom;
      return parseFloat(getComputedStyle(n).fontSize) * scale;
    });
  });
  expect(Math.min(...sizes)).toBeGreaterThanOrEqual(10.99);
  const starts = await page.locator('[data-edge-path][data-from="house"]').evaluateAll(paths => paths.map(p => p.getAttribute('data-start')));
  expect(new Set(starts).size).toBe(starts.length);
  await expect(page.locator('marker path')).toHaveAttribute('fill', 'context-stroke');
  for (let i=0;i<5;i++) await page.getByLabel('Zoom out',{exact:true}).click();
  await expect(page.locator('[class*="markKind"]').first()).toBeHidden();
  await expect(page.locator('[data-edge-label]:visible')).toHaveCount(0);
  await page.locator('[data-object="house"]').first().focus();
  await expect.poll(() => page.locator('[data-edge-label]:visible').count()).toBeGreaterThan(0);
  for (let i=0;i<9;i++) await page.getByLabel('Zoom in',{exact:true}).click();
  await expect(page.locator('[class*="markKind"]').first()).toBeVisible();
  await expect(page.locator('[data-edge-label]:visible')).toHaveCount(await page.locator('[data-edge-path]').count());
});

test('phone filters collapse, suggestions scroll and inspector begins in the first screen', async ({page}) => {
  await page.setViewportSize({width:390,height:844});
  await mockConnections(page);
  await page.goto(url+'&selected=house');
  const inspector = page.getByLabel('Selected object');
  await expect(inspector).toContainText('Values and their sources');
  expect((await inspector.boundingBox())!.y).toBeLessThan(650);
  await expect(page.getByLabel('Depth',{exact:true})).toBeHidden();
  await page.getByRole('button',{name:'Filters',exact:true}).click();
  await expect(page.getByLabel('Depth',{exact:true})).toBeVisible();
  await page.getByLabel('Depth',{exact:true}).selectOption('2');
  await expect(page.getByRole('status').filter({hasText:'objects'})).toContainText('73 objects');
  const row = await page.getByLabel('Suggested objects').evaluate(el => ({height:el.clientHeight, width:el.clientWidth,scroll:el.scrollWidth}));
  expect(row.height).toBeLessThan(48);
  expect(row.scroll).toBeGreaterThan(row.width);
});

for (const theme of ['light','dark']) test(`all hue text has AA contrast in ${theme}`, async ({page}) => {
  await page.addInitScript(t => localStorage.setItem('eunomia-theme',t),theme);
  const data = richNeighbourhood();
  const kinds = ['person','organisation','location','repository','file','symbol','custom_kind'];
  for (let i=0;i<kinds.length;i++) {
    data.nodes.push({id:`kind${i}`,name:`Kind ${i}`,kind:kinds[i],facts:[]});
    data.edges.push({id:`kind-edge${i}`,from:'house',to:`kind${i}`,label:'related_to'});
  }
  await mockConnections(page,'populated',data);
  await page.goto(url);
  await expect(page.locator('[data-edge-path]')).toHaveCount(20);
  const results = await page.evaluate(() => {
    const canvas = document.createElement('canvas'); canvas.width=canvas.height=1;
    const ctx = canvas.getContext('2d')!;
    const rgb = (css:string) => {ctx.clearRect(0,0,1,1);ctx.fillStyle=css;ctx.fillRect(0,0,1,1);return [...ctx.getImageData(0,0,1,1).data].slice(0,3);};
    const lum = (c:number[]) => c.map(v=>v/255).map(v=>v<=0.04045?v/12.92:((v+0.055)/1.055)**2.4).reduce((s,v,i)=>s+v*[0.2126,0.7152,0.0722][i],0);
    const elements = [...document.querySelectorAll('[class*="markKind"], [data-edge-label] text, [class*="kindBars"] button > span:first-child')];
    const check = () => elements.map(el=>{
      const style=getComputedStyle(el); const color=el.tagName==='text'?style.fill:style.color;
      const background=el.closest('button')??el.closest('[data-edge-label]')?.querySelector('rect');
      const bg=background?getComputedStyle(background)[background.tagName==='rect'?'fill':'backgroundColor']:'';
      const a=lum(rgb(color)), b=lum(rgb(bg && bg!=='rgba(0, 0, 0, 0)'?bg:getComputedStyle(document.documentElement).getPropertyValue('--surface')));
      return {name:el.textContent,ratio:(Math.max(a,b)+0.05)/(Math.min(a,b)+0.05)};
    });
    const results = check();
    // Include arbitrary owner colours, not only the built-in palette.
    for (const hue of [...Array.from({length:24}, (_,i)=>`hsl(${i*15} 100% 50%)`), '#ffffff', '#000000']) {
      for (const el of elements) {
        const mark = el.closest<HTMLElement>('[data-focus], [class*="kindBars"] button');
        mark?.style.setProperty('--kind-hue', hue);
      }
      results.push(...check());
    }
    return results;
  });
  expect(results.length).toBeGreaterThan(30);
  expect(results.filter(r=>r.ratio<4.5)).toEqual([]);
});

test('live neighbourhood invalidation is scoped and batches related events once', async ({page}) => {
  const requests = await mockConnections(page, 'populated', neighbourhood());
  let send: (value: object) => void = () => {};
  await page.routeWebSocket('**/api/events/ws*', ws => {
    send = value => ws.send(JSON.stringify(value));
    send({type:'hello',vaults:{business:0,other:0}});
  });
  await page.goto(url);
  await expect(page.locator('[data-object="house"]').first()).toBeVisible();
  const count = () => requests.filter(r => 'entity_id' in r).length;
  const before = count();
  for (const kind of ['entity', 'relation']) send({type:'change',vault:'other',seq:1,kind});
  send({type:'ontology',vault:'other',event:{seq:1}});
  for (const kind of ['memory', 'document']) send({type:'change',vault:'business',seq:2,kind});
  send({type:'heartbeat'});
  // Observe longer than the real 400 ms batching window, including async fetches.
  await page.waitForTimeout(650);
  expect(count()).toBe(before);
  await page.clock.install();
  await page.clock.pauseAt(new Date());
  for (const kind of ['entity', 'relation']) send({type:'change',vault:'business',seq:3,kind});
  send({type:'ontology',vault:'business',event:{seq:3}});
  await page.clock.runFor(399);
  expect(count()).toBe(before);
  await page.clock.runFor(251);
  await expect.poll(count).toBe(before + 1);
  await page.clock.runFor(650);
  expect(count()).toBe(before + 1);
});
