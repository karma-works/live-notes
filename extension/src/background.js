/* global LiveNotesApi, LiveNotesPayload */
/*
 * Event page: toolbar button, context menu and keyboard shortcut all end up in saveTab().
 * Loaded after lib/api.js, lib/payload.js and lib/youtube.js (see manifest.json).
 */
'use strict';

const yt = globalThis.__liveNotesYouTube;
const t = (key, subs) => browser.i18n.getMessage(key, subs) || key;

const HOSTS = ['http://127.0.0.1/*', 'https://www.youtube.com/*'];
const POLL_MS = 2000;
const JOB_TIMEOUT_MS = 60 * 60 * 1000; // Whisper on a long video can take a while
const BADGE_CLEAR_MS = 8000;
const COLORS = { busy: '#6B7280', ok: '#2E9E5B', err: '#D9402B' };

const busyTabs = new Set();
let lastExtraction = null; // diagnostics: which transcript strategies ran last time
const polling = new Set();

// ---------------------------------------------------------------------------------------
// Badge + notifications

async function badge(tabId, state) {
  if (tabId == null) return;
  const text = { busy: '…', ok: '✓', err: '!', none: '' }[state];
  try {
    await browser.action.setBadgeText({ tabId, text });
    if (state !== 'none') await browser.action.setBadgeBackgroundColor({ tabId, color: COLORS[state] });
    if (state === 'ok' || state === 'err') {
      setTimeout(() => browser.action.setBadgeText({ tabId, text: '' }).catch(() => {}), BADGE_CLEAR_MS);
    }
  } catch {
    // tab closed
  }
}

function notify(title, message, id) {
  return browser.notifications
    .create(id || '', { type: 'basic', iconUrl: browser.runtime.getURL('icons/icon-96.png'), title, message })
    .catch(() => {});
}

function errorMessage(e) {
  const code = e && e.code;
  switch (code) {
    case 'not_running':
      return t('errNotRunning');
    case 'unauthorized':
      return t('errUnauthorized');
    case 'forbidden':
      return t('errForbidden');
    case 'too_large':
      return t('errTooLarge');
    case 'disabled':
      return t('errDisabled');
    case 'not_found':
      return t('errJobLost');
    case 'bad_settings':
      return t('errBadSettings');
    case 'script':
      return t('errScript', [e.message]);
    default:
      return t('errHttp', [String((e && e.message) || e)]);
  }
}

async function fail(tabId, e) {
  console.warn('[live-notes]', e);
  await badge(tabId, 'err');
  await notify(t('errTitle'), errorMessage(e));
  if (e && (e.code === 'unauthorized' || e.code === 'bad_settings')) {
    browser.runtime.openOptionsPage().catch(() => {});
  }
}

// ---------------------------------------------------------------------------------------
// Settings + jobs

async function settings() {
  const s = await browser.storage.local.get({ port: LiveNotesApi.DEFAULT_PORT, token: '' });
  return { port: Number(s.port) || LiveNotesApi.DEFAULT_PORT, token: String(s.token || '').trim() };
}

async function rememberJob(id, info) {
  const { jobs = {} } = await browser.storage.session.get('jobs');
  jobs[id] = info;
  await browser.storage.session.set({ jobs });
}

async function forgetJob(id) {
  const { jobs = {} } = await browser.storage.session.get('jobs');
  delete jobs[id];
  await browser.storage.session.set({ jobs });
}

/** Poll GET /v1/clip/{id} until done/error. Resolves with the final job state. */
async function pollJob(id, info) {
  if (polling.has(id)) return null;
  polling.add(id);
  try {
    for (;;) {
      if (Date.now() - info.started > JOB_TIMEOUT_MS) {
        await forgetJob(id);
        await badge(info.tabId, 'none');
        await notify(t('notifyStillRunningTitle'), t('notifyStillRunning', [info.title]));
        return { status: 'timeout' };
      }
      let st;
      try {
        st = await LiveNotesApi.job(await settings(), id);
      } catch (e) {
        if (e.code === 'not_running') {
          // app restarted or briefly busy: keep trying, the alarm will resume later as well
          await new Promise((r) => setTimeout(r, POLL_MS * 3));
          continue;
        }
        await forgetJob(id);
        await fail(info.tabId, e);
        return { status: 'error', error: e.code };
      }
      if (st && st.status === 'done') {
        await forgetJob(id);
        await badge(info.tabId, 'ok');
        await notify(t('notifySavedTitle'), t('notifySaved', [st.title || info.title]));
        return st;
      }
      if (st && st.status === 'error') {
        await forgetJob(id);
        await badge(info.tabId, 'err');
        await notify(t('errTitle'), t('errJob', [st.error || '?']));
        return st;
      }
      await new Promise((r) => setTimeout(r, POLL_MS));
    }
  } finally {
    polling.delete(id);
  }
}

// ---------------------------------------------------------------------------------------
// Extraction

async function exec(tabId, files, world) {
  const res = await browser.scripting.executeScript({ target: { tabId }, files, world, injectImmediately: false });
  const r = res && res[0];
  if (!r) throw Object.assign(new Error('no result'), { code: 'script' });
  if (r.error) throw Object.assign(new Error(String(r.error.message || r.error)), { code: 'script' });
  return r.result;
}

async function buildPayload(tab, menuSelection) {
  if (yt.isYouTubeVideoUrl(tab.url)) {
    const r = await exec(tab.id, ['lib/youtube.js', 'content/youtube-main.js'], 'MAIN');
    if (r && !r.error) {
      lastExtraction = { attempts: r.attempts, refetched: r.refetched };
      console.info('[live-notes] YouTube transcript attempts', r.attempts, 'refetched', r.refetched);
      return LiveNotesPayload.buildYouTubePayload(r, tab, menuSelection);
    }
  }
  const r = await exec(tab.id, ['vendor/Readability.js', 'content/page-extract.js'], 'ISOLATED');
  return LiveNotesPayload.buildPagePayload(r, tab, menuSelection);
}

// ---------------------------------------------------------------------------------------
// Main action

async function saveTab(tab, menuSelection) {
  if (!tab || tab.id == null) return { status: 'error', error: 'no_tab' };
  if (busyTabs.has(tab.id)) return { status: 'busy' };
  busyTabs.add(tab.id);
  try {
    await badge(tab.id, 'busy');
    const s = await settings();
    if (!s.token) throw new LiveNotesApi.ClipError('unauthorized', 'no token');

    let payload;
    try {
      payload = await buildPayload(tab, menuSelection);
    } catch (e) {
      // Pages that cannot be scripted (reader view, PDF viewer, addons.mozilla.org, …):
      // still send URL + title so the app can at least keep the link.
      if (!/^https?:/.test(tab.url || '')) throw Object.assign(new Error(t('errNotAPage')), { code: 'script' });
      console.warn('[live-notes] extraction failed, sending URL only', e);
      payload = LiveNotesPayload.buildFallbackPayload(tab, menuSelection);
    }

    const res = await LiveNotesApi.clip(s, payload);
    if (!res || !res.job_id) throw new LiveNotesApi.ClipError('http', 'no job_id');
    const info = { tabId: tab.id, title: payload.title, started: Date.now() };
    await rememberJob(res.job_id, info);
    busyTabs.delete(tab.id);
    const final = await pollJob(res.job_id, info);
    return Object.assign({ job_id: res.job_id, kind: payload.kind }, final);
  } catch (e) {
    await fail(tab.id, e);
    return { status: 'error', error: e.code || String(e) };
  } finally {
    busyTabs.delete(tab.id);
  }
}

/** Must be the first call in a user-action handler so Firefox may show its prompt. */
function ensureHostPermissions() {
  return browser.permissions.request({ origins: HOSTS }).catch(() => false);
}

function onUserAction(tab, selection) {
  const granted = ensureHostPermissions();
  granted.then(() => saveTab(tab, selection));
}

browser.action.onClicked.addListener((tab) => onUserAction(tab));

browser.commands.onCommand.addListener((name, tab) => {
  if (name !== 'save-to-live-notes') return;
  if (tab) return onUserAction(tab);
  const granted = ensureHostPermissions();
  browser.tabs.query({ active: true, currentWindow: true }).then(([active]) => granted.then(() => saveTab(active)));
});

browser.menus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === 'save-to-live-notes') onUserAction(tab, info.selectionText);
});

function createMenu() {
  browser.menus.removeAll().then(() =>
    browser.menus.create({ id: 'save-to-live-notes', title: t('menuSave'), contexts: ['page', 'selection', 'frame'] }),
  );
}
browser.runtime.onInstalled.addListener(createMenu);
browser.runtime.onStartup.addListener(createMenu);

// Resume polling jobs if the event page was suspended in the meantime.
browser.alarms.create('resume-jobs', { periodInMinutes: 1 });
browser.alarms.onAlarm.addListener(async (alarm) => {
  if (alarm.name !== 'resume-jobs') return;
  const { jobs = {} } = await browser.storage.session.get('jobs');
  for (const [id, info] of Object.entries(jobs)) pollJob(id, info);
});

browser.runtime.onInstalled.addListener(async ({ reason }) => {
  if (reason !== 'install') return;
  const s = await settings();
  if (!s.token) browser.runtime.openOptionsPage().catch(() => {});
});

globalThis.LiveNotesBackground = { saveTab, settings, lastExtraction: () => lastExtraction };
