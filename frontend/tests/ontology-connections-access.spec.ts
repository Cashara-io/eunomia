import { test, expect, type Page } from '@playwright/test';
import { mockConnections, neighbourhood } from './fixtures/connections';
import { protectedValues } from './fixtures/protected-values';

const url = '/ontology/connections?space=business&focus=house';

async function cacheKeys(page: Page, action: 'read' | 'vaults' | 'switch' = 'read'): Promise<unknown[][]> {
  return page.locator('main').evaluate((el, action) => {
    const key = Object.keys(el).find(k => k.startsWith('__reactFiber$'))!;
    let fiber = Reflect.get(el, key);
    while (fiber) {
      const client = fiber.memoizedProps?.client;
      if (client?.getQueryCache) {
        Reflect.set(window, '__connectionTestCache', client);
        if (action === 'vaults') void client.invalidateQueries({queryKey: ['vaults', 'list']});
        if (action === 'switch') {
          const next = new URL(location.href);
          next.search = '?space=personal';
          history.pushState(null, '', next);
        }
        return client.getQueryCache().getAll().map((q: {queryKey: unknown[]}) => q.queryKey);
      }
      fiber = fiber.return;
    }
    throw new Error('Query provider not found');
  }, action);
}

async function setup(page: Page, selected = 'n0') {
  await mockConnections(page, 'populated', neighbourhood());
  await page.routeWebSocket('**/api/events/ws*', ws => ws.send(JSON.stringify({type:'hello',vaults:{business:0}})));
  await page.route('**/api/vaults/business/members', r => r.fulfill({json:{results:[]}}));
  await page.goto(url);
  await page.locator('[data-object="n0"]').first().click();
  await page.locator('[data-object="house"]').first().click();
  if (selected !== 'house') await page.locator(`[data-object="${selected}"]`).first().click();
  await expect(page.getByLabel('Selected object')).toContainText('Values and their sources');
}

for (const target of ['selected', 'focus']) test(`deleted ${target} clears only the missing object after Refresh`, async ({page}) => {
  await setup(page, target === 'focus' ? 'house' : 'n0');
  await page.getByRole('searchbox').fill('North');
  await expect(page.getByLabel('Object search results')).toBeVisible();
  await page.route(`**/ontology/objects/${target === 'focus' ? 'house' : 'n0'}*`, r => r.fulfill({status:404,json:{code:'resource.not_found',detail:'object unavailable'}}));
  if (target === 'focus') await page.route('**/api/tools/entities_neighbourhood', r => r.fulfill({json:{error:'not found',code:'entity.not_found'}}));
  await page.getByLabel('Refresh ontology').click();
  await expect(page.getByRole('status').filter({hasText:'This object no longer exists'})).toBeVisible();
  await expect(page.getByLabel('Selected object')).toHaveCount(0);
  await expect(page.getByRole('searchbox')).toHaveValue('North');
  await expect(page.getByLabel('Object search results')).toBeVisible();
  await expect(page.getByRole('navigation', {name:'Ontology views'})).toBeVisible();
  await expect(page).not.toHaveURL(/selected=/);
  if (target === 'focus') {
    await expect(page.getByRole('heading', {name:'Start with something you know.'})).toBeVisible();
    await expect(page).not.toHaveURL(/focus=/);
  }
  await page.getByRole('searchbox').fill('');
  if (target === 'focus') await expect(page.getByLabel('Suggested objects')).toBeVisible();
  await expect(page.getByLabel('Recently viewed objects')).toContainText(target === 'focus' ? 'North' : 'House');
  await expect(page.getByLabel('Recently viewed objects')).not.toContainText(target === 'focus' ? 'House' : 'North');
});

for (const source of ['profile', 'masked-neighbourhood', 'space-404']) test(`real membership loss from ${source} clears everything and blocks late reads`, async ({page}) => {
  await setup(page);
  await page.getByRole('searchbox').fill('North');
  await expect(page.getByLabel('Object search results')).toBeVisible();
  let release: () => void = () => {};
  const held = new Promise<void>(resolve => {release = resolve;});
  let requested: () => void = () => {};
  const started = new Promise<void>(resolve => {requested = resolve;});
  let delivered: () => void = () => {};
  const completed = new Promise<void>(resolve => {delivered = resolve;});
  await page.route('**/api/tools/entities_search', async r => {
    requested();
    await held;
    await r.fulfill({json:{results:neighbourhood().nodes}});
    delivered();
  });
  await page.getByRole('searchbox').fill('Late');
  await started;
  await page.route('**/api/vaults/business/members', r => r.fulfill({status:403,json:{code:'vault.forbidden'}}));
  if (source === 'profile') await page.route('**/ontology/objects/*', r => r.fulfill({status:403,json:{code:'vault.forbidden'}}));
  else if (source === 'space-404') await page.route('**/ontology', r => r.fulfill({status:404,json:{code:'vault.not_found'}}));
  else await page.route('**/api/tools/entities_neighbourhood', r => r.fulfill({json:{error:'not found',code:'entity.not_found'}}));
  await page.getByLabel('Refresh ontology').click();
  await expect(page.getByRole('alert').filter({hasText:'unavailable to you'})).toBeVisible();
  release();
  await completed;
  await expect(page.locator('[data-object], [aria-label="Selected object"]')).toHaveCount(0);
  await expect(page.getByRole('searchbox')).toHaveCount(0);
  await expect(page).not.toHaveURL(/[?&](q|focus|selected)=/);
  await expect.poll(async () => (await cacheKeys(page)).filter(k => k.includes('business'))).toEqual([]);
});

test('re-adding membership recovers the mounted view', async ({page}) => {
  await setup(page);
  await page.route('**/ontology/objects/*', r => r.fulfill({status:403,json:{code:'vault.forbidden'}}));
  await page.getByLabel('Refresh ontology').click();
  await expect(page.getByRole('alert').filter({hasText:'unavailable to you'})).toBeVisible();
  await page.unroute('**/ontology/objects/*');
  await cacheKeys(page, 'vaults');
  await expect(page.getByRole('searchbox')).toHaveValue('');
  await expect(page.getByRole('heading', {name:'Start with something you know.'})).toBeVisible();
  await expect(page.getByLabel('Suggested objects')).toBeVisible();
});

test('switching spaces removes previous connection and search caches', async ({page}) => {
  await setup(page);
  await page.getByRole('searchbox').fill('North');
  await expect(page.getByLabel('Object search results')).toBeVisible();
  expect((await cacheKeys(page)).some(k => k[0] === 'connection-search' && k[1] === 'business')).toBe(true);
  await cacheKeys(page, 'switch');
  await expect(page).toHaveURL(/space=personal/);
  await expect.poll(async () => (await cacheKeys(page)).filter(k => ['connections','connection-search'].includes(String(k[0])) && k[1] === 'business')).toEqual([]);
});

test('500 and network failures keep input, selection and recents', async ({page}) => {
  await setup(page);
  await page.getByRole('searchbox').fill('North');
  await expect(page.getByLabel('Object search results')).toBeVisible();
  for (const failure of ['500', 'network']) {
    await page.route('**/api/tools/entities_neighbourhood', r => failure === '500' ? r.fulfill({status:500,json:{code:'internal'}}) : r.abort('failed'));
    await page.route('**/ontology/objects/*', r => failure === '500' ? r.fulfill({status:500,json:{code:'internal'}}) : r.abort('failed'));
    await page.getByLabel('Refresh ontology').click();
    await expect(page.getByLabel('Selected object')).toContainText('Could not load this object');
    await expect(page.getByRole('searchbox')).toHaveValue('North');
    await expect(page).toHaveURL(/selected=n0/);
  }
  await page.getByRole('searchbox').fill('');
  await expect(page.getByLabel('Recently viewed objects')).toContainText('North Coast Beans');
});


test('Retry space access recovers after re-adding without a vault list update', async ({page}) => {
  await setup(page);
  await page.route('**/ontology/objects/*', r => r.fulfill({status:403,json:{code:'vault.forbidden'}}));
  await page.route('**/api/vaults/business/members', r => r.fulfill({status:403,json:{code:'vault.forbidden'}}));
  await page.getByLabel('Refresh ontology').click();
  const retry = page.getByRole('button', {name:'Retry space access'});
  await expect(retry).toBeVisible();
  await retry.click();
  await expect(page.getByRole('searchbox')).toHaveCount(0);
  await page.unroute('**/ontology/objects/*');
  await page.unroute('**/api/vaults/business/members');
  await page.route('**/api/vaults/business/members', r => r.fulfill({json:{results:[]}}));
  await retry.click();
  await expect(page.getByRole('searchbox')).toHaveValue('');
  await expect(page.getByLabel('Suggested objects')).toBeVisible();
});

test('a vault list that loses the space clears its caches and can recover', async ({page}) => {
  await setup(page);
  await page.route('**/api/vaults', r => r.fulfill({json:{results:[]}}));
  await cacheKeys(page, 'vaults');
  await expect(page.getByRole('heading', {name:'Choose a memory space'})).toBeVisible();
  await expect(page).not.toHaveURL(/[?&](q|focus|selected)=/);
  await expect.poll(async () => (await cacheKeys(page)).filter(k => k.includes('business'))).toEqual([]);
  await page.unroute('**/api/vaults');
  await cacheKeys(page, 'vaults');
  await expect(page.getByRole('searchbox')).toHaveValue('');
  await expect(page.getByRole('heading', {name:'Start with something you know.'})).toBeVisible();
});

test('logout removes connection and search caches', async ({page}) => {
  await setup(page);
  await cacheKeys(page);
  await page.route('**/api/auth/logout', r => r.fulfill({json:{ok:true}}));
  await page.getByRole('button', {name:'Log out', exact:true}).click();
  await expect.poll(() => page.evaluate(() => Reflect.get(window, '__connectionTestCache')
    .getQueryCache().getAll().filter((q: {queryKey: unknown[]}) => q.queryKey.includes('business')).length)).toBe(0);
});


test('leaving through the space chooser clears previous search caches', async ({page}) => {
  await setup(page);
  await page.getByRole('searchbox').fill('North');
  await expect(page.getByLabel('Object search results')).toBeVisible();
  await page.getByRole('link', {name:'Change space, current space Banksia Coffee'}).click();
  await expect(page.getByRole('heading', {name:'Choose a memory space'})).toBeVisible();
  await expect.poll(async () => (await cacheKeys(page)).filter(k => ['connections','connection-search'].includes(String(k[0])) && k[1] === 'business')).toEqual([]);
});

async function expectNoProtectedContent(page: Page) {
  for (const name of ['House blend 1kg', 'North Coast Beans', 'PO-1042', 'Fitzroy store', 'Decaf 1kg', 'Banksia Coffee']) {
    await expect(page.locator('main')).not.toContainText(name);
  }
  await expect(page.locator('[data-object], [aria-label="Selected object"], [aria-label="Recently viewed objects"], [aria-label="Suggested objects"], [aria-label="Object search results"], [aria-label="Connections in words"]')).toHaveCount(0);
}

test('masked neighbourhood loss removes protected text during a delayed membership check', async ({page}) => {
  await setup(page);
  await page.getByRole('searchbox').fill('North');
  await expect(page.getByLabel('Object search results')).toContainText('North Coast Beans');
  let release = () => {};
  const held = new Promise<void>(resolve => {release = resolve;});
  let requested = () => {};
  const started = new Promise<void>(resolve => {requested = resolve;});
  await page.route('**/api/vaults/business/members', async r => {
    requested();
    await held;
    await r.fulfill({status:403,json:{code:'vault.forbidden'}});
  });
  await page.route('**/api/tools/entities_neighbourhood', r => r.fulfill({json:{error:'not found',code:'entity.not_found'}}));
  await page.getByLabel('Refresh ontology').click();
  await started;
  await expectNoProtectedContent(page);
  await expect(page.getByRole('status').filter({hasText:'Checking access'})).toBeVisible();
  release();
  await expect(page.getByRole('alert').filter({hasText:'unavailable to you'})).toBeVisible();
  await expectNoProtectedContent(page);
  await expect.poll(async () => (await cacheKeys(page)).filter(k => k.includes('business'))).toEqual([]);
});

test('a membership check that never answers times out, stays closed and offers a retry', async ({page}) => {
  await setup(page);
  await page.getByRole('searchbox').fill('North');
  await expect(page.getByLabel('Object search results')).toContainText('North Coast Beans');
  await page.clock.install();
  let requested = () => {};
  const started = new Promise<void>(resolve => {requested = resolve;});
  // Never fulfilled: the request hangs until the page aborts it.
  await page.route('**/api/vaults/business/members', () => { requested(); });
  await page.route('**/api/tools/entities_neighbourhood', r => r.fulfill({json:{error:'not found',code:'entity.not_found'}}));
  await page.getByLabel('Refresh ontology').click();
  await started;
  await expect(page.getByRole('status').filter({hasText:'Checking access'})).toBeVisible();
  await expectNoProtectedContent(page);
  await page.clock.fastForward(11_000);
  await expect(page.getByRole('button', {name:'Try again', exact:true})).toBeVisible();
  await expect(page.getByRole('status').filter({hasText:'Checking access'})).toHaveCount(0);
  await expectNoProtectedContent(page);
});

for (const failure of ['network', '500']) for (const outcome of ['confirmed', 'denied']) {
  test(`masked neighbourhood with ${failure} membership failure stays closed, retry ${outcome}`, async ({page}) => {
    await setup(page);
    await page.getByLabel('Depth', {exact:true}).selectOption('2');
    await page.getByLabel('Relation', {exact:true}).selectOption('supplies');
    await page.getByLabel('Direction', {exact:true}).selectOption('in');
    await page.getByRole('searchbox').fill('North');
    await expect(page.getByLabel('Object search results')).toContainText('North Coast Beans');
    await page.clock.install();
    let attempts = 0;
    let recovered = false;
    await page.route('**/api/vaults/business/members', r => {
      attempts++;
      if (!recovered) return failure === 'network' ? r.abort('failed') : r.fulfill({status:500,json:{code:'internal'}});
      return outcome === 'confirmed' ? r.fulfill({json:{results:[]}}) : r.fulfill({status:403,json:{code:'vault.forbidden'}});
    });
    await page.route('**/api/tools/entities_neighbourhood', r => r.fulfill({json:{error:'not found',code:'entity.not_found'}}));
    await page.getByLabel('Refresh ontology').click();
    const retry = page.getByRole('button', {name:'Try again',exact:true});
    await expect(retry).toBeVisible();
    await expect(page.getByRole('alert').filter({hasText:"Couldn't confirm access"})).toBeVisible();
    await expectNoProtectedContent(page);
    // One automatic retry, then a longer backoff. Failed checks never reveal cached text.
    await page.clock.fastForward(2000);
    await expect.poll(() => attempts).toBe(2);
    await expect(retry).toBeVisible();
    await expectNoProtectedContent(page);
    recovered = true;
    if (outcome === 'confirmed') await retry.click();
    else await page.clock.fastForward(4000);
    if (outcome === 'confirmed') {
      await expect(page.getByRole('status').filter({hasText:'This object no longer exists'})).toBeVisible();
      await expect(page.getByRole('searchbox')).toHaveValue('North');
      await expect(page).toHaveURL(/depth=2/);
      await expect(page).toHaveURL(/relation=supplies/);
      await expect(page).toHaveURL(/direction=in/);
      await expect(page).not.toHaveURL(/focus=/);
      await expect(page.getByLabel('Object search results')).toContainText('North Coast Beans');
    } else {
      await expect(page.getByRole('alert').filter({hasText:'unavailable to you'})).toBeVisible();
      await expectNoProtectedContent(page);
      await expect.poll(async () => (await cacheKeys(page)).filter(k => k.includes('business'))).toEqual([]);
    }
  });
}

test('delayed confirmation of a deleted selection preserves input', async ({page}) => {
  await setup(page);
  await page.getByLabel('Depth', {exact:true}).selectOption('2');
  await page.getByLabel('Relation', {exact:true}).selectOption('supplies');
  await page.getByLabel('Direction', {exact:true}).selectOption('in');
  await page.getByRole('searchbox').fill('North');
  await expect(page.getByLabel('Object search results')).toContainText('North Coast Beans');
  let release = () => {};
  const held = new Promise<void>(resolve => {release = resolve;});
  await page.route('**/api/vaults/business/members', async r => {
    await held;
    await r.fulfill({json:{results:[]}});
  });
  await page.route('**/ontology/objects/n0*', r => r.fulfill({status:404,json:{code:'entity.not_found'}}));
  await page.getByLabel('Refresh ontology').click();
  await expectNoProtectedContent(page);
  await expect(page.getByRole('status').filter({hasText:'Checking access'})).toBeVisible();
  release();
  await expect(page.getByRole('status').filter({hasText:'This object no longer exists'})).toBeVisible();
  await expect(page.getByRole('searchbox')).toHaveValue('North');
  await expect(page.getByLabel('Depth', {exact:true})).toHaveValue('2');
  await expect(page.getByLabel('Relation', {exact:true})).toHaveValue('supplies');
  await expect(page.getByLabel('Direction', {exact:true})).toHaveValue('in');
  await expect(page.getByLabel('Selected object')).toHaveCount(0);
  await expect(page).toHaveURL(/focus=house/);
  await expect(page).not.toHaveURL(/selected=/);
  await expect(page.locator('main')).toContainText('House blend 1kg');
  await expect(page.locator('main')).not.toContainText('North Coast Beans');
  await page.getByRole('searchbox').fill('');
  await expect(page.getByLabel('Recently viewed objects')).toContainText('House blend 1kg');
});

test('healthy navigation never starts a membership confirmation', async ({page}) => {
  let checks = 0;
  await setup(page);
  await page.route('**/api/vaults/business/members', r => {
    checks++;
    return r.fulfill({json:{results:[]}});
  });
  await page.getByLabel('Depth', {exact:true}).selectOption('2');
  await page.getByRole('searchbox').fill('North');
  await expect(page.getByLabel('Object search results')).toContainText('North Coast Beans');
  await expect(page.getByLabel('Selected object')).toContainText('North Coast Beans');
  await expect(page.getByText('Checking access', {exact:true})).toHaveCount(0);
  expect(checks).toBe(0);
});

test('a late membership success cannot reopen a confirmed denial', async ({page}) => {
  await setup(page);
  let releaseSearch = () => {};
  const heldSearch = new Promise<void>(resolve => {releaseSearch = resolve;});
  let searchStarted = () => {};
  const startedSearch = new Promise<void>(resolve => {searchStarted = resolve;});
  await page.route('**/api/tools/entities_search', async r => {
    searchStarted();
    await heldSearch;
    await r.fulfill({status:403,json:{code:'vault.forbidden'}});
  });
  await page.getByRole('searchbox').fill('Late');
  await startedSearch;
  let releaseMembership = () => {};
  const heldMembership = new Promise<void>(resolve => {releaseMembership = resolve;});
  let delivered = () => {};
  const completed = new Promise<void>(resolve => {delivered = resolve;});
  await page.route('**/api/vaults/business/members', async r => {
    await heldMembership;
    await r.fulfill({json:{results:[]}});
    delivered();
  });
  await page.route('**/api/tools/entities_neighbourhood', r => r.fulfill({json:{error:'not found',code:'entity.not_found'}}));
  await page.getByLabel('Depth', {exact:true}).selectOption('2');
  await expect(page.getByRole('status').filter({hasText:'Checking access'})).toBeVisible();
  releaseSearch();
  await expect(page.getByRole('alert').filter({hasText:'unavailable to you'})).toBeVisible();
  releaseMembership();
  await completed;
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  await expectNoProtectedContent(page);
  await expect(page.getByRole('button', {name:'Retry space access'})).toBeVisible();
  await expect.poll(async () => (await cacheKeys(page)).filter(k => k.includes('business'))).toEqual([]);
});

test('checking and unknown block server text and attributes, then restore input', async ({page}) => {
  await setup(page);
  await page.getByRole('searchbox').fill('House blend 1kg');
  await expect(page.getByLabel('Object search results')).toContainText('House blend 1kg');
  await page.clock.install();
  await page.route('**/api/vaults/business/members', () => {});
  await page.route('**/ontology/objects/n0*', r => r.fulfill({status:404,json:{code:'entity.not_found'}}));
  await page.getByLabel('Refresh ontology').click();
  const absent = async () => {
    await expect.poll(() => protectedValues(page, ['House blend 1kg','North Coast Beans','Banksia Coffee','products.csv'])).toEqual([]);
  };
  await expect(page.getByText('Checking access',{exact:true})).toBeVisible();
  await absent();
  await expect(page.getByRole('searchbox')).toHaveValue('House blend 1kg');
  await expect(page).toHaveURL(/q=House\+blend\+1kg/);
  await page.clock.fastForward(11_000);
  await expect(page.getByRole('button',{name:'Try again',exact:true})).toBeVisible();
  await absent();
  await page.unroute('**/api/vaults/business/members');
  await page.route('**/api/vaults/business/members', r=>r.fulfill({json:{results:[]}}));
  await page.getByRole('button',{name:'Try again',exact:true}).click();
  await expect(page.getByRole('searchbox')).toHaveValue('House blend 1kg');
  await expect(page).toHaveURL(/q=House\+blend\+1kg/);
});
