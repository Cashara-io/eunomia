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
    await expect(page.locator('[data-object], [aria-label="Selected object"], [aria-label="Recently viewed objects"], [aria-label="Suggested objects"], [aria-label="Object search results"]')).toHaveCount(0);
    await expect(page.getByRole('searchbox')).toHaveCount(0);
    expect(await protectedCache(page)).toEqual([]);
  });
}
for (const trigger of ['refresh', 'refresh-fit', 'relation', 'ontology', 'resync']) {
  test(`${trigger} updates links and search without moving selection or typing`, async ({page}) => {
    const data = neighbourhood();
    await mockConnections(page, 'populated', data);
    let send: (value: string) => void = () => {};
    await page.routeWebSocket('**/api/events/ws*', ws => {
      send = value => ws.send(value);
      send(JSON.stringify({type:'hello',vaults:{business:0}}));
    });
    await page.goto(url);
    await page.locator('[data-object="house"]').first().click();
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
    await expect(page.locator('[data-lane]')).toHaveCount(5);
    await expect(page.getByLabel('Object search results')).toContainText('New supplier');
    await expect(page.locator('[data-object="house"]').first()).toHaveAttribute('aria-pressed','true');
    data.edges.splice(1, 2);
    await refresh(2);
    await expect(page.locator('[data-lane]')).toHaveCount(3);
    await expect(page.locator('[data-object="house"]').first()).toHaveAttribute('aria-pressed','true');
    await expect(box).toHaveValue('North');
    await expect(box).toBeFocused();
  });
}

test('phone filters collapse and the inspector sheet follows the selection', async ({page}) => {
  await page.setViewportSize({width:390,height:844});
  await mockConnections(page);
  await page.goto(url+'&selected=house');
  const inspector = page.getByLabel('Selected object');
  await expect(inspector).toContainText('Values and their sources');
  await expect(inspector).toHaveAttribute('data-open', 'true');
  await expect(page.getByLabel('Depth',{exact:true})).toBeHidden();
  await page.getByRole('button',{name:'Filters',exact:true}).click();
  await expect(page.getByLabel('Depth',{exact:true})).toBeVisible();
  await page.getByLabel('Depth',{exact:true}).selectOption('2');
  await expect(page).toHaveURL(/depth=2/);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});

for (const theme of ['light','dark']) test(`stage text has AA contrast in ${theme}, whatever the kind colour`, async ({page}) => {
  await page.addInitScript(t => localStorage.setItem('eunomia-theme',t),theme);
  const data = richNeighbourhood();
  const kinds = ['person','organisation','location','repository','file','symbol','custom_kind'];
  for (let i=0;i<kinds.length;i++) {
    data.nodes.push({id:`kind${i}`,name:`Kind ${i}`,kind:kinds[i],facts:[]});
    data.edges.push({id:`kind-edge${i}`,from:'house',to:`kind${i}`,label:'related_to'});
  }
  await mockConnections(page,'populated',data);
  await page.goto(url);
  await expect(page.locator('[data-object]').first()).toBeVisible();
  const results = await page.evaluate(() => {
    const canvas = document.createElement('canvas'); canvas.width=canvas.height=1;
    const ctx = canvas.getContext('2d')!;
    const rgba = (css:string) => {ctx.clearRect(0,0,1,1);ctx.fillStyle=css;ctx.fillRect(0,0,1,1);return [...ctx.getImageData(0,0,1,1).data];};
    const lum = (c:number[]) => c.slice(0,3).map(v=>v/255).map(v=>v<=0.04045?v/12.92:((v+0.055)/1.055)**2.4).reduce((s,v,i)=>s+v*[0.2126,0.7152,0.0722][i],0);
    const background = (el: Element) => {
      for (let n: Element | null = el; n; n = n.parentElement) {
        const c = rgba(getComputedStyle(n).backgroundColor);
        if (c[3] === 255) return c;
      }
      return rgba(getComputedStyle(document.body).backgroundColor);
    };
    const elements = [...document.querySelectorAll('[data-object] span, [data-object] small, [data-lane] > span, nav[aria-label="Kinds"] button span, [data-wire^="def:"]')];
    return elements.filter(el => el.textContent?.trim()).map(el => {
      const a = lum(rgba(getComputedStyle(el).color)), b = lum(background(el));
      return {name: el.textContent, ratio: (Math.max(a,b)+0.05)/(Math.min(a,b)+0.05)};
    });
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
