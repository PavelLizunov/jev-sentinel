// Explicitly opt-in real-daemon acceptance. Makes paid Jev calls and creates one
// temporary category. Use only on an owned, unpublished recovery daemon; restart
// that daemon afterwards to discard the category (no category-delete API exists).
// LIVE_URL=http://127.0.0.1:8088 LIVE_MUTATIONS=approved CHROME_BIN=... node ...
// A real verification key is provided on stdin, never through CLI args or logs.
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {createHash} from 'node:crypto';

const origin = process.env.LIVE_URL;
assert.equal(process.env.LIVE_MUTATIONS, 'approved', 'Live operations require explicit opt-in');
assert(origin && process.env.CHROME_BIN && process.env.ARTIFACT_DIR);
const out = process.env.ARTIFACT_DIR;
await fs.mkdir(out, {recursive: true});
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const key = Buffer.concat(chunks).toString().trim();
assert(key.length > 0, 'Provide the approved key on stdin');
const repo = path.resolve(import.meta.dirname, '..');
const yaml = await fs.readFile(path.join(repo, 'sentinel.homelab-full.yaml'), 'utf8');
const expected = [...yaml.matchAll(/^  - type: ([a-z_]+)\n    name: "([^"]+)"/gm)].map(m => `${m[1]}:${m[2]}`).sort();
assert.equal(expected.length, 29);
const json = async route => {
  const r = await fetch(origin + route, {signal: AbortSignal.timeout(5000)});
  assert.equal(r.status, 200, route);
  return r.json();
};
const verifyInventory = s => {
  assert.equal(s.schema_version, 2);
  assert.deepEqual(s.targets.map(t => t.id).sort(), expected);
};
const first = await json('/api/status');
verifyInventory(first);
const initialCategories = await json('/api/categories');
assert(!initialCategories.some(c => c.id === 'recovery_verification'), 'Use a fresh recovery daemon');
const checks = [], errors = [], posts = [], hashes = {};
const hash = v => createHash('sha256').update(v).digest('hex');
for (const file of ['dashboard.css', 'dashboard.js', 'locales/ru.json']) {
  const r = await fetch(origin + '/' + file);
  assert.equal(r.status, 200);
  const actual = Buffer.from(await r.arrayBuffer());
  hashes[file] = hash(actual);
  assert.equal(hashes[file], hash(await fs.readFile(path.join(repo, 'src/web', file))));
}
checks.push('real 29-probe identity set and exact embedded UI assets');
const scratch = await fs.mkdtemp(path.join(os.tmpdir(), 'sentinel-live-browser-'));
const browser = spawn(process.env.CHROME_BIN, ['--headless=new', '--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--disable-sync', '--no-first-run', '--no-default-browser-check', '--remote-debugging-port=0', `--user-data-dir=${scratch}`, 'about:blank'], {stdio:['ignore','ignore','pipe'], env:{...process.env, HOME:scratch, XDG_CONFIG_HOME:scratch, XDG_CACHE_HOME:scratch}});
let socket, stderr = '';
browser.stderr.on('data', c => { stderr += c; });
try {
  let port;
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    try { port = (await fs.readFile(path.join(scratch, 'DevToolsActivePort'), 'utf8')).split('\n')[0]; break; } catch {}
    if (browser.exitCode !== null) throw new Error('Chromium exited during startup');
    await new Promise(r => setTimeout(r, 50));
  }
  assert(port, 'Chromium debugging port unavailable');
  const page = await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, {method:'PUT'}).then(r => r.json());
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await once(socket, 'open');
  let seq = 0;
  const pending = new Map();
  socket.addEventListener('message', ({data}) => {
    const m = JSON.parse(data);
    if (m.id) {
      const p = pending.get(m.id); pending.delete(m.id);
      m.error ? p.reject(new Error('CDP operation failed')) : p.resolve(m.result);
    } else if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.text);
    else if (m.method === 'Network.responseReceived' && ['/api/categories','/api/key','/api/categorize'].some(p => m.params.response.url.endsWith(p))) posts.push({path:new URL(m.params.response.url).pathname, status:m.params.response.status});
  });
  const cdp = (method, params={}) => new Promise((resolve,reject) => { const id=++seq;pending.set(id,{resolve,reject});socket.send(JSON.stringify({id,method,params})); });
  const ev = async expression => {
    const r = await cdp('Runtime.evaluate', {expression,awaitPromise:true,returnByValue:true});
    if (r.exceptionDetails) throw new Error('Browser evaluation failed; details omitted to protect key');
    return r.result.value;
  };
  const wait = async (expression, timeout=20000) => {
    const end = Date.now()+timeout;
    while (Date.now()<end) { if (await ev(expression)) return; await new Promise(r=>setTimeout(r,100)); }
    throw new Error('Browser condition timed out');
  };
  const click = id => ev(`document.getElementById(${JSON.stringify(id)}).click()`);
  const input = (id,value) => ev(`document.getElementById(${JSON.stringify(id)}).value=${JSON.stringify(value)};document.getElementById(${JSON.stringify(id)}).dispatchEvent(new Event('input',{bubbles:true}))`);
  const capture = async name => fs.writeFile(path.join(out,name+'.png'),Buffer.from((await cdp('Page.captureScreenshot',{format:'png'})).data,'base64'));
  await cdp('Runtime.enable');await cdp('Page.enable');await cdp('Network.enable');
  await cdp('Emulation.setDeviceMetricsOverride',{width:1440,height:1100,deviceScaleFactor:1,mobile:false});
  await cdp('Page.navigate',{url:origin});
  await wait('document.getElementById("total").textContent === "29"');
  await ev('I.setLocale("ru")');
  await wait('document.documentElement.lang === "ru"');
  assert.equal(await ev('["categorize","categoryButton","tokenButton"].every(id=>!document.getElementById(id).disabled)'),true);
  assert.equal(await ev('document.querySelector(".preview-banner") === null'),true);
  checks.push('real UI renders 29 targets; action buttons enabled; no snapshot wrapper');
  for (const mode of ['cards','compact','table']) {
    await ev(`setMode(${JSON.stringify(mode)})`);await click('clearFilters');
    assert.equal(await ev(mode==='table'?'document.querySelectorAll("#tableBody tr").length':'document.querySelectorAll("#grid article").length'),29);
  }
  checks.push('all 29 names render in Cards, Compact and Table');
  await click('categoryButton');await wait('document.getElementById("categoryDialog").open');
  await input('categoryId','recovery_verification');
  await input('categoryLabel','Проверка восстановления (временная)');
  await input('categoryDescription','Temporary acceptance-only category; never prefer this over an existing operational category.');
  await click('saveCategory');await wait('!document.getElementById("categoryDialog").open');
  assert((await json('/api/categories')).some(c=>c.id==='recovery_verification'));
  assert.equal(await ev('Array.from(document.getElementById("category").options).some(o=>o.value==="recovery_verification")'),true);
  checks.push('Add category form creates real server state and visible filter option');
  await click('tokenButton');await wait('document.getElementById("tokenDialog").open');
  await input('keyInput',key);await click('saveKey');
  await wait('tokenMessageState?.key === "key.saved"');
  assert.equal((await json('/api/key-status')).has_custom_key,true);
  assert.equal(await ev('document.getElementById("keyInput").value === ""'),true);
  await click('resetKey');await wait('tokenMessageState?.key === "key.reset"');
  assert.equal((await json('/api/key-status')).has_custom_key,false);
  await ev('document.querySelector("[data-close=tokenDialog]").click()');
  checks.push('Jev key form verifies real key, saves masked override, clears input and restores server key');
  await click('categorize');await wait('operationMessageState?.key === "operation.categorized"');
  const categorized = await json('/api/status');verifyInventory(categorized);
  assert.equal(categorized.targets.filter(t=>t.category).length,29);
  checks.push('Categorize with Jev performs real inference and assigns all 29 targets');
  // Bounded live-cycle acceptance, not background-job polling. No test fixture injects observations.
  const beforeCycle = categorized.cycle_id;
  await wait(`data?.cycle_id > ${beforeCycle} && data?.advisor_state === "ready"`,90000);
  const second = await json('/api/status');verifyInventory(second);
  assert.equal(second.daemon_epoch,first.daemon_epoch);
  assert(second.cycle_id > beforeCycle);
  assert(second.categories.some(c=>c.id==='recovery_verification'));
  for (const t of second.targets) {
    const prev=categorized.targets.find(p=>p.id===t.id);
    assert(BigInt(t.sample_seq)>BigInt(prev.sample_seq));
    assert(Date.parse(t.measured_at)>Date.parse(prev.measured_at));
    assert.equal(t.category,prev.category);
    assert(t.history.length>=2);
  }
  checks.push('next real cycle advances every probe and preserves category definitions and assignments');
  for (const width of [1440,390]) {
    await cdp('Emulation.setDeviceMetricsOverride',{width,height:1100,deviceScaleFactor:1,mobile:width<700});
    await ev('setMode("compact");scrollTo(0,0)');await click('clearFilters');
    assert.equal(await ev('document.documentElement.scrollWidth <= innerWidth'),true);
    await capture(width===1440?'live-desktop':'live-mobile');
  }
  await input('search','mac-mini-m4');
  await ev('document.querySelector("#grid .target-name").click()');
  await wait('document.getElementById("detailDialog").open');
  await capture('live-mac-details');
  checks.push('real macOS details open; desktop/mobile no horizontal overflow');
  assert.equal(errors.length,0);
  assert(posts.filter(p=>p.path==='/api/key').every(p=>p.status===200));
  const result={origin,checks,posts,hashes,errors,epoch:first.daemon_epoch,cycles:[first.cycle_id,second.cycle_id],probeIds:expected,category_cleanup:'requires owned recovery-daemon restart before publishing',states:second.targets.reduce((o,t)=>(o[t.state]=(o[t.state]||0)+1,o),{}),time:new Date().toISOString()};
  await fs.writeFile(path.join(out,'live-acceptance.json'),JSON.stringify(result,null,2)+'\n');
  console.log(JSON.stringify({passed:checks.length,checks,posts,cycles:result.cycles,states:result.states},null,2));
} finally {
  socket?.close();
  if(browser.exitCode===null&&browser.signalCode===null){const done=once(browser,'exit');browser.kill('SIGTERM');await done;}
  await fs.rm(scratch,{recursive:true,force:true,maxRetries:5,retryDelay:100});
}
