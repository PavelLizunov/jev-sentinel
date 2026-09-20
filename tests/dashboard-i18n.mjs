// tests/dashboard-i18n.mjs
// Native test suite for Jev Sentinel internationalization (i18n).
// Tests catalog parity, shapes, placeholders, CLDR plurals, invariants,
// technical semantics, fallbacks, negotiation, races, errors, XSS safety, and bounded loads.

import test, { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const localesDir = path.resolve(import.meta.dirname, '../src/web/locales');
const i18nJsPath = path.resolve(import.meta.dirname, '../src/web/i18n.js');
const i18nCode = fs.readFileSync(i18nJsPath, 'utf8');

const APPROVED_LOCALES = ['en', 'ru', 'de', 'fr', 'es', 'pt-BR', 'zh-CN', 'ja'];
const catalogs = Object.fromEntries(
  APPROVED_LOCALES.map(loc => [loc, JSON.parse(fs.readFileSync(path.join(localesDir, `${loc}.json`), 'utf8'))])
);
const enCatalog = catalogs.en;
const enKeys = Object.keys(enCatalog).sort();

function extractPlaceholders(str) {
  const matches = str.match(/\{([a-zA-Z0-9_]+)\}/g) || [];
  return [...new Set(matches.map(m => m.slice(1, -1)))].sort();
}

function createMinimalDom() {
  const elements = new Map();
  function makeElement(tag, attrs = {}) {
    return {
      tagName: tag.toUpperCase(),
      attributes: { ...attrs },
      children: [],
      textContent: '',
      hidden: false,
      value: '',
      options: [],
      listeners: {},
      getAttribute(name) { return this.attributes[name] ?? null; },
      setAttribute(name, val) {
        this.attributes[name] = String(val);
        if (name === 'hidden') this.hidden = true;
      },
      hasAttribute(name) { return name in this.attributes; },
      removeAttribute(name) {
        delete this.attributes[name];
        if (name === 'hidden') this.hidden = false;
      },
      appendChild(child) {
        this.children.push(child);
        if (this.tagName === 'SELECT') this.options.push(child);
      },
      addEventListener(event, fn) {
        (this.listeners[event] = this.listeners[event] || []).push(fn);
      },
      dispatchEvent(event) {
        (this.listeners[event.type] || []).forEach(fn => fn(event));
      }
    };
  }

  const select = makeElement('select', { id: 'languageSelect' });
  const note = makeElement('div', { id: 'translationNote' });
  const msg = makeElement('div', { id: 'localeMessage' });
  elements.set('languageSelect', select);
  elements.set('translationNote', note);
  elements.set('localeMessage', msg);

  const doc = {
    nodeType: 9,
    documentElement: { lang: 'en' },
    title: '',
    getElementById(id) { return elements.get(id) || null; },
    createElement(tag) { return makeElement(tag); },
    querySelectorAll(selector) {
      const results = [];
      const attrMatch = selector.match(/^\[([a-zA-Z0-9_-]+)\]$/);
      for (const el of elements.values()) {
        if (attrMatch && el.hasAttribute(attrMatch[1])) {
          results.push(el);
        }
      }
      return results;
    },
    registerElement(id, el) {
      elements.set(id, el);
      return el;
    }
  };
  return { doc, select, note, msg, makeElement };
}

function createMockFetch(localeMap = catalogs) {
  return async (url, options) => {
    const match = url.match(/\/locales\/(.+)\.json/);
    if (!match) return { ok: false, status: 404, json: async () => ({}) };
    const loc = decodeURIComponent(match[1]);
    if (loc in localeMap) {
      return { ok: true, status: 200, json: async () => localeMap[loc] };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  };
}

function createI18n({
  document,
  localStorage = { getItem: () => null, setItem: () => {} },
  navigator = { languages: ['en'] },
  fetch = createMockFetch(),
  enOverride = enCatalog
} = {}) {
  const ctx = {
    globalThis: {},
    SentinelEnglish: enOverride,
    Intl,
    Date,
    Number,
    String,
    Math,
    Set,
    Map,
    Array,
    Object,
    RegExp,
    Error,
    isNaN,
    AbortSignal,
    AbortController,
    console,
    document,
    localStorage,
    navigator,
    fetch
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(i18nCode, ctx);
  return ctx.SentinelI18n;
}

describe('1. Catalog Parity', () => {
  it('all eight approved locale files exist in src/web/locales', () => {
    for (const loc of APPROVED_LOCALES) {
      const p = path.join(localesDir, `${loc}.json`);
      assert.ok(fs.existsSync(p), `Missing locale file for ${loc}`);
    }
  });

  it('every locale has exact key parity with en.json', () => {
    for (const loc of APPROVED_LOCALES) {
      if (loc === 'en') continue;
      const cat = catalogs[loc];
      const keys = Object.keys(cat).sort();
      const missing = enKeys.filter(k => !(k in cat));
      const extra = keys.filter(k => !(k in enCatalog));
      assert.deepEqual(missing, [], `${loc} has missing keys`);
      assert.deepEqual(extra, [], `${loc} has extra keys`);
    }
  });

  it('SentinelI18n.supported defines all eight locales with code and native name', () => {
    const i = createI18n();
    const codes = [...i.supported.map(s => s.code)];
    assert.deepEqual(codes, APPROVED_LOCALES);
    for (const item of i.supported) {
      assert.equal(typeof item.code, 'string');
      assert.equal(typeof item.name, 'string');
      assert.ok(item.name.trim().length > 0);
    }
  });
});

describe('2. String and Plural Shapes', () => {
  it('every locale key matches string or plural object shape of en.json', () => {
    for (const [key, val] of Object.entries(enCatalog)) {
      const isPlural = typeof val === 'object' && val !== null;
      for (const loc of APPROVED_LOCALES) {
        const targetVal = catalogs[loc][key];
        if (isPlural) {
          assert.equal(
            typeof targetVal,
            'object',
            `${loc} key ${key} must be an object (plural)`
          );
          assert.ok(!Array.isArray(targetVal), `${loc} key ${key} must not be an array`);
          assert.notEqual(targetVal, null, `${loc} key ${key} must not be null`);
        } else {
          assert.equal(
            typeof targetVal,
            'string',
            `${loc} key ${key} must be a string`
          );
        }
      }
    }
  });
});

describe('3. Non-empty Catalog Entries', () => {
  it('no catalog entry or plural form string is empty or whitespace-only', () => {
    for (const loc of APPROVED_LOCALES) {
      for (const [key, val] of Object.entries(catalogs[loc])) {
        if (typeof val === 'string') {
          assert.ok(val.trim().length > 0, `${loc} key ${key} must not be empty`);
        } else {
          for (const [form, formVal] of Object.entries(val)) {
            assert.equal(typeof formVal, 'string', `${loc} ${key}.${form} must be string`);
            assert.ok(formVal.trim().length > 0, `${loc} ${key}.${form} must not be empty`);
          }
        }
      }
    }
  });
});

describe('4. Exact Named Placeholder Sets', () => {
  it('every string key matches canonical placeholders across all locales', () => {
    for (const [key, val] of Object.entries(enCatalog)) {
      if (typeof val !== 'string') continue;
      const expected = extractPlaceholders(val);
      for (const loc of APPROVED_LOCALES) {
        const actual = extractPlaceholders(catalogs[loc][key]);
        assert.deepEqual(
          actual,
          expected,
          `Placeholder mismatch in ${loc} for key ${key}: expected [${expected}], got [${actual}]`
        );
      }
    }
  });

  it('every plural form matches canonical placeholders across all locales', () => {
    for (const [key, val] of Object.entries(enCatalog)) {
      if (typeof val !== 'object' || val === null) continue;
      const enForms = Object.values(val);
      const expected = extractPlaceholders(enForms[0] || '');
      for (const loc of APPROVED_LOCALES) {
        for (const [form, formStr] of Object.entries(catalogs[loc][key])) {
          const actual = extractPlaceholders(formStr);
          assert.deepEqual(
            actual,
            expected,
            `Placeholder mismatch in ${loc} for ${key}.${form}: expected [${expected}], got [${actual}]`
          );
        }
      }
    }
  });
});

describe('5. Intl Required Cardinal Categories', () => {
  it('all plural objects satisfy Intl.PluralRules required categories for each locale', () => {
    for (const loc of APPROVED_LOCALES) {
      const pr = new Intl.PluralRules(loc);
      const required = pr.resolvedOptions().pluralCategories;
      for (const [key, val] of Object.entries(catalogs[loc])) {
        if (typeof val !== 'object' || val === null) continue;
        const present = Object.keys(val);
        for (const reqCat of required) {
          assert.ok(
            present.includes(reqCat),
            `${loc} key ${key} is missing required CLDR cardinal category "${reqCat}" (present: ${present})`
          );
        }
        assert.ok(
          present.includes('other'),
          `${loc} key ${key} must always include "other"`
        );
      }
    }
  });
});

describe('6. Brand and Machine Invariants', () => {
  it('category_form.id_placeholder is "databases" in all eight locales', () => {
    for (const loc of APPROVED_LOCALES) {
      assert.equal(
        catalogs[loc]['category_form.id_placeholder'],
        'databases',
        `${loc} category_form.id_placeholder must be "databases"`
      );
    }
  });

  it('technical units MiB and % remain invariant across all locales', () => {
    for (const loc of APPROVED_LOCALES) {
      assert.equal(catalogs[loc]['unit.mib'], 'MiB', `${loc} unit.mib must be MiB`);
      assert.equal(catalogs[loc]['unit.percent'], '%', `${loc} unit.percent must be %`);
    }
  });
});

describe('7. Technical Semantics: Numeric Formatting & Date Time', () => {
  it('number() formats finite numbers with grouping and exact fixed fractional digits', () => {
    const i = createI18n();
    assert.equal(i.number(1234.56, 1), '1,234.6');
    assert.equal(i.number(1234.56), '1,235'); // default digits=0
    assert.equal(i.number(0, 2), '0.00');
    assert.equal(i.number('1234.5', 2), '1,234.50');
  });

  it('number() returns em dash (—) for non-finite and invalid inputs', () => {
    const i = createI18n();
    assert.equal(i.number(NaN), '—');
    assert.equal(i.number(Infinity), '—');
    assert.equal(i.number(-Infinity), '—');
    assert.equal(i.number(null), '—');
    assert.equal(i.number(undefined), '—');
    assert.equal(i.number(''), '—');
    assert.equal(i.number('   '), '—');
    assert.equal(i.number('invalid'), '—');
    assert.equal(i.number(true), '—');
    assert.equal(i.number(false), '—');
    assert.equal(i.number({}), '—');
  });

  it('number() formats according to active locale separators', async () => {
    const i = createI18n();
    await i.setLocale('de');
    const formatted = i.number(1234.5, 2);
    assert.equal(formatted, '1.234,50');
  });

  it('dateTime() formats valid Date, ISO string, and timestamp; returns em dash for invalid', () => {
    const i = createI18n();
    const d = new Date('2026-09-19T17:00:00Z');
    assert.notEqual(i.dateTime(d), '—');
    assert.notEqual(i.dateTime('2026-09-19T17:00:00Z'), '—');
    assert.notEqual(i.dateTime(d.getTime()), '—');

    assert.equal(i.dateTime('invalid-date'), '—');
    assert.equal(i.dateTime(NaN), '—');
    assert.equal(i.dateTime(null), '—');
    assert.equal(i.dateTime(undefined), '—');
    assert.equal(i.dateTime(''), '—');
    assert.equal(i.dateTime('  '), '—');
  });

  it('dateTime() includes time component by default when options are omitted', () => {
    const i = createI18n();
    const formatted = i.dateTime('2026-09-19T17:00:00Z');
    // Expected behavior: observation timestamp formatted with date and time
    assert.match(
      formatted,
      /\d{1,2}:\d{2}/,
      'dateTime default formatting must include time (hours:minutes)'
    );
  });
});

describe('8. Missing Key and Plural Form Fallback', () => {
  it('falls back to English when key is missing in target catalog', async () => {
    const customCatalogs = {
      de: { 'app.subtitle': 'Deutsch Untertitel' } // missing app.title
    };
    const i = createI18n({ fetch: createMockFetch(customCatalogs) });
    await i.setLocale('de');
    assert.equal(i.t('app.subtitle'), 'Deutsch Untertitel');
    assert.equal(i.t('app.title'), enCatalog['app.title']);
  });

  it('returns raw key when key is unknown in both target and English', async () => {
    const i = createI18n();
    assert.equal(i.t('completely.unknown.key'), 'completely.unknown.key');
  });

  it('falls back to English form or other when target plural form is missing', async () => {
    const customCatalogs = {
      ru: {
        'summary.probes': {
          other: 'зондов (ru other)'
          // missing one, few, many
        }
      }
    };
    const i = createI18n({ fetch: createMockFetch(customCatalogs) });
    await i.setLocale('ru');
    // For count=1, Russian requires 'one'. Missing in target -> falls back to English 'one'
    const res1 = i.t('summary.probes', { count: 1 });
    assert.equal(res1, enCatalog['summary.probes'].one);
  });

  it('handles missing or non-finite count in plural keys gracefully', () => {
    const i = createI18n();
    const resNone = i.t('summary.probes');
    assert.ok(typeof resNone === 'string' && resNone.length > 0);
    const resNaN = i.t('summary.probes', { count: 'abc' });
    assert.ok(typeof resNaN === 'string' && resNaN.length > 0);
  });

  it('hasKey returns true only if key is in English catalog', () => {
    const i = createI18n();
    assert.equal(i.hasKey('app.title'), true);
    assert.equal(i.hasKey('category_form.id_placeholder'), true);
    assert.equal(i.hasKey('nonexistent.key'), false);
    assert.equal(i.hasKey(''), false);
    assert.equal(i.hasKey(null), false);
  });
});

describe('9. Invalid Locale and Structure: No Mutation', () => {
  it('setLocale ignores invalid or unwhitelisted locales without mutating state', async () => {
    const i = createI18n();
    assert.equal(i.locale, 'en');

    await i.setLocale('unknown_lang');
    assert.equal(i.locale, 'en');

    await i.setLocale('');
    assert.equal(i.locale, 'en');

    await i.setLocale(123);
    assert.equal(i.locale, 'en');

    await i.setLocale(null);
    assert.equal(i.locale, 'en');

    await i.setLocale('__proto__');
    assert.equal(i.locale, 'en');
  });

  it('setLocale rejects malformed catalog structures and preserves prior state', async () => {
    const malformedCatalogs = {
      de: [] // array instead of object
    };
    const dom = createMinimalDom();
    const i = createI18n({
      document: dom.doc,
      fetch: createMockFetch(malformedCatalogs)
    });
    assert.equal(i.locale, 'en');
    await i.setLocale('de');
    assert.equal(i.locale, 'en', 'Locale must not mutate on malformed catalog');
    assert.equal(dom.select.value, 'en', 'Select must remain en');
    assert.equal(
      dom.msg.textContent,
      'Could not load Deutsch. The previous language is still active.'
    );
  });

  it('setLocale rejects empty object or invalid value types in catalog', async () => {
    const invalidCatalogs = {
      de: {},
      fr: { 'app.title': 12345 } // number instead of string
    };
    const i = createI18n({ fetch: createMockFetch(invalidCatalogs) });
    await i.setLocale('de');
    assert.equal(i.locale, 'en');
    await i.setLocale('fr');
    assert.equal(i.locale, 'en');
  });
});

describe('10. Supported Browser Locale, Precedence, and Storage Denied', () => {
  it('init() prioritizes stored sentinel.locale over navigator.languages', async () => {
    const storage = {
      getItem: k => (k === 'sentinel.locale' ? 'ru' : null),
      setItem: () => {}
    };
    const nav = { languages: ['de-DE', 'fr'] };
    const i = createI18n({ localStorage: storage, navigator: nav });
    await i.init();
    assert.equal(i.locale, 'ru');
  });

  it('init() falls back to navigator.languages when storage is empty or invalid', async () => {
    const storage = {
      getItem: k => (k === 'sentinel.locale' ? 'invalid-locale' : null),
      setItem: () => {}
    };
    const nav = { languages: ['fr-CA', 'en'] };
    const i = createI18n({ localStorage: storage, navigator: nav });
    await i.init();
    assert.equal(i.locale, 'fr');
  });

  it('negotiates base fallbacks for supported browser language tags', async () => {
    const testCases = [
      { langs: ['en-US'], expected: 'en' },
      { langs: ['ru-RU'], expected: 'ru' },
      { langs: ['de-AT'], expected: 'de' },
      { langs: ['fr-FR'], expected: 'fr' },
      { langs: ['es-MX'], expected: 'es' },
      { langs: ['pt-PT'], expected: 'pt-BR' },
      { langs: ['pt'], expected: 'pt-BR' },
      { langs: ['zh-Hans'], expected: 'zh-CN' },
      { langs: ['zh-SG'], expected: 'zh-CN' },
      { langs: ['ja-JP'], expected: 'ja' }
    ];

    for (const tc of testCases) {
      const i = createI18n({ navigator: { languages: tc.langs } });
      await i.init();
      assert.equal(i.locale, tc.expected, `Negotiation failed for ${tc.langs}`);
    }
  });

  it('does NOT force Traditional Chinese (zh-Hant, zh-TW, zh-HK, zh-MO) to simplified zh-CN', async () => {
    const i1 = createI18n({ navigator: { languages: ['zh-TW', 'ja'] } });
    await i1.init();
    assert.equal(i1.locale, 'ja');

    const i2 = createI18n({ navigator: { languages: ['zh-Hant'] } });
    await i2.init();
    assert.equal(i2.locale, 'en');

    const i3 = createI18n({ navigator: { languages: ['zh-HK', 'de'] } });
    await i3.init();
    assert.equal(i3.locale, 'de');
  });

  it('does NOT alias Belarusian (be) or Kazakh (kk) to Russian (ru)', async () => {
    const i1 = createI18n({ navigator: { languages: ['be', 'en'] } });
    await i1.init();
    assert.equal(i1.locale, 'en');

    const i2 = createI18n({ navigator: { languages: ['kk', 'ru'] } });
    await i2.init();
    assert.equal(i2.locale, 'ru'); // matches second language 'ru', not 'kk'

    const i3 = createI18n({ navigator: { languages: ['kk'] } });
    await i3.init();
    assert.equal(i3.locale, 'en');
  });

  it('handles denied or throwing localStorage gracefully without boot rejection', async () => {
    const throwingStorage = {
      getItem: () => { throw new Error('SecurityError: Access is denied'); },
      setItem: () => { throw new Error('SecurityError: Access is denied'); }
    };
    const i = createI18n({
      localStorage: throwingStorage,
      navigator: { languages: ['de'] }
    });
    await assert.doesNotReject(async () => {
      await i.init();
    });
    assert.equal(i.locale, 'de');

    await assert.doesNotReject(async () => {
      await i.setLocale('fr');
    });
    assert.equal(i.locale, 'fr');
  });
});

describe('11. Race Conditions: Late Success and Late Error', () => {
  it('late successful response never overwrites a newer setLocale request', async () => {
    const pending = {};
    const mockFetch = (url) => new Promise((resolve) => {
      const match = url.match(/\/locales\/(.+)\.json/);
      const loc = match[1];
      pending[loc] = resolve;
    });

    const i = createI18n({ fetch: mockFetch });
    const p1 = i.setLocale('ru'); // slow request 1
    const p2 = i.setLocale('de'); // fast request 2

    // Resolve de first
    pending['de']({ ok: true, status: 200, json: async () => catalogs.de });
    await p2;
    assert.equal(i.locale, 'de');

    // Late resolve ru
    pending['ru']({ ok: true, status: 200, json: async () => catalogs.ru });
    await p1;
    assert.equal(i.locale, 'de', 'Late success from request 1 must not overwrite request 2');
  });

  it('late error response never overwrites or corrupts a newer setLocale request', async () => {
    const pending = {};
    const mockFetch = (url) => new Promise((resolve, reject) => {
      const match = url.match(/\/locales\/(.+)\.json/);
      const loc = match[1];
      pending[loc] = { resolve, reject };
    });

    const dom = createMinimalDom();
    const i = createI18n({ document: dom.doc, fetch: mockFetch });
    const p1 = i.setLocale('ru'); // failing slow request
    const p2 = i.setLocale('de'); // successful fast request

    pending['de'].resolve({ ok: true, status: 200, json: async () => catalogs.de });
    await p2;
    assert.equal(i.locale, 'de');
    assert.equal(dom.select.value, 'de');

    // Reject ru late
    pending['ru'].reject(new Error('Network drop'));
    await p1;
    assert.equal(i.locale, 'de');
    assert.equal(dom.select.value, 'de');
    assert.equal(dom.msg.textContent, '', 'Late error must not display load error message');
  });
});

describe('12. Failed Locale Preserves State', () => {
  it('failed setLocale keeps prior active locale, catalog, selection, and displays error', async () => {
    const dom = createMinimalDom();
    const failingFetch = async () => ({ ok: false, status: 500, json: async () => ({}) });

    const i = createI18n({ document: dom.doc, fetch: failingFetch });
    assert.equal(i.locale, 'en');

    await i.setLocale('ru');
    assert.equal(i.locale, 'en', 'Must preserve en on failure');
    assert.equal(dom.select.value, 'en', 'Select must stay en');
    assert.equal(
      dom.msg.textContent,
      'Could not load Русский. The previous language is still active.'
    );
    assert.equal(i.t('app.title'), enCatalog['app.title']);
  });

  it('onChange listeners are not triggered on failed setLocale', async () => {
    let fired = 0;
    const failingFetch = async () => ({ ok: false, status: 500, json: async () => ({}) });
    const i = createI18n({ fetch: failingFetch });
    i.onChange(() => { fired++; });

    await i.setLocale('de');
    assert.equal(fired, 0, 'onChange must not fire on failed setLocale');
  });
});

describe('13. Safe Interpolation & Literal HTML', () => {
  it('t() performs literal string interpolation without evaluating HTML or script tags', () => {
    const i = createI18n();
    const malicious = '<script>alert("xss")</script>';
    const result = i.t('fleet.cell', { name: malicious, state: 'OK' });
    assert.equal(result, '<script>alert("xss")</script>: OK');
  });

  it('apply() sets textContent rather than innerHTML on data-i18n leaf elements', () => {
    const dom = createMinimalDom();
    const leaf = dom.makeElement('div', { 'data-i18n': 'app.title' });
    dom.doc.registerElement('titleEl', leaf);

    const i = createI18n({ document: dom.doc });
    i.apply();
    assert.equal(leaf.textContent, enCatalog['app.title']);
    assert.equal(leaf.innerHTML, undefined, 'Must not use innerHTML');
  });

  it('apply() sets attributes safely as text for aria, title, and placeholder', () => {
    const dom = createMinimalDom();
    const el = dom.makeElement('input', {
      'data-i18n-aria': 'language.label',
      'data-i18n-title': 'details.title',
      'data-i18n-placeholder': 'filter.search_placeholder'
    });
    dom.doc.registerElement('inputEl', el);

    const i = createI18n({ document: dom.doc });
    i.apply();
    assert.equal(el.getAttribute('aria-label'), enCatalog['language.label']);
    assert.equal(el.getAttribute('title'), enCatalog['details.title']);
    assert.equal(el.getAttribute('placeholder'), enCatalog['filter.search_placeholder']);
  });
});

describe('14. Bounded Loads', () => {
  it('enforces bounded loads with an AbortSignal on fetch', async () => {
    let capturedOptions = null;
    const mockFetch = async (url, options) => {
      capturedOptions = options;
      return { ok: true, status: 200, json: async () => catalogs.de };
    };
    const i = createI18n({ fetch: mockFetch });
    await i.setLocale('de');
    assert.ok(
      capturedOptions && capturedOptions.signal instanceof AbortSignal,
      'fetch must be called with an AbortSignal for bounded load timeout'
    );
  });
});
