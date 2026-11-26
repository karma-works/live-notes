/* global LiveNotesApi */
'use strict';

const t = (key, subs) => browser.i18n.getMessage(key, subs) || key;
const $ = (id) => document.getElementById(id);
const HOSTS = ['http://127.0.0.1/*', 'https://www.youtube.com/*'];

for (const el of document.querySelectorAll('[data-i18n]')) el.textContent = t(el.dataset.i18n);
document.documentElement.lang = browser.i18n.getUILanguage();

function status(text, kind) {
  $('status').textContent = text || '';
  $('status').className = kind || '';
}

async function load() {
  const s = await browser.storage.local.get({ port: LiveNotesApi.DEFAULT_PORT, token: '' });
  $('port').value = s.port;
  $('token').value = s.token;
  await checkPermission();
}

async function checkPermission() {
  const ok = await browser.permissions.contains({ origins: HOSTS });
  $('perm').hidden = ok;
  return ok;
}

function current() {
  return { port: Number($('port').value) || LiveNotesApi.DEFAULT_PORT, token: $('token').value.trim() };
}

async function save() {
  const s = current();
  await browser.storage.local.set(s);
  return s;
}

$('form').addEventListener('submit', async (ev) => {
  ev.preventDefault();
  await save();
  status(t('optSaved'), 'ok');
});

$('show').addEventListener('click', () => {
  $('token').type = $('token').type === 'password' ? 'text' : 'password';
});

$('grant').addEventListener('click', async () => {
  await browser.permissions.request({ origins: HOSTS }).catch(() => false);
  await checkPermission();
});

$('test').addEventListener('click', async () => {
  // request first (synchronously in the click handler) so Firefox can prompt
  const granted = browser.permissions.request({ origins: HOSTS }).catch(() => false);
  status(t('optTesting'));
  await granted;
  await checkPermission();
  const s = await save();
  try {
    const r = await LiveNotesApi.ping(s);
    status(t('optTestOk', [(r && r.version) || '?']), 'ok');
  } catch (e) {
    const key =
      {
        not_running: 'errNotRunning',
        unauthorized: 'errUnauthorized',
        forbidden: 'errForbidden',
        disabled: 'errDisabled',
        bad_settings: 'errBadSettings',
      }[e.code] || null;
    status(key ? t(key) : t('errHttp', [e.message]), 'err');
  }
});

load();
