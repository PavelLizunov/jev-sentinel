'use strict';
const I = globalThis.SentinelI18n;
const $ = id => document.getElementById(id);
const numeric = n => typeof n === 'number' && Number.isFinite(n);
const fmt = (n, unit = '', digits = 0) => {
  if (!numeric(n)) return '—';
  const key = {'%':'unit.percent', 'ms':'unit.ms', ' MiB':'unit.mib'}[unit];
  return unit ? I.t('value.unit', {value: I.number(n, digits), unit: key ? I.t(key) : unit}) : I.number(n, digits);
};
const labels = {healthy: 'OK', degraded: 'Degraded', failed: 'Failed', unknown: 'Unknown', stale: 'Stale'};
const symbols = {healthy: '·', degraded: '!', failed: '×', unknown: '?', stale: '~'};
let data = null, receivedAt = 0, connected = false, timer = null, fetching = false, selectedId = null;
let mode = 'compact', sortKey = 'name', sortDescending = false;
try { mode = localStorage.getItem('sentinel.view') || mode; } catch {}
if (!['cards', 'compact', 'table'].includes(mode)) mode = 'compact';
const items = new Map(), cells = new Map(), headings = new Map();
let lastEpoch = null, requestController = null;

let connectionErrorState = null;
let tokenMessageState = null;
let keyStatusState = null;
let categoryMessageState = null;
let operationMessageState = null;

function el(tag, className, textContent) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (textContent !== undefined) node.textContent = textContent;
  return node;
}
function text(node, value) { if (node && node.textContent !== String(value)) node.textContent = value; }

function stateOf(target) {
  if (!numeric(target.age_ms)) return 'unknown';
  return target.age_ms + Math.max(0, performance.now() - receivedAt) > data.stale_after_ms ? 'stale' : target.state;
}
function ageOf(target) {
  return numeric(target.age_ms) ? target.age_ms + Math.max(0, performance.now() - receivedAt) : null;
}
function stateName(state) {
  const key = `state.${state}`;
  return I.hasKey(key) ? I.t(key) : I.t('state.unknown');
}
function stateLabel(node, state) {
  node.dataset.state = state;
  text(node, `${symbols[state] || '?'} ${stateName(state)}`);
}
function healthLabel(health) {
  if (!health) return I.t('health.unknown');
  const key = `health.${String(health).toLowerCase()}`;
  return I.hasKey(key) ? I.t(key) : String(health);
}
function metricName(key) {
  if (key === 'latency_ms' || key === 'latency') return I.t('metric.latency');
  if (key === 'ram_pct' || key === 'ram') return I.t('metric.ram');
  if (key === 'swap_mib' || key === 'swap') return I.t('metric.swap');
  if (key === 'cpu_pct' || key === 'cpu') return I.t('metric.cpu');
  return key;
}
function unitName(key) {
  if (key === 'latency_ms') return I.t('unit.ms');
  if (key === 'ram_pct') return I.t('unit.percent');
  if (key === 'swap_mib') return I.t('unit.mib');
  return '';
}
function resolveError(code, message) {
  if (typeof code === 'string' && code) {
    const key = `error.${code}`;
    if (I.hasKey(key)) return I.t(key);
  }
  return typeof message === 'string' ? message : '';
}
function errorStateFrom(err) {
  if (err && typeof err.error_code === 'string' && err.error_code) {
    const key = `error.${err.error_code}`;
    if (I.hasKey(key)) return { key };
  }
  return { key: 'error.request_failed', params: {detail: err?.message || ''} };
}
function categoryLabel(id) {
  if (!id) return I.t('category.uncategorized');
  const c = data?.categories?.find(cat => cat.id === id);
  if (!c) return I.t('category.uncategorized');
  if (typeof c.label_key === 'string' && c.label_key.startsWith('category.') && I.hasKey(c.label_key)) {
    return I.t(c.label_key);
  }
  return typeof c.label === 'string' ? c.label : I.t('category.uncategorized');
}
function memoryBasisText(m) {
  if (!m) return I.t('memory.unavailable');
  if (typeof m.memory_basis_code === 'string' && m.memory_basis_code) {
    const key = `memory.${m.memory_basis_code}`;
    if (I.hasKey(key)) return I.t(key);
  }
  if (typeof m.memory_basis === 'string' && m.memory_basis) {
    return m.memory_basis;
  }
  return I.t('memory.unavailable');
}

function messageText(state) {
  if (!state) return '';
  const params = Object.fromEntries(Object.entries(state.params || {}).map(([key, value]) =>
    [key, value && typeof value === 'object' && value.key ? messageText(value) : value]));
  return state.key ? I.t(state.key, params) : (state.text || '');
}
function renderMsgNode(node, msgState) {
  if (!node) return;
  if (!msgState) {
    text(node, '');
    return;
  }
  if (msgState.key) {
    text(node, messageText(msgState));
  } else if (msgState.text !== undefined) {
    text(node, msgState.text);
  }
}
function refreshMessages() {
  renderMsgNode($('connectionError'), connectionErrorState);
  renderMsgNode($('tokenMessage'), tokenMessageState);
  renderMsgNode($('keyStatus'), keyStatusState);
  renderMsgNode($('categoryMessage'), categoryMessageState);
  renderMsgNode($('operationMessage'), operationMessageState);
}

function saveFilters() {
  savedFilters = {query: $('search').value, category: $('category').value, issues: $('issues').checked, group: $('group').value};
  try { localStorage.setItem('sentinel.filters', JSON.stringify(savedFilters)); } catch {}
}
let savedFilters = {};
try {
  const stored = JSON.parse(localStorage.getItem('sentinel.filters') || '{}');
  if (stored && typeof stored === 'object' && !Array.isArray(stored)) savedFilters = stored;
} catch {}

function spark() {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg'); svg.setAttribute('viewBox', '0 0 100 24');
  svg.classList.add('spark'); svg.setAttribute('role', 'img');
  const title = document.createElementNS(ns, 'title');
  const guide = document.createElementNS(ns, 'path'); guide.classList.add('guide'); guide.setAttribute('d', 'M0,23 L100,23');
  const path = document.createElementNS(ns, 'path');
  const dot = document.createElementNS(ns, 'circle'); dot.setAttribute('r', '1.7');
  svg.append(title, guide, path, dot); return {svg, title, path, dot, signature: ''};
}
function updateSpark(s, history, key, name) {
  const points = (history || []).slice(-20);
  const signature = `${I.locale}:${JSON.stringify([key, points])}`;
  if (signature === s.signature) return; s.signature = signature;
  const valid = points.filter(p => numeric(p.t_ms) && numeric(p[key]));
  const lo = 0, hi = key === 'ram_pct' ? 100 : Math.max(key === 'latency_ms' ? 100 : 1, ...valid.map(p => p[key]));
  const t0 = points[0]?.t_ms || 0, t1 = points.at(-1)?.t_ms || t0;
  let d = '', drawing = false, last = null;
  for (const p of points) {
    if (!numeric(p.t_ms) || !numeric(p[key])) { drawing = false; continue; }
    if (p.break_before) drawing = false;
    const x = t1 === t0 ? 50 : 100 * (p.t_ms - t0) / (t1 - t0);
    const y = 23 - 22 * Math.max(0, Math.min(1, (p[key] - lo) / (hi - lo)));
    d += `${drawing ? 'L' : 'M'}${x.toFixed(1)},${y.toFixed(1)} `;
    drawing = true; last = {x, y};
  }
  s.path.setAttribute('d', d);
  s.dot.style.display = last ? '' : 'none';
  if (last) { s.dot.setAttribute('cx', last.x); s.dot.setAttribute('cy', last.y); }
  const displayName = name || metricName(key);
  const unit = unitName(key);
  const span = Math.round((t1 - t0) / 1000);
  const description = valid.length
    ? I.t('history.description', {name: displayName, count: valid.length, span: I.number(span), max: I.number(Math.round(hi)), unit})
    : I.t('history.empty', {name: displayName});
  s.title.textContent = description; s.svg.setAttribute('aria-label', description);
}
function historyRow(metricKey) {
  const root = el('div', 'history-row'), label = el('span', '', metricName(metricKey)), chart = spark(), value = el('span', 'number');
  root.append(label, chart.svg, value); return {root, label, chart, value, metricKey};
}
function meter(metricKey) {
  const root = el('div'), head = el('div', 'metric-heading'), label = el('span', '', metricName(metricKey)), value = el('span', 'number'), track = el('div', 'track'), fill = el('span', 'fill');
  head.append(label, value); track.append(fill); root.append(head, track); return {root, label, value, fill, metricKey};
}
function updateMeter(m, value) {
  text(m.value, fmt(value, '%'));
  m.fill.style.width = numeric(value) ? `${Math.max(0, Math.min(100, value))}%` : '0%';
  m.fill.dataset.level = value >= 90 ? 'critical' : value >= 75 ? 'high' : 'normal';
}
function makeItem(target) {
  const root = el(mode === 'table' ? 'tr' : 'article', mode === 'table' ? '' : 'target');
  root.dataset.id = target.id;
  const name = el('button', 'target-name', target.name); name.type = 'button';
  name.addEventListener('click', () => openDetails(target.id));
  const state = el('span', 'state'), age = el('span', 'age'), meta = el('div', 'meta');
  const latency = historyRow('latency_ms'), ramHistory = historyRow('ram_pct'), swapHistory = historyRow('swap_mib');
  const cpu = meter('cpu_pct'), ram = meter('ram_pct'), error = el('p', 'error-text expanded'), note = el('p', 'history-note');
  if (mode === 'table') {
    swapHistory.value.classList.add('swap-value');
    const values = [name, state, cpu.root, ram.root, swapHistory.value, latency.root, age];
    values.forEach((value, i) => { const td = el('td', [2,3,6].includes(i) ? 'desktop' : ''); td.append(value); root.append(td); });
    latency.root.className = 'table-spark'; latency.root.firstChild.remove();
  } else {
    const head = el('div', 'target-head'); head.append(name, state);
    const resources = el('div', 'resources');
    cpu.root.dataset.metric = 'cpu'; ramHistory.root.dataset.metric = 'ram'; swapHistory.root.dataset.metric = 'swap';
    resources.append(cpu.root, ramHistory.root, swapHistory.root);
    root.append(head, meta, resources, latency.root, note, age, error);
  }
  return {root, name, state, age, meta, latency, ramHistory, swapHistory, cpu, ram, error, note};
}
function updateItem(item, target) {
  const state = stateOf(target), m = target.normalized || {};
  text(item.name, target.name);
  item.name.setAttribute('aria-label', I.t('details.open', {name: target.name}));
  stateLabel(item.state, state);
  const age = ageOf(target);
  text(item.age, numeric(age) ? I.t('time.ago', {count: Math.floor(age / 1000)}) : I.t('time.no_observation'));
  text(item.meta, `${target.target_type} · ${categoryLabel(target.category)}`);
  text(item.cpu.label, I.t('metric.cpu'));
  text(item.ram.label, I.t('metric.ram'));
  updateMeter(item.cpu, m.cpu_pct);
  updateMeter(item.ram, m.ram_used_pct);
  if (mode !== 'table' && item.latency.label) text(item.latency.label, I.t('metric.latency'));
  text(item.ramHistory.label, I.t('metric.ram'));
  text(item.swapHistory.label, I.t('metric.swap'));
  updateSpark(item.latency.chart, target.history, 'latency_ms', I.t('metric.latency'));
  updateSpark(item.ramHistory.chart, target.history, 'ram_pct', I.t('metric.ram'));
  updateSpark(item.swapHistory.chart, target.history, 'swap_mib', I.t('metric.swap'));
  text(item.latency.value, fmt(target.latency_ms, 'ms', 1));
  text(item.ramHistory.value, fmt(m.ram_used_pct, '%'));
  text(item.swapHistory.value, fmt(m.swap_used_mib, ' MiB'));
  const h = target.history || [], span = h.length > 1 ? Math.round((h.at(-1).t_ms - h[0].t_ms) / 1000) : 0;
  text(item.note, I.t('history.note', {count: h.length, span: I.number(span)}));
  const targetErr = resolveError(target.error_code, target.error_message);
  text(item.error, targetErr);
  item.error.hidden = !targetErr;
}
function reconcile(parent, nodes) {
  nodes.forEach((node, i) => { if (parent.children[i] !== node) parent.insertBefore(node, parent.children[i] || null); });
  while (parent.children.length > nodes.length) parent.lastElementChild.remove();
}
function metricForSort(t, key) {
  if (key === 'name') return t.name.toLowerCase();
  if (key === 'state') return ['failed', 'degraded', 'unknown', 'stale', 'healthy'].indexOf(stateOf(t));
  if (key === 'cpu') return t.normalized?.cpu_pct;
  if (key === 'ram') return t.normalized?.ram_used_pct;
  if (key === 'age') return ageOf(t);
  return t.latency_ms;
}
function render() {
  const active = [];
  if ($('category').value) active.push(`${I.t('filter.category')}: ${categoryLabel($('category').value)}`);
  if ($('issues').checked) active.push(I.t('filter.issues'));
  if ($('group').value === 'category') active.push(`${I.t('filter.group')}: ${I.t('filter.group_category')}`);
  text($('activeFilters'), active.length ? I.t('filter.active', {filters: active.join(' · ')}) : I.t('filter.no_restrictions'));
  text($('fleetToggle'), I.t($('fleetContent').hidden ? 'fleet.show' : 'fleet.hide'));
  text($('connection'), I.t(connected ? 'connection.connected' : connectionErrorState ? 'connection.disconnected' : 'connection.connecting'));
  refreshMessages();
  if (!data) {
    text($('telemetry'), I.t('telemetry.waiting'));
    text($('advisor'), I.t('advisor.unavailable'));
    text($('health'), I.t('health.unknown'));
    text($('advice'), I.t('advice.none_current'));
    text($('summaryProbes'), I.t('summary.probes', {count: 0}));
    text($('resultCount'), I.t('empty.waiting'));
    text($('empty'), I.t(connectionErrorState ? 'empty.retrying' : 'empty.loading'));
    return;
  }
  const targets = data.targets;
  const counts = {healthy: 0, degraded: 0, failed: 0, unknown: 0, stale: 0};
  targets.forEach(t => { const state = stateOf(t); counts[state] = (counts[state] || 0) + 1; });
  text($('total'), I.number(targets.length));
  for (const key of Object.keys(counts)) text($(`count-${key}`), I.number(counts[key]));
  const summaryProbes = $('summaryProbes');
  if (summaryProbes) {
    text(summaryProbes, I.t('summary.probes', {count: targets.length}));
  } else {
    const totalEl = $('total');
    if (totalEl?.nextSibling && totalEl.nextSibling.nodeType === 3) {
      totalEl.nextSibling.textContent = ' ' + I.t('summary.probes', {count: targets.length});
    }
  }
  const telemetryText = data.phase === 'starting'
    ? I.t('telemetry.first_cycle')
    : (counts.stale ? I.t('telemetry.stale', {count: counts.stale}) : I.t('telemetry.fresh'));
  text($('telemetry'), telemetryText);
  const advisorKey = `advisor.${data.advisor_state}`;
  text($('advisor'), I.hasKey(advisorKey) ? I.t(advisorKey) : `Jev: ${data.advisor_state.replaceAll('_', ' ')}`);
  const ready = data.advisor_state === 'ready' && counts.stale === 0 && data.phase !== 'starting';
  text($('risk'), ready && numeric(data.risk_score) ? I.t('advice.risk_value', {value: I.number(data.risk_score, 2), max: I.number(1, 2)}) : '—');
  text($('health'), ready ? healthLabel(data.system_health) : I.t('health.unknown'));
  const actionKey = data.suggested_action ? `action.${data.suggested_action}` : '';
  const actionText = ready ? (I.hasKey(actionKey) ? I.t(actionKey) : (data.suggested_action || '—')) : I.t('advice.none_current');
  text($('advice'), actionText);
  const attention = counts.failed + counts.degraded + counts.unknown + counts.stale;
  text($('attention'), attention ? I.t('attention.issues', {count: attention}) : I.t('attention.none'));
  $('attention').hidden = !targets.length;
  const advisorErr = resolveError(data.error_code, data.error_message);
  text($('advisorError'), advisorErr);
  $('advisorError').hidden = !advisorErr;

  const allIds = new Set(targets.map(t => t.id));
  if (selectedId !== null && !allIds.has(selectedId)) {
    selectedId = null; $('detailDialog').close();
  }
  for (const [id, item] of items) if (!allIds.has(id)) { item.root.remove(); items.delete(id); }
  for (const [id, cell] of cells) if (!allIds.has(id)) { cell.remove(); cells.delete(id); }
  const mapNodes = targets.map((t, i) => {
    let cell = cells.get(t.id);
    if (!cell) { cell = el('button', 'fleet-cell'); cell.type = 'button'; cell.addEventListener('click', () => openDetails(t.id)); cells.set(t.id, cell); }
    const state = stateOf(t); text(cell, `${symbols[state] || '?'}${i + 1}`); cell.dataset.state = state;
    cell.title = I.t('fleet.cell', {name: t.name, state: stateName(state)});
    cell.setAttribute('aria-label', cell.title);
    cell.setAttribute('aria-pressed', String(selectedId === t.id)); return cell;
  });
  reconcile($('fleetMap'), mapNodes);
  const query = $('search').value.toLowerCase().trim(), category = $('category').value;
  const matches = targets.filter(t => !query ||
    `${t.name} ${t.id} ${t.target_type} ${t.category || ''} ${categoryLabel(t.category)}`.toLowerCase().includes(query));
  const filtered = matches.filter(t => (!$('issues').checked || stateOf(t) !== 'healthy') && (!category || t.category === category));
  const hiddenCount = query ? matches.length - filtered.length : 0;
  $('filterFeedback').hidden = hiddenCount === 0;
  text($('hiddenMatches'), hiddenCount ? I.t('filter.hidden_matches', {count: I.number(hiddenCount)}) : '');
  filtered.sort((a, b) => {
    const x = metricForSort(a, sortKey), y = metricForSort(b, sortKey);
    if (x == null) return y == null ? a.id.localeCompare(b.id) : 1;
    if (y == null) return -1;
    const order = typeof x === 'string' ? x.localeCompare(y) : x - y;
    return (sortDescending ? -order : order) || a.id.localeCompare(b.id);
  });
  text($('resultCount'), I.t('filter.results', {count: filtered.length, total: targets.length}));
  const nodes = [];
  const groups = $('group').value === 'category' && mode !== 'table' ? [...new Set(filtered.map(t => t.category || ''))].sort() : [null];
  for (const group of groups) {
    if (group !== null) {
      let heading = headings.get(group);
      if (!heading) { heading = el('h2', 'group-heading'); headings.set(group, heading); }
      text(heading, categoryLabel(group)); nodes.push(heading);
    }
    for (const target of filtered.filter(t => group === null || (t.category || '') === group)) {
      let item = items.get(target.id); if (!item) { item = makeItem(target); items.set(target.id, item); }
      updateItem(item, target); nodes.push(item.root);
    }
  }
  reconcile(mode === 'table' ? $('tableBody') : $('grid'), nodes);
  $('empty').hidden = filtered.length > 0;
  text($('empty'), data.phase === 'starting' ? I.t('empty.first_cycle') : (!targets.length ? I.t('empty.no_probes') : I.t('empty.no_matches')));
  if ($('detailDialog').open) updateDetails();
}
function updateCategories() {
  const select = $('category');
  if (!select) return;
  const previous = select.value || savedFilters.category || '';
  const cats = data?.categories || [];
  const signature = `${I.locale}:${JSON.stringify(cats)}`;
  if (select.dataset.signature === signature) return;
  select.dataset.signature = signature;
  const options = [
    new Option(I.t('filter.all_categories'), ''),
    ...cats.map(c => new Option(categoryLabel(c.id), c.id))
  ];
  select.replaceChildren(...options);
  select.value = options.some(o => o.value === previous) ? previous : '';
}
function setMode(next) {
  if (!['cards', 'compact', 'table'].includes(next)) return;
  if (mode !== next) { items.clear(); headings.clear(); $('grid').replaceChildren(); $('tableBody').replaceChildren(); }
  mode = next; try { localStorage.setItem('sentinel.view', mode); } catch {}
  $('grid').className = `grid ${mode}`; $('grid').hidden = mode === 'table'; $('tableWrap').hidden = mode !== 'table';
  document.querySelectorAll('[data-view]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.view === mode)));
  render();
}
async function fetchStatus() {
  if (fetching) return;
  clearTimeout(timer); fetching = true;
  requestController = new AbortController();
  const deadline = setTimeout(() => requestController.abort(), 4000);
  try {
    const response = await fetch('/api/status', {signal: requestController.signal, cache: 'no-store'});
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const next = await response.json();
    const object = v => v !== null && typeof v === 'object' && !Array.isArray(v);
    if (!object(next) || next.schema_version !== 2 || typeof next.daemon_epoch !== 'string' ||
        typeof next.phase !== 'string' || typeof next.system_health !== 'string' ||
        !['ready', 'evaluating', 'unavailable', 'invalid_response'].includes(next.advisor_state) ||
        !numeric(next.stale_after_ms) || next.stale_after_ms <= 0 ||
        !Array.isArray(next.categories) || !next.categories.every(c => object(c) && typeof c.id === 'string' && typeof c.label === 'string') ||
        !Array.isArray(next.targets) || next.targets.length > 1024 ||
        !next.targets.every(t => object(t) && typeof t.id === 'string' && typeof t.name === 'string' &&
          typeof t.target_type === 'string' && Object.hasOwn(labels, t.state) &&
          (t.age_ms === null || (numeric(t.age_ms) && t.age_ms >= 0)) &&
          object(t.normalized) && Array.isArray(t.history) && t.history.length <= 20 &&
          t.history.every(p => object(p) && numeric(p.t_ms))) ||
        new Set(next.targets.map(t => t.id)).size !== next.targets.length) {
      throw Object.assign(new Error('Unsupported status schema'), {error_code: 'unsupported_schema'});
    }
    if (lastEpoch !== null && next.daemon_epoch !== lastEpoch) {
      items.clear(); cells.clear(); $('grid').replaceChildren(); $('tableBody').replaceChildren(); $('fleetMap').replaceChildren();
      selectedId = null; if ($('detailDialog').open) $('detailDialog').close();
    }
    lastEpoch = next.daemon_epoch; data = next; receivedAt = performance.now(); connected = true;
    updateCategories(); render();
    text($('connection'), I.t('connection.connected'));
    $('connectionError').hidden = true;
    connectionErrorState = null;
  } catch (error) {
    connected = false;
    text($('connection'), I.t('connection.disconnected'));
    $('connectionError').hidden = false;
    connectionErrorState = data
      ? { key: 'connection.lost' }
      : { key: 'connection.load_failed', params: { error: errorStateFrom(error) } };
    renderMsgNode($('connectionError'), connectionErrorState);
    if (!data) text($('empty'), I.t('empty.retrying'));
  } finally {
    clearTimeout(deadline); fetching = false;
    timer = setTimeout(fetchStatus, document.hidden ? 15000 : 3000);
  }
}

const detailCharts = [historyRow('latency_ms'), historyRow('ram_pct'), historyRow('swap_mib')];
$('detailCharts').append(...detailCharts.map(c => c.root));
function openDetails(id) {
  selectedId = id; updateDetails(); if (!$('detailDialog').open) $('detailDialog').showModal(); render();
}
function updateDetails() {
  const t = data?.targets.find(t => t.id === selectedId);
  if (!t) {
    text($('detailTitle'), I.t('details.removed_title'));
    text($('detailMeta'), I.t('details.removed'));
    return;
  }
  text($('detailTitle'), t.name);
  const age = ageOf(t);
  const ageFormatted = numeric(age) ? I.t('time.ago', {count: Math.floor(age / 1000)}) : I.t('time.no_observation');
  const timeFormatted = I.dateTime(t.measured_at);
  text($('detailMeta'), I.t('details.meta', {
    state: stateName(stateOf(t)),
    type: t.target_type,
    age: ageFormatted,
    time: timeFormatted
  }));
  const detailErr = resolveError(t.error_code, t.error_message);
  text($('detailError'), detailErr);
  $('detailError').hidden = !detailErr;

  const m = t.normalized || {};
  text($('detailResources'), I.t('details.resources', {
    cpu: fmt(m.cpu_pct, '%'),
    ram: fmt(m.ram_used_pct, '%'),
    available: fmt(m.available_mib, ' MiB'),
    total: fmt(m.total_memory_mib, ' MiB'),
    gpu_vram: fmt(m.gpu_used_pct, '%'),
    gpu_use: fmt(m.gpu_util_pct, '%')
  }));
  text($('memoryBasis'), memoryBasisText(m));

  const detailConfigs = [
    {key: 'latency_ms', val: t.latency_ms, unit: 'ms', digits: 1, metric: 'latency'},
    {key: 'ram_pct', val: m.ram_used_pct, unit: '%', digits: 0, metric: 'ram'},
    {key: 'swap_mib', val: m.swap_used_mib, unit: ' MiB', digits: 0, metric: 'swap'}
  ];
  detailConfigs.forEach((d, i) => {
    const row = detailCharts[i];
    text(row.label, I.t(`metric.${d.metric}`));
    updateSpark(row.chart, t.history, d.key, I.t(`metric.${d.metric}`));
    text(row.value, fmt(d.val, d.unit, d.digits));
  });

  text($('detailRaw'), JSON.stringify(t.metrics, null, 2));
  text($('detailId'), I.t('details.identity', {
    id: t.id,
    sequence: t.sample_seq ?? '—'
  }));
}

async function api(path, body, method = 'POST') {
  const response = await fetch(path, {
    method,
    headers: {'Content-Type': 'application/json'},
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(18000)
  });
  const result = await response.json();
  if (!response.ok) {
    const err = new Error(result.error || `HTTP ${response.status}`);
    if (result.error_code) err.error_code = result.error_code;
    throw err;
  }
  return result;
}
async function loadKeyStatus() {
  keyStatusState = { key: 'key.loading' };
  renderMsgNode($('keyStatus'), keyStatusState);
  try {
    const r = await api('/api/key-status', undefined, 'GET');
    if (r.has_custom_key) {
      keyStatusState = { key: 'key.custom_status', params: { key: r.active_key_masked && r.active_key_masked !== 'none' ? r.active_key_masked : {key: 'key.none'} } };
    } else {
      keyStatusState = { key: 'key.server_status', params: { key: r.default_key_masked && r.default_key_masked !== 'none' ? r.default_key_masked : {key: 'key.none'} } };
    }
  } catch (e) {
    keyStatusState = errorStateFrom(e);
  }
  renderMsgNode($('keyStatus'), keyStatusState);
}
$('tokenButton').addEventListener('click', () => {
  $('tokenDialog').showModal();
  tokenMessageState = null;
  renderMsgNode($('tokenMessage'), null);
  loadKeyStatus();
});
$('tokenForm').addEventListener('submit', async e => {
  e.preventDefault();
  const button = $('saveKey');
  button.disabled = true;
  tokenMessageState = { key: 'key.verifying' };
  renderMsgNode($('tokenMessage'), tokenMessageState);
  try {
    await api('/api/key', {api_key: $('keyInput').value.trim()});
    $('keyInput').value = '';
    tokenMessageState = { key: 'key.saved' };
    renderMsgNode($('tokenMessage'), tokenMessageState);
    await loadKeyStatus();
  } catch (err) {
    tokenMessageState = errorStateFrom(err);
    renderMsgNode($('tokenMessage'), tokenMessageState);
  } finally {
    button.disabled = false;
  }
});
$('resetKey').addEventListener('click', async () => {
  $('resetKey').disabled = true;
  try {
    await api('/api/key', {api_key: null});
    tokenMessageState = { key: 'key.reset' };
    renderMsgNode($('tokenMessage'), tokenMessageState);
    await loadKeyStatus();
  } catch (e) {
    tokenMessageState = errorStateFrom(e);
    renderMsgNode($('tokenMessage'), tokenMessageState);
  } finally {
    $('resetKey').disabled = false;
  }
});
function renderCategoryList() {
  const list = $('categoryList');
  if (!list) return;
  list.replaceChildren();
  const cats = data?.categories || [];
  if (cats.length === 0) {
    const emptyLi = document.createElement('li');
    emptyLi.className = 'muted';
    emptyLi.textContent = '—';
    list.appendChild(emptyLi);
    return;
  }
  for (const cat of cats) {
    const li = document.createElement('li');
    li.className = 'category-item';

    const info = document.createElement('div');
    info.className = 'category-item-info';

    const idSpan = document.createElement('span');
    idSpan.className = 'category-item-id';
    idSpan.textContent = cat.id;

    const labelSpan = document.createElement('span');
    labelSpan.className = 'category-item-label';
    labelSpan.textContent = ` (${categoryLabel(cat.id)})`;

    info.appendChild(idSpan);
    info.appendChild(labelSpan);

    const delBtn = document.createElement('button');
    delBtn.type = 'button';
    delBtn.className = 'category-item-del';
    delBtn.textContent = I.t('button.delete');
    delBtn.addEventListener('click', async () => {
      delBtn.disabled = true;
      categoryMessageState = null;
      renderMsgNode($('categoryMessage'), null);
      try {
        await api('/api/categories', { id: cat.id, delete: true });
        await fetchStatus();
        renderCategoryList();
      } catch (err) {
        categoryMessageState = errorStateFrom(err);
        renderMsgNode($('categoryMessage'), categoryMessageState);
      } finally {
        delBtn.disabled = false;
      }
    });

    li.appendChild(info);
    li.appendChild(delBtn);
    list.appendChild(li);
  }
}

$('categoryButton').addEventListener('click', () => {
  categoryMessageState = null;
  renderMsgNode($('categoryMessage'), null);
  renderCategoryList();
  $('categoryDialog').showModal();
});
$('categoryForm').addEventListener('submit', async e => {
  e.preventDefault();
  $('saveCategory').disabled = true;
  try {
    await api('/api/categories', {
      id: $('categoryId').value.trim(),
      label: $('categoryLabel').value.trim(),
      description: $('categoryDescription').value.trim()
    });
    $('categoryDialog').close();
    $('categoryForm').reset();
    categoryMessageState = null;
    await fetchStatus();
  } catch (err) {
    categoryMessageState = errorStateFrom(err);
    renderMsgNode($('categoryMessage'), categoryMessageState);
  } finally {
    $('saveCategory').disabled = false;
  }
});
$('categorize').addEventListener('click', async () => {
  $('categorize').disabled = true;
  operationMessageState = { key: 'operation.categorizing' };
  renderMsgNode($('operationMessage'), operationMessageState);
  try {
    const r = await api('/api/categorize');
    operationMessageState = { key: 'operation.categorized', params: { count: r.categorized_count } };
    renderMsgNode($('operationMessage'), operationMessageState);
    await fetchStatus();
  } catch (e) {
    operationMessageState = errorStateFrom(e);
    renderMsgNode($('operationMessage'), operationMessageState);
  } finally {
    $('categorize').disabled = false;
  }
});
document.querySelectorAll('[data-close]').forEach(button => button.addEventListener('click', () => $(button.dataset.close).close()));
$('tokenDialog').addEventListener('close', () => { $('keyInput').value = ''; });
$('detailDialog').addEventListener('close', () => { selectedId = null; render(); });
document.querySelectorAll('[data-view]').forEach(button => button.addEventListener('click', () => setMode(button.dataset.view)));
$('search').value = typeof savedFilters.query === 'string' ? savedFilters.query : '';
$('issues').checked = savedFilters.issues === true; $('group').value = savedFilters.group === 'category' ? 'category' : 'none';
for (const id of ['search', 'category', 'issues', 'group']) $(id).addEventListener(id === 'search' ? 'input' : 'change', () => {
  saveFilters(); render();
});
$('sort').addEventListener('change', () => { sortKey = $('sort').value; sortDescending = ['cpu','ram','latency','age'].includes(sortKey); render(); });
document.querySelectorAll('[data-sort]').forEach(button => button.addEventListener('click', () => {
  const key = button.dataset.sort; sortDescending = sortKey === key ? !sortDescending : ['cpu','ram','latency','age'].includes(key); sortKey = key; $('sort').value = key; render();
}));
$('clearFilters').addEventListener('click', () => {
  $('search').value = ''; $('issues').checked = false; $('category').value = ''; $('group').value = 'none';
  saveFilters(); render();
});
$('showAllMatches').addEventListener('click', () => {
  $('issues').checked = false; $('category').value = '';
  saveFilters(); render(); $('search').focus({preventScroll: true});
});
$('fleetToggle').addEventListener('click', () => {
  const hidden = !$('fleetContent').hidden;
  $('fleetContent').hidden = hidden;
  $('fleetToggle').setAttribute('aria-expanded', String(!hidden));
  render();
});
$('demoButton')?.addEventListener('click', () => $('demoDialog').showModal());
document.addEventListener('visibilitychange', () => { if (!document.hidden) fetchStatus(); });
window.addEventListener('pagehide', () => { clearTimeout(timer); requestController?.abort(); });

async function boot() {
  await I.init();
  I.onChange(() => {
    I.apply();
    updateCategories();
    refreshMessages();
    render();
  });
  setMode(mode);
  await fetchStatus();
}
boot();
setInterval(() => { if (!document.hidden && data) render(); }, 1000);
