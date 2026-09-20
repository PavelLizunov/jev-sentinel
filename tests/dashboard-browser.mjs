// Dependency-free Chromium/CDP regression suite. Uses loopback fixtures only.
// Run: CHROME_BIN=/path/to/chrome node tests/dashboard-browser.mjs
import assert from 'node:assert/strict';
import http from 'node:http';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {once} from 'node:events';

const root = path.resolve(import.meta.dirname, '..');
const chrome = process.env.CHROME_BIN;
assert(chrome, 'Set CHROME_BIN to an already installed Chromium binary');
const scratch = await fs.mkdtemp(path.join(os.tmpdir(), 'jev-browser-'));
const artifacts = process.env.ARTIFACT_DIR || scratch;
await fs.mkdir(artifacts, {recursive:true});
const assets = Object.fromEntries(await Promise.all(['html','css','js'].map(async ext => [ext, await fs.readFile(path.join(root, `src/web/dashboard.${ext}`))])));
const localeCodes = ['en','ru','de','fr','es','pt-BR','zh-CN','ja'];
const localeFiles = Object.fromEntries(await Promise.all(localeCodes.map(async loc => [
  loc,
  await fs.readFile(path.join(root, `src/web/locales/${loc}.json`))
])));
const localeCatalogs = Object.fromEntries(localeCodes.map(loc => [loc, JSON.parse(localeFiles[loc].toString('utf8'))]));
const rawI18n = await fs.readFile(path.join(root, 'src/web/i18n.js'), 'utf8');
const enCatalogJson = localeFiles['en'].toString('utf8');
assets['i18n'] = Buffer.from(`globalThis.SentinelEnglish = ${enCatalogJson};\n${rawI18n}`, 'utf8');
let epoch = 'fixture-1', failStatus = false, stale = false, empty = false, invalid = false, starting = false;
let revision = 1, customKey = false, categoryError = false, keyError = false, categorizeError = false, malformedTarget = false;
let categories = [
  {id:'virtualization',label:'Virtualization',description:'Hosts',label_key:'category.virtualization'},
  {id:'workstations',label:'Workstations',description:'Clients',label_key:'category.workstations'}
];
const observations = Array.from({length:100}, (_,i) => ({
  id: `probe:${i}`, name:i === 1 ? '<img src=x onerror=window.INJECTED=1>' : `node-${String(i).padStart(3,'0')}`,
  target_type:'exec_probe', category:i % 2 ? 'workstations' : 'virtualization',
  status: i === 0 ? 'timeout' : 'online', state:i === 0 ? 'unknown' : 'healthy',
  measured_at:'2026-09-19T17:00:00Z', age_ms:2000, sample_seq:'20', latency_ms:i === 0 ? null : 12.5,
  normalized:{cpu_pct: i % 100,ram_used_pct:62,available_mib:4000,swap_used_mib:i ? 32 : 0,total_memory_mib:16000,memory_basis:'Synthetic fixture, not a real machine',memory_basis_code:'synthetic'},
  metrics:{status:'running',name:'<script>window.INJECTED=1</script>'},
  error_code:i === 0 ? 'observation_unavailable' : null,
  error_message:i === 0 ? 'Observation unavailable.' : null,
  history:Array.from({length:20}, (_,j) => ({t_ms:j*10000,latency_ms:j===10?null:10+j,ram_pct:40+j,swap_mib:j,break_before:j===15})),
}));
function snapshot() {
  return {schema_version:invalid?999:2,daemon_epoch:epoch,view_revision:String(revision++),cycle_id:1,timestamp:'2026-09-19T17:00:00Z',
    phase:starting?'starting':'ready',advisor_state:'ready',system_health:'healthy',risk_score:0,health_confidence:null,
    suggested_action:'none',stale_after_ms:30000,categories,
    targets:empty||starting?[]:malformedTarget?[null]:observations.map(t=>({...t,age_ms:stale?31000:2000}))};
}
const requests = [];
const server = http.createServer(async (req,res) => {
  requests.push(`${req.method} ${req.url}`);
  const json = (body, status=200) => {res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(body));};
  if (req.url==='/api/status') return failStatus?json({error:'unavailable'},503):json(snapshot());
  if (req.url==='/api/key-status') return json({has_custom_key:customKey,active_key_masked:'test…1234',default_key_masked:'server…1234'});
  if (req.url==='/api/categorize') return categorizeError?json({error:'Fixture categorization error'},502):json({success:true,categorized_count:100});
  if (req.method==='POST') {
    let body=''; for await(const chunk of req) body+=chunk; const input=JSON.parse(body);
    if(req.url==='/api/key') { if(keyError)return json({error:'Fixture invalid key'},400); customKey=input.api_key!==null; return json({success:true}); }
    if(req.url==='/api/categories') { if(categoryError)return json({error:'Fixture category error'},400); if(input.delete){categories=categories.filter(c=>c.id!==input.id);return json(categories);} categories.push(input); return json(categories); }
  }
  if (req.url === '/i18n.js') {
    res.writeHead(200, {'Content-Type': 'text/javascript; charset=utf-8'});
    return res.end(assets['i18n']);
  }
  if (req.url.startsWith('/locales/')) {
    const code = req.url.slice('/locales/'.length).replace(/\.json$/, '');
    if (localeFiles[code]) {
      res.writeHead(200, {'Content-Type': 'application/json; charset=utf-8'});
      return res.end(localeFiles[code]);
    }
    res.writeHead(404);
    return res.end();
  }
  const ext=req.url==='/'?'html':req.url==='/dashboard.css'?'css':req.url==='/dashboard.js'?'js':null;
  if(!ext){res.writeHead(404);return res.end();}
  res.writeHead(200,{'Content-Type':{html:'text/html',css:'text/css',js:'text/javascript'}[ext]});res.end(assets[ext]);
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const origin=`http://127.0.0.1:${server.address().port}`;
const browser=spawn(chrome,['--headless=new','--no-sandbox','--disable-gpu','--disable-background-networking','--disable-component-update','--disable-sync','--no-first-run','--no-default-browser-check','--remote-debugging-port=0',`--user-data-dir=${scratch}`,'about:blank'],{stdio:['ignore','ignore','pipe'],env:{...process.env,HOME:scratch,XDG_CONFIG_HOME:scratch,XDG_CACHE_HOME:scratch}});
let browserError='';browser.stderr.on('data',chunk=>browserError+=chunk.toString());
let socket;
const checks=[];
try {
  let port;
  const end=Date.now()+10000;
  while(Date.now()<end){
    try {port=(await fs.readFile(path.join(scratch,'DevToolsActivePort'),'utf8')).split('\n')[0];break;} catch {}
    if(browser.exitCode!==null || browser.signalCode!==null) throw new Error(`Chromium exited (${browser.exitCode}/${browser.signalCode}): ${browserError}`);
    await new Promise(r=>setTimeout(r,50));
  }
  assert(port,`Chromium debugging endpoint did not start: ${browserError}`);
  const page=await fetch(`http://127.0.0.1:${port}/json/new?about:blank`,{method:'PUT'}).then(r=>r.json());
  socket=new WebSocket(page.webSocketDebuggerUrl); await once(socket,'open');
  let nextId=0;const pending=new Map();const runtimeErrors=[];
  socket.addEventListener('message',({data})=>{const m=JSON.parse(data);if(m.id){const p=pending.get(m.id);pending.delete(m.id);m.error?p.reject(new Error(JSON.stringify(m.error))):p.resolve(m.result);}else if(m.method==='Runtime.exceptionThrown')runtimeErrors.push(m.params.exceptionDetails);});
  function cdp(method,params={}){return new Promise((resolve,reject)=>{const id=++nextId;pending.set(id,{resolve,reject});socket.send(JSON.stringify({id,method,params}));});}
  async function evaluate(expression){const r=await cdp('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw new Error(JSON.stringify(r.exceptionDetails));return r.result.value;}
  async function waitFor(expression){const end=Date.now()+6000;while(Date.now()<end){if(await evaluate(expression))return;await new Promise(r=>setTimeout(r,25));}throw new Error(`Timeout: ${expression}`);}
  async function check(name,fn){await fn();checks.push(name);console.log(`PASS ${name}`);}
  const click=selector=>evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
  async function input(id,value){await evaluate(`document.getElementById(${JSON.stringify(id)}).value=${JSON.stringify(value)};document.getElementById(${JSON.stringify(id)}).dispatchEvent(new Event('input',{bubbles:true}))`);}
  async function switchLanguage(loc){
    await evaluate(`(async () => {
      const s = document.getElementById('languageSelect');
      s.value = ${JSON.stringify(loc)};
      s.dispatchEvent(new Event('change', {bubbles: true}));
    })()`);
    await waitFor(`document.documentElement.lang === ${JSON.stringify(loc)} && Boolean(globalThis.SentinelI18n) && globalThis.SentinelI18n.locale === ${JSON.stringify(loc)}`);
  }
  async function assertNoRawLeaks(loc){
    const leaks = await evaluate(`(() => {
      const rawPattern = /\\{[a-zA-Z0-9_]+\\}/;
      const keyPattern = /\\b(app|language|connection|telemetry|advisor|summary|state|health|advice|action|attention|fleet|view|filter|metric|unit|value|time|history|category|table|empty|footer|details|memory|button|key|category_form|operation|error)\\.[a-z0-9_]+/i;
      const found = [];
      const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, null);
      let node;
      while ((node = walker.nextNode())) {
        const p = node.parentElement;
        if (!p || p.tagName === 'SCRIPT' || p.tagName === 'STYLE' || p.closest('[hidden]')) continue;
        if (p.id === 'detailRaw' || p.closest('#detailRaw') || p.tagName === 'PRE' || p.tagName === 'CODE') continue;
        const t = node.textContent.trim();
        if (!t) continue;
        if (rawPattern.test(t)) found.push({type: 'placeholder', text: t, id: p.id, tag: p.tagName});
        if (keyPattern.test(t)) found.push({type: 'raw_key', text: t, id: p.id, tag: p.tagName});
      }
      const attrs = ['aria-label', 'title', 'placeholder'];
      for (const el of document.querySelectorAll('*')) {
        if (el.closest('[hidden]')) continue;
        for (const attr of attrs) {
          const val = el.getAttribute(attr);
          if (val && rawPattern.test(val)) found.push({type: 'placeholder_attr', attr, text: val, id: el.id, tag: el.tagName});
          if (val && keyPattern.test(val)) found.push({type: 'raw_key_attr', attr, text: val, id: el.id, tag: el.tagName});
        }
      }
      return found;
    })()`);
    assert.deepEqual(leaks, [], `Raw key or placeholder leaks in ${loc}: ${JSON.stringify(leaks)}`);
  }

  await cdp('Runtime.enable');await cdp('Page.enable');
  await cdp('Emulation.setDeviceMetricsOverride',{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
  await cdp('Page.navigate',{url:origin});await waitFor('document.querySelectorAll("#fleetMap button").length===100');
  await check('100 probes, zero risk and injection-safe labels',async()=>{
    assert.equal(await evaluate('document.querySelectorAll("#grid article").length'),100);
    assert.equal(await evaluate('document.getElementById("risk").textContent'),'0.00 / 1.00');
    assert.equal(await evaluate('Boolean(window.INJECTED)'),false);
    assert.equal(await evaluate('document.querySelectorAll("#grid img, #grid script").length'),0);
  });
  await check('refresh preserves DOM and keyboard focus',async()=>{
    await evaluate('window.originalCard=document.querySelector("#grid article");window.originalButton=originalCard.querySelector("button");originalButton.focus();');
    await evaluate('fetchStatus()');
    assert.equal(await evaluate('originalCard===document.querySelector("#grid article") && document.activeElement===originalButton'),true);
  });
  await check('density modes preserve filters and persist',async()=>{
    await input('search','node-00');await click('[data-view="cards"]');
    assert.equal(await evaluate('document.querySelectorAll("#grid article").length'),9);
    await click('[data-view="table"]');assert.equal(await evaluate('document.querySelectorAll("#tableBody tr").length'),9);
    await cdp('Page.reload');await waitFor('document.querySelectorAll("#tableBody tr").length===9');
    assert.equal(await evaluate('localStorage.getItem("sentinel.view")'),'table');
    await click('#clearFilters');assert.equal(await evaluate('document.querySelectorAll("#tableBody tr").length'),100);
  });
  await check('table sorting and missing values last',async()=>{
    await click('[data-sort="latency"]');
    assert.equal(await evaluate('document.querySelector("#tableBody tr:last-child").dataset.id'),'probe:0');
    await click('[data-sort="cpu"]');assert.equal(await evaluate('document.querySelector("#tableBody tr").dataset.id'),'probe:99');
    await click('[data-sort="name"]');
  });
  await check('details, real-time SVG gaps and Escape',async()=>{
    await click('#fleetMap button');await waitFor('document.getElementById("detailDialog").open');
    assert.equal(await evaluate('document.querySelectorAll("#detailCharts svg").length'),3);
    const commands=await evaluate('document.querySelector("#detailCharts svg path:not(.guide)").getAttribute("d")');
    assert.equal((commands.match(/M/g)||[]).length,3);
    assert.equal(await evaluate('Boolean(window.INJECTED)'),false);
    await cdp('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
    await cdp('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
    await waitFor('!document.getElementById("detailDialog").open');
  });
  await check('issues, categories, grouping and empty filters',async()=>{
    await click('[data-view="compact"]');await click('#issues');assert.equal(await evaluate('document.querySelectorAll("#grid article").length'),1);
    await click('#clearFilters');
    await evaluate('document.getElementById("category").value="workstations";document.getElementById("category").dispatchEvent(new Event("change"))');
    assert.equal(await evaluate('document.querySelectorAll("#grid article").length'),50);
    await click('#clearFilters');await evaluate('document.getElementById("group").value="category";document.getElementById("group").dispatchEvent(new Event("change"))');
    assert.equal(await evaluate('document.querySelectorAll(".group-heading").length'),2);
    await input('search','no-such-node');assert.match(await evaluate('document.getElementById("empty").textContent'),/No probes match/);
    await click('#clearFilters');
  });
  await check('fresh HTTP with stale telemetry cannot show healthy advice',async()=>{
    stale=true;await evaluate('fetchStatus()');assert.equal(await evaluate('document.getElementById("count-stale").textContent'),'100');
    assert.equal(await evaluate('document.getElementById("health").textContent'),'Unknown');
    assert.match(await evaluate('document.getElementById("connection").textContent'),/connected/);stale=false;
  });
  await check('HTTP error retains last samples; invalid schema is rejected',async()=>{
    failStatus=true;await evaluate('fetchStatus()');assert.equal(await evaluate('document.querySelectorAll("#fleetMap button").length'),100);
    assert.equal(await evaluate('document.getElementById("connectionError").hidden'),false);failStatus=false;
    invalid=true;await evaluate('fetchStatus()');assert.equal(await evaluate('document.getElementById("connectionError").hidden'),false);invalid=false;await evaluate('fetchStatus()');
    await evaluate('window.lastGoodStatus = data');
    malformedTarget=true;await evaluate('fetchStatus()');
    assert.equal(await evaluate('data === lastGoodStatus'),true);
    assert.equal(await evaluate('document.querySelectorAll("#fleetMap button").length'),100);
    malformedTarget=false;await evaluate('fetchStatus()');
  });
  await check('key dialog save/reset/error and secret cleanup',async()=>{
    await click('#tokenButton');await waitFor('document.getElementById("keyStatus").textContent.includes("Server")');
    await input('keyInput','fixture-only-key');await evaluate('document.getElementById("tokenForm").requestSubmit()');
    await waitFor('document.getElementById("tokenMessage").textContent.includes("updated")');assert.equal(customKey,true);
    await click('#resetKey');await waitFor('document.getElementById("tokenMessage").textContent.includes("configured server")');assert.equal(customKey,false);
    keyError=true;await input('keyInput','bad-fixture');await evaluate('document.getElementById("tokenForm").requestSubmit()');await waitFor('document.getElementById("tokenMessage").textContent.includes("invalid key")');keyError=false;
    await click('[data-close="tokenDialog"]');await waitFor('document.getElementById("keyInput").value===""');assert.equal(await evaluate('document.getElementById("keyInput").value'),'');
  });
  await check('category create, error and categorization states',async()=>{
    await click('#categoryButton');await input('categoryId','databases');await input('categoryLabel','Databases');await input('categoryDescription','Database service');
    categoryError=true;await evaluate('document.getElementById("categoryForm").requestSubmit()');await waitFor('document.getElementById("categoryMessage").textContent.includes("category error")');categoryError=false;
    await evaluate('document.getElementById("categoryForm").requestSubmit()');await waitFor('!document.getElementById("categoryDialog").open');

    // Test category deletion on a temporary category
    await click('#categoryButton');await input('categoryId','temp_del');await input('categoryLabel','Temp Delete');await input('categoryDescription','To delete');
    await evaluate('document.getElementById("categoryForm").requestSubmit()');await waitFor('!document.getElementById("categoryDialog").open');
    await click('#categoryButton');await waitFor('document.getElementById("categoryDialog").open');
    const items = await evaluate('Array.from(document.querySelectorAll("#categoryList .category-item")).map(el => el.querySelector(".category-item-id")?.textContent)');
    assert(items.includes('temp_del'));
    await evaluate('Array.from(document.querySelectorAll("#categoryList .category-item")).find(el => el.querySelector(".category-item-id")?.textContent === "temp_del").querySelector(".category-item-del").click()');
    await waitFor('!Array.from(document.querySelectorAll("#categoryList .category-item")).some(el => el.querySelector(".category-item-id")?.textContent === "temp_del")');
    await click('[data-close="categoryDialog"]');await waitFor('!document.getElementById("categoryDialog").open');

    await click('#categorize');await waitFor('document.getElementById("operationMessage").textContent.includes("Categorized 100")');
    categorizeError=true;await click('#categorize');await waitFor('document.getElementById("operationMessage").textContent.includes("categorization error")');categorizeError=false;
  });
  await check('empty and initial loading states; epoch reset clears selection',async()=>{
    await click('#fleetMap button');
    empty=true;await evaluate('fetchStatus()');assert.match(await evaluate('document.getElementById("empty").textContent'),/No probes configured/);
    assert.equal(await evaluate('document.getElementById("detailDialog").open'),false);
    starting=true;await evaluate('fetchStatus()');assert.match(await evaluate('document.getElementById("empty").textContent'),/first observation/);
    empty=false;starting=false;await evaluate('fetchStatus()');await click('#fleetMap button');epoch='fixture-2';await evaluate('fetchStatus()');
    assert.equal(await evaluate('document.getElementById("detailDialog").open'),false);
  });
  await click('[data-view="compact"]');
  await evaluate('document.getElementById("operationMessage").textContent=""');
  await fs.writeFile(path.join(artifacts,'dashboard-desktop.png'),Buffer.from((await cdp('Page.captureScreenshot',{format:'png'})).data,'base64'));
  await check('mobile modes and dialogs fit viewport',async()=>{
    await cdp('Emulation.setDeviceMetricsOverride',{width:390,height:844,deviceScaleFactor:1,mobile:true});
    for(const mode of ['cards','compact','table']){
      await click(`[data-view="${mode}"]`);
      assert.equal(await evaluate('document.documentElement.scrollWidth <= innerWidth'),true,mode);
    }
    await click('#fleetMap button');assert.equal(await evaluate('document.getElementById("detailDialog").getBoundingClientRect().right<=innerWidth'),true);
    await click('[data-close="detailDialog"]');
    assert.equal(await evaluate('Array.from(document.querySelectorAll(".fleet-cell")).every(b=>b.getBoundingClientRect().height>=44)'),true);
    assert.equal(await evaluate('Array.from(document.querySelectorAll("#tableBody .target-name")).every(b=>b.getBoundingClientRect().height>=44)'),true);
  });
  await check('mobile initial overview stays expanded with all fleet cells',async()=>{
    await cdp('Page.reload');await waitFor('document.querySelectorAll("#fleetMap button").length===100');
    assert.equal(await evaluate('document.getElementById("fleetContent").hidden'),false);
    assert.equal(await evaluate('document.documentElement.scrollWidth <= innerWidth'),true);
  });
  await fs.writeFile(path.join(artifacts,'dashboard-mobile.png'),Buffer.from((await cdp('Page.captureScreenshot',{format:'png'})).data,'base64'));

  // Scenario 14: Eight locales live #languageSelect change, html.lang, static translations and runtime global
  await cdp('Emulation.setDeviceMetricsOverride',{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
  await check('eight locales: live #languageSelect change, html.lang, static translations, I.locale', async () => {
    for (const loc of localeCodes) {
      await switchLanguage(loc);
      const cat = localeCatalogs[loc];
      assert.equal(await evaluate('globalThis.SentinelI18n.locale'), loc);
      assert.equal(await evaluate('document.documentElement.lang'), loc);
      assert.equal(await evaluate('document.getElementById("languageSelect").value'), loc);
      assert.equal(await evaluate('document.title'), cat['app.title']);
      assert.equal(await evaluate('document.querySelector("[data-i18n=\\"app.subtitle\\"]").textContent'), cat['app.subtitle']);
      assert.equal(await evaluate('document.getElementById("categorize").textContent'), cat['button.categorize']);
      assert.equal(await evaluate('document.getElementById("categoryButton").textContent'), cat['button.add_category']);
      assert.equal(await evaluate('document.getElementById("tokenButton").textContent'), cat['button.jev_key']);
      assert.equal(await evaluate('document.querySelector("footer").textContent'), cat['footer.note']);
      if (loc === 'en') {
        assert.equal(await evaluate('document.getElementById("translationNote").hidden'), true);
      } else {
        assert.equal(await evaluate('document.getElementById("translationNote").hidden'), false);
        assert.equal(await evaluate('document.getElementById("translationNote").textContent'), cat['language.machine_note']);
      }
    }
  });

  // Scenario 15: Eight locales dynamic states, spark titles, aria labels, errors and empty loading
  await check('eight locales: dynamic states, chart titles, aria labels, errors and empty loading', async () => {
    await click('[data-view="cards"]');
    for (const loc of localeCodes) {
      await switchLanguage(loc);
      const cat = localeCatalogs[loc];
      assert.equal(await evaluate('document.getElementById("advisor").textContent'), cat['advisor.ready']);
      assert.equal(await evaluate('document.getElementById("health").textContent'), cat['health.healthy']);
      assert.equal(await evaluate('document.getElementById("advice").textContent'), cat['action.none']);
      assert.equal(await evaluate('document.querySelector(".statusline").getAttribute("aria-label")'), cat['connection.label']);
      assert.equal(await evaluate('document.querySelector(".summary").getAttribute("aria-label")'), cat['summary.label']);
      assert.equal(await evaluate('document.querySelector(".view-buttons").getAttribute("aria-label")'), cat['view.label']);
      assert.equal(await evaluate('document.getElementById("fleetMap").getAttribute("aria-label")'), cat['fleet.label']);
      assert.equal(await evaluate('document.querySelector("#tableWrap table").getAttribute("aria-label")'), cat['table.label']);
      assert.equal(await evaluate('document.getElementById("search").getAttribute("placeholder")'), cat['filter.search_placeholder']);

      const sparkOk = await evaluate('Array.from(document.querySelectorAll("#grid .spark")).every(s => Boolean(s.getAttribute("aria-label")) && s.getAttribute("aria-label") === s.querySelector("title").textContent && !s.getAttribute("aria-label").includes("undefined"))');
      assert.equal(sparkOk, true);

      const probe0Err = await evaluate('document.querySelector("[data-id=\\"probe:0\\"] .error-text")?.textContent');
      assert.equal(probe0Err, cat['error.observation_unavailable']);

      await input('search', 'no-match-for-filter-xyz');
      assert.equal(await evaluate('document.getElementById("empty").textContent'), cat['empty.no_matches']);
      await click('#clearFilters');

      await assertNoRawLeaks(loc);
    }
  });

  // Scenario 16: Category user data and ID invariance across language changes
  await check('category user data and ID invariance across all locales', async () => {
    for (const loc of localeCodes) {
      await switchLanguage(loc);
      const cat = localeCatalogs[loc];
      const optLabels = await evaluate('Array.from(document.getElementById("category").options).map(o => ({ value: o.value, text: o.text }))');
      const virt = optLabels.find(o => o.value === 'virtualization');
      const work = optLabels.find(o => o.value === 'workstations');
      const db = optLabels.find(o => o.value === 'databases');
      assert(virt && virt.text === cat['category.virtualization'], `Virt option in ${loc}`);
      assert(work && work.text === cat['category.workstations'], `Work option in ${loc}`);
      assert(db && db.text === 'Databases', `User database category preserved in ${loc}`);

      await evaluate('document.getElementById("group").value = "category"; document.getElementById("group").dispatchEvent(new Event("change"))');
      const headings = await evaluate('Array.from(document.querySelectorAll(".group-heading")).map(h => h.textContent)');
      assert(headings.includes(cat['category.virtualization']), `Heading virt in ${loc}`);
      assert(headings.includes(cat['category.workstations']), `Heading work in ${loc}`);
      assert.equal(await evaluate('categoryLabel("databases")'), 'Databases', `categoryLabel for user category in ${loc}`);
      await evaluate('document.getElementById("group").value = "none"; document.getElementById("group").dispatchEvent(new Event("change"))');
    }
  });

  // Scenario 17: DOM, focus, filters, selected detail retained without status refetch on switch
  await check('locale switch retains DOM, focus, filters, and open details without status refetch', async () => {
    await switchLanguage('en');
    await input('search', 'node-02');
    await evaluate('document.getElementById("category").value = "workstations"; document.getElementById("category").dispatchEvent(new Event("change"))');
    await evaluate('document.getElementById("search").focus()');
    await click('#fleetMap button');
    await waitFor('document.getElementById("detailDialog").open');
    await evaluate('window.retainedCard = document.querySelector("#grid article")');
    await evaluate('window.retainedFocus = document.activeElement; window.retainedDetailPath = document.querySelector("#detailCharts path:not(.guide)").getAttribute("d")');
    const reqsBefore = requests.filter(r => r.includes('/api/status')).length;
    await switchLanguage('de');
    const reqsAfter = requests.filter(r => r.includes('/api/status')).length;
    assert.equal(reqsAfter, reqsBefore, 'Switching locale must not refetch status');

    assert.equal(await evaluate('window.retainedCard === document.querySelector("#grid article")'), true);
    assert.equal(await evaluate('document.activeElement === window.retainedFocus'), true);
    assert.equal(await evaluate('document.querySelector("#detailCharts path:not(.guide)").getAttribute("d") === window.retainedDetailPath'), true);
    assert.equal(await evaluate('document.getElementById("search").value'), 'node-02');
    assert.equal(await evaluate('document.getElementById("category").value'), 'workstations');
    assert.equal(await evaluate('document.getElementById("detailDialog").contains(document.activeElement)'), true);
    assert.equal(await evaluate('document.getElementById("detailDialog").open'), true);
    assert.equal(await evaluate('document.getElementById("detailTitle").textContent'), 'node-000');
    assert.equal(await evaluate('document.getElementById("memoryBasis").textContent'), localeCatalogs['de']['memory.synthetic']);
    assert.equal(await evaluate('document.getElementById("detailError").textContent'), localeCatalogs['de']['error.observation_unavailable']);

    await click('[data-close="detailDialog"]');
    await waitFor('!document.getElementById("detailDialog").open');
    await click('#clearFilters');
  });

  // Scenario 18: Open form fields retained and messages retranslate on language switch
  await check('open form inputs preserved and form messages retranslated on locale switch', async () => {
    await switchLanguage('de');
    await click('#tokenButton');
    await waitFor('document.getElementById("tokenDialog").open');
    await input('keyInput', 'user-draft-secret-key');
    keyError = true;
    await evaluate('document.getElementById("tokenForm").requestSubmit()');
    await waitFor('document.getElementById("tokenMessage").textContent.includes("invalid key")');
    keyError = false;

    await switchLanguage('ja');
    assert.equal(await evaluate('document.getElementById("tokenDialog").open'), true);
    assert.equal(await evaluate('document.getElementById("keyInput").value'), 'user-draft-secret-key');
    assert.equal(await evaluate('document.getElementById("tokenTitle").textContent'), localeCatalogs['ja']['key.title']);
    assert.equal(await evaluate('document.getElementById("resetKey").textContent'), localeCatalogs['ja']['key.use_server']);
    assert.equal(await evaluate('document.getElementById("saveKey").textContent'), localeCatalogs['ja']['key.save']);

    await click('[data-close="tokenDialog"]');
    await waitFor('document.getElementById("keyInput").value===""');

    await click('#categoryButton');
    await waitFor('document.getElementById("categoryDialog").open');
    await input('categoryId', 'analytics');
    await input('categoryLabel', 'Analytics & Reports');
    await input('categoryDescription', 'Business metrics');

    await switchLanguage('pt-BR');
    assert.equal(await evaluate('document.getElementById("categoryDialog").open'), true);
    assert.equal(await evaluate('document.getElementById("categoryId").value'), 'analytics');
    assert.equal(await evaluate('document.getElementById("categoryLabel").value'), 'Analytics & Reports');
    assert.equal(await evaluate('document.getElementById("categoryDescription").value'), 'Business metrics');
    assert.equal(await evaluate('document.getElementById("categoryTitle").textContent'), localeCatalogs['pt-BR']['category_form.title']);
    assert.equal(await evaluate('document.getElementById("saveCategory").textContent'), localeCatalogs['pt-BR']['category_form.save']);

    await click('[data-close="categoryDialog"]');
    await waitFor('!document.getElementById("categoryDialog").open');
  });

  // Scenario 19: Locale persistence in localStorage across page reload
  await check('locale storage reload restores active language', async () => {
    await switchLanguage('ru');
    assert.equal(await evaluate('localStorage.getItem("sentinel.locale")'), 'ru');
    await cdp('Page.reload');
    await waitFor('document.querySelectorAll("#fleetMap button").length===100');
    assert.equal(await evaluate('document.documentElement.lang'), 'ru');
    assert.equal(await evaluate('document.getElementById("languageSelect").value'), 'ru');
    assert.equal(await evaluate('globalThis.SentinelI18n.locale'), 'ru');
    assert.equal(await evaluate('document.getElementById("categorize").textContent'), localeCatalogs['ru']['button.categorize']);
  });

  // Scenario 20: Blocked localStorage and browser language negotiation fallback
  await check('fallback language negotiation when localStorage is unavailable', async () => {
    const neg = await evaluate(`(async () => {
      const origGetItem = Storage.prototype.getItem;
      let blockedOk = false;
      try {
        Storage.prototype.getItem = () => { throw new Error('Blocked localStorage'); };
        const oldSetItem = Storage.prototype.setItem;
        Storage.prototype.setItem = () => { throw new Error('Blocked localStorage'); };
        try {
          await globalThis.SentinelI18n.init();
          await globalThis.SentinelI18n.setLocale('es');
          blockedOk = globalThis.SentinelI18n.locale === 'es';
        } finally { Storage.prototype.setItem = oldSetItem; }
      } finally {
        Storage.prototype.getItem = origGetItem;
      }
      localStorage.removeItem('sentinel.locale');
      const origLangs = navigator.languages;
      let deNegotiated = null, twFallback = null;
      try {
        Object.defineProperty(navigator, 'languages', {value: ['de-DE', 'de'], configurable: true});
        await globalThis.SentinelI18n.init();
        deNegotiated = globalThis.SentinelI18n.locale;

        localStorage.removeItem('sentinel.locale');
        Object.defineProperty(navigator, 'languages', {value: ['zh-TW'], configurable: true});
        await globalThis.SentinelI18n.init();
        twFallback = globalThis.SentinelI18n.locale;
      } finally {
        Object.defineProperty(navigator, 'languages', {value: origLangs, configurable: true});
      }
      return {blockedOk, deNegotiated, twFallback};
    })()`);
    assert.equal(neg.blockedOk, true, 'setLocale survives blocked localStorage');
    assert.equal(neg.deNegotiated, 'de', 'negotiates de from de-DE');
    assert.equal(neg.twFallback, 'en', 'Traditional Chinese falls back to en');
  });

  // Scenario 21: Keyboard navigation: focus, space toggle, enter and escape
  await check('keyboard navigation: focus, space toggle, enter submit, and escape dismiss', async () => {
    await switchLanguage('en');
    await click('[data-view="compact"]');
    await evaluate('document.querySelector("[data-view=\\"cards\\"]").focus()');
    await cdp('Input.dispatchKeyEvent', {type: 'keyDown', key: ' ', code: 'Space', text: ' ', unmodifiedText: ' ', windowsVirtualKeyCode: 32});
    await cdp('Input.dispatchKeyEvent', {type: 'keyUp', key: ' ', code: 'Space', text: ' ', unmodifiedText: ' ', windowsVirtualKeyCode: 32});
    assert.equal(await evaluate('document.querySelector("[data-view=\\"cards\\"]").getAttribute("aria-pressed")'), 'true');

    await evaluate('document.getElementById("issues").focus()');
    await cdp('Input.dispatchKeyEvent', {type: 'keyDown', key: ' ', code: 'Space', text: ' ', unmodifiedText: ' ', windowsVirtualKeyCode: 32});
    await cdp('Input.dispatchKeyEvent', {type: 'keyUp', key: ' ', code: 'Space', text: ' ', unmodifiedText: ' ', windowsVirtualKeyCode: 32});
    assert.equal(await evaluate('document.getElementById("issues").checked'), true);
    await cdp('Input.dispatchKeyEvent', {type: 'keyDown', key: ' ', code: 'Space', text: ' ', unmodifiedText: ' ', windowsVirtualKeyCode: 32});
    await cdp('Input.dispatchKeyEvent', {type: 'keyUp', key: ' ', code: 'Space', text: ' ', unmodifiedText: ' ', windowsVirtualKeyCode: 32});
    assert.equal(await evaluate('document.getElementById("issues").checked'), false);

    await evaluate('document.getElementById("categoryButton").focus()');
    await cdp('Input.dispatchKeyEvent', {type: 'keyDown', key: 'Enter', code: 'Enter', text: '\r', unmodifiedText: '\r', windowsVirtualKeyCode: 13});
    await cdp('Input.dispatchKeyEvent', {type: 'keyUp', key: 'Enter', code: 'Enter', text: '\r', unmodifiedText: '\r', windowsVirtualKeyCode: 13});
    await waitFor('document.getElementById("categoryDialog").open');

    await cdp('Input.dispatchKeyEvent', {type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27});
    await cdp('Input.dispatchKeyEvent', {type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27});
    await waitFor('!document.getElementById("categoryDialog").open');
    await click('[data-view="compact"]');
  });

  // Scenario 22: Desktop 1440, Mobile 390, Mobile 320 viewports across all 8 locales with zero overflow
  await check('desktop 1440 and mobile 390/320 viewports across all 8 locales with zero overflow', async () => {
    const viewports = [
      {width: 1440, height: 1000, mobile: false, name: 'desktop1440'},
      {width: 390, height: 844, mobile: true, name: 'mobile390'},
      {width: 320, height: 568, mobile: true, name: 'mobile320'}
    ];
    for (const vp of viewports) {
      await cdp('Emulation.setDeviceMetricsOverride', {width: vp.width, height: vp.height, deviceScaleFactor: 1, mobile: vp.mobile});
      for (const loc of localeCodes) {
        await switchLanguage(loc);
        for (const density of ['cards', 'compact', 'table']) {
          await click(`[data-view="${density}"]`);
          const overflow = await evaluate('document.documentElement.scrollWidth > innerWidth || document.body.scrollWidth > innerWidth');
          assert.equal(overflow, false, `Overflow in ${vp.name} ${loc} ${density}`);
        }
      }
    }
  });

  await check('all locales: stale, starting, empty, disconnected and coded form errors', async () => {
    for (const loc of localeCodes) {
      await switchLanguage(loc);
      const cat = localeCatalogs[loc];
      assert.equal(await evaluate('document.getElementById("connection").textContent'), cat['connection.connected']);
      stale = true; await evaluate('fetchStatus()');
      assert.equal(await evaluate('document.getElementById("health").textContent'), cat['health.unknown']);
      assert.equal(await evaluate('document.getElementById("telemetry").textContent === I.t("telemetry.stale", {count: 100})'), true);
      stale = false; empty = true; await evaluate('fetchStatus()');
      assert.equal(await evaluate('document.getElementById("empty").textContent'), cat['empty.no_probes']);
      starting = true; await evaluate('fetchStatus()');
      assert.equal(await evaluate('document.getElementById("empty").textContent'), cat['empty.first_cycle']);
      starting = false; empty = false; await evaluate('fetchStatus()');
      failStatus = true; await evaluate('fetchStatus()');
      assert.equal(await evaluate('document.getElementById("connection").textContent'), cat['connection.disconnected']);
      assert.equal(await evaluate('document.getElementById("connectionError").textContent'), cat['connection.lost']);
      failStatus = false; await evaluate('fetchStatus()');
      await evaluate('tokenMessageState = {key: "error.key_rejected"}; categoryMessageState = {key: "error.invalid_json"}; refreshMessages()');
      assert.equal(await evaluate('document.getElementById("tokenMessage").textContent'), cat['error.key_rejected']);
      assert.equal(await evaluate('document.getElementById("categoryMessage").textContent'), cat['error.invalid_json']);
    }
    await evaluate('tokenMessageState = categoryMessageState = operationMessageState = null; refreshMessages()');
  });

  await check('initial disconnected state and nested schema errors retranslate without data', async () => {
    failStatus = true;
    await cdp('Page.reload');
    await waitFor('document.getElementById("connectionError").hidden === false');
    for (const loc of ['ru', 'de', 'ja']) {
      await switchLanguage(loc);
      assert.equal(await evaluate('document.getElementById("connection").textContent'), localeCatalogs[loc]['connection.disconnected']);
      assert.equal(await evaluate('document.getElementById("empty").textContent'), localeCatalogs[loc]['empty.retrying']);
    }
    failStatus = false; invalid = true; await evaluate('fetchStatus()');
    await switchLanguage('ru');
    assert.equal(await evaluate('document.getElementById("connectionError").textContent.includes(I.t("error.unsupported_schema"))'), true);
    invalid = false; await evaluate('fetchStatus()');
  });

  // Owner-reported UX regressions, using real DOM/layout and isolated browser storage.
  await check('substring search: 012, exact ID, type, category ID and localized label', async () => {
    await switchLanguage('en'); await click('#clearFilters'); await click('[data-view="compact"]');
    for (const [query, count] of [[' 012 ',1], ['PROBE:12',1], ['EXEC_PROBE',100], ['virtualization',50], ['Workstations',50]]) {
      await input('search',query);
      assert.equal(await evaluate('document.querySelectorAll("#grid article").length'),count,query);
    }
    await switchLanguage('ru'); await input('search',localeCatalogs.ru['category.virtualization']);
    assert.equal(await evaluate('document.querySelectorAll("#grid article").length'),50);
    await click('#clearFilters');
  });
  await check('hidden matches explained; explicit recovery retains query and persists', async () => {
    for (const loc of localeCodes) {
      await switchLanguage(loc); await input('search','012'); await click('#issues');
      assert.equal(await evaluate('document.querySelectorAll("#grid article").length'),0);
      assert.equal(await evaluate('document.getElementById("filterFeedback").hidden'),false);
      assert.equal(await evaluate('document.getElementById("activeFilters").textContent.includes(I.t("filter.issues"))'),true);
      assert.equal(await evaluate('document.getElementById("hiddenMatches").textContent'),localeCatalogs[loc]['filter.hidden_matches'].replace('{count}','1'));
      await click('#showAllMatches');
      assert.equal(await evaluate('document.getElementById("search").value'),'012');
      assert.equal(await evaluate('document.activeElement.id'),'search');
      assert.equal(await evaluate('document.querySelectorAll("#grid article").length'),1);
      await evaluate('document.getElementById("category").value="workstations";document.getElementById("category").dispatchEvent(new Event("change"))');
      assert.equal(await evaluate('document.getElementById("filterFeedback").hidden'),false);
      await click('#showAllMatches');
      await input('search','not-a-real-probe');
      assert.equal(await evaluate('document.getElementById("filterFeedback").hidden'),true);
      await click('#clearFilters');
    }
    await input('search','012'); await click('#issues'); await cdp('Page.reload');
    await waitFor('document.querySelectorAll("#fleetMap button").length===100');
    assert.equal(await evaluate('document.getElementById("filterFeedback").hidden'),false);
    await click('#showAllMatches'); await click('#clearFilters');
    await evaluate('document.getElementById("group").value="category";document.getElementById("group").dispatchEvent(new Event("change"))');
    await click('#clearFilters');
    assert.equal(await evaluate('document.getElementById("group").value'),'none');
  });
  const frame = () => evaluate('new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))');
  const controlGeometry = () => evaluate(`['search','fleetToggle'].map(id=>{const r=document.getElementById(id).getBoundingClientRect();return [r.x,r.y,r.width,r.height]})`);
  await check('100→1→0→100 and polls keep controls and search focus stable', async () => {
    await switchLanguage('ru');
    for (const width of [1440,390,320]) {
      await cdp('Emulation.setDeviceMetricsOverride',{width,height:900,deviceScaleFactor:1,mobile:false});
      for (const view of ['cards','compact','table']) {
        await click(`[data-view="${view}"]`); await click('#clearFilters');
        await evaluate('document.getElementById("search").scrollIntoView({block:"center"});document.getElementById("search").focus()');
        await frame(); const before=await controlGeometry();
        for (const query of ['012','no-such-probe','']) {
          await input('search',query); await frame();
          assert.deepEqual(await controlGeometry(),before,`${width}/${view}/${query}`);
          assert.equal(await evaluate('document.activeElement.id'),'search');
        }
        for (const ageState of [true,false]) {
          stale=ageState; await evaluate('fetchStatus()'); await frame();
          assert.deepEqual(await controlGeometry(),before,`${width}/${view}/poll`);
        }
      }
    }
    await evaluate('scrollTo(0,document.getElementById("results").offsetTop+200)'); await frame();
    const scrollBefore=await evaluate('scrollY');
    await evaluate('fetchStatus()'); await frame();
    assert.equal(await evaluate('scrollY'),scrollBefore);
  });
  await check('map open on mobile reload; explicit keyboard toggle survives polls and locales', async () => {
    await cdp('Page.reload'); await waitFor('document.querySelectorAll("#fleetMap button").length===100');
    assert.equal(await evaluate('document.getElementById("fleetContent").hidden'),false);
    await evaluate('document.getElementById("fleetToggle").focus()');
    await cdp('Input.dispatchKeyEvent',{type:'keyDown',key:' ',code:'Space',text:' ',windowsVirtualKeyCode:32});
    await cdp('Input.dispatchKeyEvent',{type:'keyUp',key:' ',code:'Space',windowsVirtualKeyCode:32});
    assert.equal(await evaluate('document.getElementById("fleetContent").hidden'),true);
    await evaluate('fetchStatus()'); await switchLanguage('de');
    assert.equal(await evaluate('document.getElementById("fleetToggle").textContent'),localeCatalogs.de['fleet.show']);
    assert.equal(await evaluate('document.getElementById("fleetToggle").getAttribute("aria-expanded")'),'false');
    await click('#fleetToggle');
    assert.equal(await evaluate('document.getElementById("fleetContent").hidden'),false);
    assert.equal(await evaluate('document.getElementById("fleetToggle").textContent'),localeCatalogs.de['fleet.hide']);
  });
  await check('vertical CPU/RAM/Swap, one RAM value, visible zero and missing Swap in every view', async () => {
    await switchLanguage('en'); await click('#clearFilters');
    const old=observations[2].normalized.swap_used_mib;
    observations[2].normalized.swap_used_mib=null;
    await evaluate('fetchStatus()');
    for (const width of [1440,390,320]) {
      await cdp('Emulation.setDeviceMetricsOverride',{width,height:900,deviceScaleFactor:1,mobile:false});
      for (const view of ['cards','compact','table']) {
        await click(`[data-view="${view}"]`);
        if (view!=='table') {
          const layout=await evaluate(`(()=>{const c=document.querySelector('[data-id="probe:0"]');return [...c.querySelector('.resources').children].map(e=>({key:e.dataset.metric,y:e.getBoundingClientRect().y,text:e.innerText,visible:e.getBoundingClientRect().height>0}))})()`);
          assert.deepEqual(layout.map(x=>x.key),['cpu','ram','swap']);
          assert(layout[0].y<layout[1].y&&layout[1].y<layout[2].y);
          assert(layout.every(x=>x.visible)); assert.match(layout[2].text,/0 MiB/);
          assert.equal(await evaluate(`document.querySelector('[data-id="probe:2"] [data-metric="swap"] .number').textContent`),'—');
          assert.equal(await evaluate(`document.querySelector('[data-id="probe:0"]').innerText.split('RAM').length-1`),1);
        } else {
          assert.equal(await evaluate(`document.querySelector('[data-id="probe:0"] .swap-value').textContent`),'0 MiB');
          assert.equal(await evaluate(`document.querySelector('[data-id="probe:2"] .swap-value').textContent`),'—');
          assert.equal(await evaluate(`document.querySelector('[data-id="probe:0"] .swap-value').getBoundingClientRect().height>0`),true);
        }
        assert.equal(await evaluate('document.documentElement.scrollWidth<=innerWidth'),true,`${width}/${view}`);
        assert.equal(await evaluate('document.getElementById("results").scrollWidth<=document.getElementById("results").clientWidth'),true,`${width}/${view} result overflow`);
      }
    }
    observations[2].normalized.swap_used_mib=old; await evaluate('fetchStatus()');
  });
  await check('200% reflow equivalent and enlarged text keep controls and Swap in bounds', async () => {
    await cdp('Emulation.setDeviceMetricsOverride',{width:640,height:450,deviceScaleFactor:2,mobile:false});
    await evaluate('document.documentElement.style.fontSize="28px"');
    for (const loc of ['ru','de','ja']) {
      await switchLanguage(loc);
      for (const view of ['cards','compact','table']) {
        await click(`[data-view="${view}"]`); await frame();
        assert.equal(await evaluate('document.documentElement.scrollWidth<=innerWidth'),true,`${loc}/${view}`);
        assert.equal(await evaluate('document.getElementById("results").scrollWidth<=document.getElementById("results").clientWidth'),true,`${loc}/${view} zoom results`);
      }
    }
    await evaluate('document.documentElement.style.fontSize=""');
  });

  // Visual artifacts and screenshots for selected locales (ru, de, ja)
  await check('screenshots captured for selected locales (ru, de, ja)', async () => {
    await cdp('Emulation.setDeviceMetricsOverride', {width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false});
    await click('[data-view="compact"]');
    for (const loc of ['ru', 'de', 'ja']) {
      await switchLanguage(loc);
      const shot = await cdp('Page.captureScreenshot', {format: 'png'});
      await fs.writeFile(path.join(artifacts, `dashboard-${loc}.png`), Buffer.from(shot.data, 'base64'));
    }
  });

  assert.deepEqual(runtimeErrors,[]);
  assert(requests.every(r=>!r.includes('typesafe')));
  await fs.writeFile(path.join(artifacts,'browser-results.json'),JSON.stringify({checks,requests:requests.length,runtimeErrors,scope:'Chromium on loopback fixtures; no live infrastructure'},null,2));
  console.log(`PASS ${checks.length} browser scenarios; artifacts ${artifacts}`);
} finally {
  socket?.close();
  if (browser.exitCode === null && browser.signalCode === null) {
    const exited = once(browser,'exit').catch(()=>{}); browser.kill('SIGTERM'); await exited;
  }
  server.closeAllConnections();
  await new Promise(resolve=>server.close(resolve));
  if(artifacts!==scratch)await fs.rm(scratch,{recursive:true,force:true,maxRetries:5,retryDelay:100});
}
