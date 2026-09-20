// src/web/i18n.js
// Runtime internationalization module for Jev Sentinel.
// Injected after `globalThis.SentinelEnglish = <JSON from en.json>;` by the server.

(function () {
  'use strict';

  /**
   * Supported locales with native display names.
   */
  const SUPPORTED = Object.freeze([
    { code: 'en', name: 'English' },
    { code: 'ru', name: 'Русский' },
    { code: 'de', name: 'Deutsch' },
    { code: 'fr', name: 'Français' },
    { code: 'es', name: 'Español' },
    { code: 'pt-BR', name: 'Português (Brasil)' },
    { code: 'zh-CN', name: '简体中文' },
    { code: 'ja', name: '日本語' }
  ]);

  const SUPPORTED_CODES = new Set(SUPPORTED.map(function (s) { return s.code; }));

  // Internal state
  let currentLocale = 'en';
  let currentCatalog = null;
  let currentLocaleMessageState = null; // null or { key: string, params: object }
  let currentRequestId = 0;
  let initHooked = false;

  const callbacks = new Set();
  const catalogCache = new Map();
  const pluralRulesCache = new Map();
  const numberFormatCache = new Map();
  const dateTimeFormatCache = new Map();

  /**
   * Safe access to the injected English catalog.
   */
  function getEnglishCatalog() {
    const globalEn = typeof globalThis !== 'undefined' ? globalThis.SentinelEnglish : null;
    if (globalEn && typeof globalEn === 'object' && !Array.isArray(globalEn)) {
      return globalEn;
    }
    return {};
  }

  /**
   * Validates catalog structure: JSON object with string values or cardinal-plural objects.
   */
  function isValidCatalog(obj) {
    if (!obj || typeof obj !== 'object' || Array.isArray(obj)) {
      return false;
    }
    const keys = Object.keys(obj);
    if (keys.length === 0) {
      return false;
    }
    for (let i = 0; i < keys.length; i++) {
      const key = keys[i];
      const val = obj[key];
      if (typeof val === 'string') {
        continue;
      }
      if (val && typeof val === 'object' && !Array.isArray(val)) {
        const formKeys = Object.keys(val);
        if (formKeys.length === 0) return false;
        for (let j = 0; j < formKeys.length; j++) {
          if (typeof val[formKeys[j]] !== 'string') {
            return false;
          }
        }
      } else {
        return false;
      }
    }
    return true;
  }

  /**
   * Safe localStorage helper to save locale.
   */
  function saveLocale(code) {
    try {
      if (typeof localStorage !== 'undefined') {
        localStorage.setItem('sentinel.locale', code);
      }
    } catch (_) {
      // localStorage failure fallback
    }
  }

  /**
   * Cached Intl.PluralRules lookup.
   */
  function getPluralRules(locale) {
    let pr = pluralRulesCache.get(locale);
    if (!pr) {
      try {
        pr = new Intl.PluralRules(locale);
      } catch (_) {
        pr = new Intl.PluralRules('en');
      }
      pluralRulesCache.set(locale, pr);
    }
    return pr;
  }

  /**
   * Safe number formatting: finite numbers via Intl.NumberFormat, exact fixed fractional digits and grouping; invalid -> em dash.
   */
  function number(value, digits) {
    const num = typeof value === 'number' ? value : (typeof value === 'string' && value.trim() !== '' ? Number(value) : NaN);
    if (typeof value !== 'number' && typeof value !== 'string') return '—';
    if (!Number.isFinite(num)) return '—';

    const d = (typeof digits === 'number' && Number.isFinite(digits))
      ? Math.max(0, Math.min(20, Math.floor(digits)))
      : 0;

    const key = currentLocale + ':' + d;
    let formatter = numberFormatCache.get(key);
    if (!formatter) {
      try {
        formatter = new Intl.NumberFormat(currentLocale, {
          minimumFractionDigits: d,
          maximumFractionDigits: d,
          useGrouping: true
        });
        numberFormatCache.set(key, formatter);
      } catch (_) {
        return num.toFixed(d);
      }
    }
    return formatter.format(num);
  }

  /**
   * Safe date-time formatting: valid Date via Intl.DateTimeFormat(current locale); invalid -> em dash.
   */
  function dateTime(iso, options = {dateStyle: 'medium', timeStyle: 'medium'}) {
    if (iso === null || iso === undefined || iso === '') return '—';
    let d;
    if (iso instanceof Date) {
      d = iso;
    } else if (typeof iso === 'string' || typeof iso === 'number') {
      if (typeof iso === 'string' && iso.trim() === '') return '—';
      d = new Date(iso);
    } else {
      return '—';
    }

    if (isNaN(d.getTime())) {
      return '—';
    }

    const key = options ? currentLocale + ':' + JSON.stringify(options) : currentLocale;
    let formatter = dateTimeFormatCache.get(key);
    if (!formatter) {
      try {
        formatter = options
          ? new Intl.DateTimeFormat(currentLocale, options)
          : new Intl.DateTimeFormat(currentLocale);
        dateTimeFormatCache.set(key, formatter);
      } catch (_) {
        return '—';
      }
    }

    try {
      return formatter.format(d);
    } catch (_) {
      return '—';
    }
  }

  /**
   * True only if key is present in English catalog.
   */
  function hasKey(key) {
    if (typeof key !== 'string') return false;
    const en = getEnglishCatalog();
    return Object.prototype.hasOwnProperty.call(en, key);
  }

  /**
   * Named parameter interpolation: replaces {name} with values; numeric params use number formatting.
   */
  function interpolate(template, params) {
    if (typeof template !== 'string') return '';
    if (!params || typeof params !== 'object') return template;

    return template.replace(/\{([a-zA-Z0-9_]+)\}/g, function (match, name) {
      if (!Object.prototype.hasOwnProperty.call(params, name)) {
        return match;
      }
      const val = params[name];
      if (val === null || val === undefined) {
        return '';
      }
      if (typeof val === 'number') {
        return number(val);
      }
      return String(val);
    });
  }

  /**
   * Translate key with optional params.
   * Named interpolation, numeric params formatted via number(), CLDR plurals via Intl.PluralRules,
   * per-key and per-form English fallback; unknown key returns key.
   */
  function t(key, params) {
    if (typeof key !== 'string') return String(key || '');

    const enCatalog = getEnglishCatalog();
    const targetVal = (currentLocale !== 'en' && currentCatalog) ? currentCatalog[key] : undefined;
    const enVal = enCatalog[key];

    const hasTarget = targetVal !== undefined;
    const hasEn = enVal !== undefined;

    if (!hasTarget && !hasEn) {
      return key; // unknown key returns key
    }

    const isPlural = (targetVal && typeof targetVal === 'object') || (enVal && typeof enVal === 'object');

    let template;
    if (isPlural) {
      const hasCount = params && typeof params === 'object' && ('count' in params);
      const countVal = hasCount ? params.count : undefined;
      const countNum = (typeof countVal === 'number')
        ? countVal
        : (typeof countVal === 'string' && countVal.trim() !== '' && Number.isFinite(Number(countVal)) ? Number(countVal) : NaN);

      if (Number.isFinite(countNum)) {
        const form = getPluralRules(currentLocale).select(countNum);

        // Try target catalog first
        if (targetVal && typeof targetVal === 'object') {
          template = targetVal[form];
        } else if (typeof targetVal === 'string') {
          template = targetVal;
        }

        // Per-form / per-key English fallback
        if (typeof template !== 'string') {
          if (enVal && typeof enVal === 'object') {
            const enForm = getPluralRules('en').select(countNum);
            template = enVal[enForm];
            if (typeof template !== 'string') {
              template = enVal.other;
            }
          } else if (typeof enVal === 'string') {
            template = enVal;
          }
        }

        // Fall back to target 'other' if still undefined
        if (typeof template !== 'string' && targetVal && typeof targetVal === 'object') {
          template = targetVal.other;
        }
      } else {
        // count is not finite or omitted
        if (targetVal && typeof targetVal === 'object') {
          template = targetVal.other || targetVal.one;
        } else if (typeof targetVal === 'string') {
          template = targetVal;
        }

        if (typeof template !== 'string' && enVal) {
          if (typeof enVal === 'object') {
            template = enVal.other || enVal.one;
          } else if (typeof enVal === 'string') {
            template = enVal;
          }
        }
      }
    } else {
      // Non-plural
      if (typeof targetVal === 'string') {
        template = targetVal;
      } else if (typeof enVal === 'string') {
        template = enVal;
      }
    }

    if (typeof template !== 'string') {
      return key;
    }

    return interpolate(template, params);
  }

  /**
   * Browser language negotiation:
   * en-US->en, ru-RU->ru, de etc, pt variants->pt-BR;
   * zh/zh-Hans/zh-CN->zh-CN but zh-Hant/TW/HK/MO NOT forced to simplified, use next browser locale or en.
   * No be/kk->ru alias.
   */
  function matchCandidateLocale(tag) {
    if (typeof tag !== 'string') return null;
    const raw = tag.trim();
    if (!raw) return null;
    const lower = raw.toLowerCase();

    // Chinese variants
    if (lower === 'zh' || lower.startsWith('zh-')) {
      // Exclude Traditional Chinese variants: do not force to simplified
      if (
        lower.startsWith('zh-hant') ||
        lower === 'zh-tw' || lower.startsWith('zh-tw-') ||
        lower === 'zh-hk' || lower.startsWith('zh-hk-') ||
        lower === 'zh-mo' || lower.startsWith('zh-mo-')
      ) {
        return null;
      }
      // zh, zh-CN, zh-Hans, zh-SG map to zh-CN
      if (
        lower === 'zh' ||
        lower === 'zh-cn' || lower.startsWith('zh-cn-') ||
        lower.startsWith('zh-hans') ||
        lower === 'zh-sg' || lower.startsWith('zh-sg-')
      ) {
        return 'zh-CN';
      }
      return null;
    }

    // Portuguese variants -> pt-BR
    if (lower === 'pt' || lower.startsWith('pt-')) {
      return 'pt-BR';
    }

    // Base fallbacks for other supported languages
    if (lower === 'en' || lower.startsWith('en-')) return 'en';
    if (lower === 'ru' || lower.startsWith('ru-')) return 'ru';
    if (lower === 'de' || lower.startsWith('de-')) return 'de';
    if (lower === 'fr' || lower.startsWith('fr-')) return 'fr';
    if (lower === 'es' || lower.startsWith('es-')) return 'es';
    if (lower === 'ja' || lower.startsWith('ja-')) return 'ja';

    return null;
  }

  function negotiateLocale(languages) {
    if (Array.isArray(languages)) {
      for (let i = 0; i < languages.length; i++) {
        const matched = matchCandidateLocale(languages[i]);
        if (matched) return matched;
      }
    }
    return 'en';
  }

  /**
   * Applies translations to the DOM.
   * Translates leaf [data-i18n], [data-i18n-aria], [data-i18n-title], [data-i18n-placeholder]
   * through textContent and attributes, updates html.lang/title, language selection,
   * shows #translationNote for any non-English locale, and updates #localeMessage.
   */
  function apply(root) {
    if (root === undefined && typeof document !== 'undefined') {
      root = document;
    }
    if (!root || typeof root !== 'object') return;

    function query(selector) {
      const list = [];
      if (typeof root.querySelectorAll === 'function') {
        try {
          const matched = root.querySelectorAll(selector);
          for (let i = 0; i < matched.length; i++) {
            list.push(matched[i]);
          }
        } catch (_) {}
      }
      const attr = selector.slice(1, -1);
      if (typeof root.getAttribute === 'function' && root.hasAttribute && root.hasAttribute(attr)) {
        list.unshift(root);
      }
      return list;
    }

    // Leaf [data-i18n]
    const i18nEls = query('[data-i18n]');
    for (let i = 0; i < i18nEls.length; i++) {
      const el = i18nEls[i];
      const key = el.getAttribute('data-i18n');
      if (key && (!el.children || el.children.length === 0)) {
        el.textContent = t(key);
      }
    }

    // [data-i18n-aria] -> aria-label
    const ariaEls = query('[data-i18n-aria]');
    for (let i = 0; i < ariaEls.length; i++) {
      const el = ariaEls[i];
      const key = el.getAttribute('data-i18n-aria');
      if (key) {
        el.setAttribute('aria-label', t(key));
      }
    }

    // [data-i18n-title] -> title
    const titleEls = query('[data-i18n-title]');
    for (let i = 0; i < titleEls.length; i++) {
      const el = titleEls[i];
      const key = el.getAttribute('data-i18n-title');
      if (key) {
        el.setAttribute('title', t(key));
      }
    }

    // [data-i18n-placeholder] -> placeholder
    const placeholderEls = query('[data-i18n-placeholder]');
    for (let i = 0; i < placeholderEls.length; i++) {
      const el = placeholderEls[i];
      const key = el.getAttribute('data-i18n-placeholder');
      if (key) {
        el.setAttribute('placeholder', t(key));
      }
    }

    // Document-level updates
    const doc = (root.nodeType === 9)
      ? root
      : (root.ownerDocument || (typeof document !== 'undefined' ? document : null));

    if (doc) {
      if (doc.documentElement) {
        doc.documentElement.lang = currentLocale;
      }
      if (hasKey('app.title')) {
        doc.title = t('app.title');
      }
      if (typeof doc.getElementById === 'function') {
        const select = doc.getElementById('languageSelect');
        if (select) {
          select.value = currentLocale;
        }

        const note = doc.getElementById('translationNote');
        if (note) {
          if (currentLocale !== 'en') {
            note.hidden = false;
            if (typeof note.removeAttribute === 'function') note.removeAttribute('hidden');
            if (hasKey('language.machine_note')) {
              note.textContent = t('language.machine_note');
            }
          } else {
            note.hidden = true;
            if (typeof note.setAttribute === 'function') note.setAttribute('hidden', '');
          }
        }

        const msg = doc.getElementById('localeMessage');
        if (msg) {
          if (currentLocaleMessageState) {
            msg.textContent = t(currentLocaleMessageState.key, currentLocaleMessageState.params);
            msg.hidden = false;
            if (typeof msg.removeAttribute === 'function') msg.removeAttribute('hidden');
          } else {
            msg.textContent = '';
            msg.hidden = true;
            if (typeof msg.setAttribute === 'function') msg.setAttribute('hidden', '');
          }
        }
      }
    }
  }

  /**
   * Notifies all registered onChange listeners.
   */
  function notifyChange() {
    const list = Array.from(callbacks);
    for (let i = 0; i < list.length; i++) {
      try {
        list[i]();
      } catch (err) {
        if (typeof console !== 'undefined' && console.error) {
          console.error(err);
        }
      }
    }
  }

  /**
   * Set the active locale.
   * Async, last request wins, whitelist, failed load keeps prior active dictionary/selection,
   * error message in #localeMessage, no unhandled rejection.
   */
  async function setLocale(code) {
    if (typeof code !== 'string' || !SUPPORTED_CODES.has(code)) {
      return;
    }

    const requestId = ++currentRequestId;

    if (code === 'en') {
      currentLocale = 'en';
      currentCatalog = getEnglishCatalog();
      currentLocaleMessageState = null;
      saveLocale('en');
      apply();
      notifyChange();
      return;
    }

    let catalog = catalogCache.get(code);
    if (!catalog) {
      try {
        if (typeof fetch !== 'function') {
          throw new Error('fetch unavailable');
        }
        const res = await fetch('/locales/' + encodeURIComponent(code) + '.json', {signal: AbortSignal.timeout(4000), cache: 'no-store'});
        if (!res.ok) {
          throw new Error('HTTP ' + res.status);
        }
        const data = await res.json();
        if (!isValidCatalog(data)) {
          throw new Error('Invalid catalog format');
        }
        catalog = data;
        catalogCache.set(code, catalog);
      } catch (_) {
        if (requestId !== currentRequestId) {
          return; // Superseded by newer request
        }
        // Failed load keeps prior active dictionary and selection
        let langName = code;
        for (let i = 0; i < SUPPORTED.length; i++) {
          if (SUPPORTED[i].code === code) {
            langName = SUPPORTED[i].name;
            break;
          }
        }
        currentLocaleMessageState = {
          key: 'language.load_error',
          params: { language: langName }
        };
        if (typeof document !== 'undefined') {
          const select = document.getElementById('languageSelect');
          if (select) select.value = currentLocale;
          const msg = document.getElementById('localeMessage');
          if (msg) {
            msg.textContent = t(currentLocaleMessageState.key, currentLocaleMessageState.params);
            msg.hidden = false;
            if (typeof msg.removeAttribute === 'function') msg.removeAttribute('hidden');
          }
        }
        return; // No unhandled rejection
      }
    }

    if (requestId !== currentRequestId) {
      return; // Superseded
    }

    currentLocale = code;
    currentCatalog = catalog;
    currentLocaleMessageState = null;
    saveLocale(code);
    apply();
    notifyChange();
  }

  /**
   * Initializes the runtime i18n system.
   * Reads sentinel.locale or negotiates navigator.languages, hooks #languageSelect change,
   * applies labels and selection, handles load error without rejecting boot.
   */
  async function init() {
    if (typeof document !== 'undefined') {
      const select = document.getElementById('languageSelect');
      if (select && !initHooked) {
        initHooked = true;
        if (select.options && select.options.length === 0 && typeof document.createElement === 'function') {
          for (let i = 0; i < SUPPORTED.length; i++) {
            const item = SUPPORTED[i];
            const opt = document.createElement('option');
            opt.value = item.code;
            opt.textContent = item.name;
            select.appendChild(opt);
          }
        }
        select.addEventListener('change', function (e) {
          setLocale(e.target.value);
        });
      }
    }

    let target = null;
    try {
      if (typeof localStorage !== 'undefined') {
        const stored = localStorage.getItem('sentinel.locale');
        if (stored && SUPPORTED_CODES.has(stored)) {
          target = stored;
        }
      }
    } catch (_) {
      // localStorage failure fallback
    }

    if (!target) {
      if (typeof navigator !== 'undefined') {
        const langs = Array.isArray(navigator.languages) && navigator.languages.length > 0
          ? navigator.languages
          : (navigator.language ? [navigator.language] : []);
        target = negotiateLocale(langs);
      } else {
        target = 'en';
      }
    }

    if (!SUPPORTED_CODES.has(target)) {
      target = 'en';
    }

    try {
      await setLocale(target);
    } catch (_) {
      // Handles load error without rejecting boot
    }
  }

  /**
   * Register a callback to be called after a successful locale change.
   * Callback takes no arguments.
   */
  function onChange(callback) {
    if (typeof callback === 'function') {
      callbacks.add(callback);
      return function () {
        callbacks.delete(callback);
      };
    }
  }

  const SentinelI18n = {
    supported: SUPPORTED,
    get locale() {
      return currentLocale;
    },
    init: init,
    setLocale: setLocale,
    t: t,
    hasKey: hasKey,
    number: number,
    dateTime: dateTime,
    apply: apply,
    onChange: onChange
  };

  globalThis.SentinelI18n = SentinelI18n;
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = SentinelI18n;
  }
})();
