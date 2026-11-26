/*
 * TEST BUILD ONLY (added by `node scripts/build.mjs --test`, never in dist/).
 * Polls the mock server's /__test/next on a fixed port and executes commands, so the e2e test
 * can trigger the same saveTab() that the toolbar button uses without driving the desktop.
 */
'use strict';
(() => {
  const CONTROL = 'http://127.0.0.1:__TEST_CONTROL_PORT__/__test/';
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const report = (obj) =>
    fetch(CONTROL + 'report', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(obj) }).catch(() => {});

  function waitComplete(tabId, timeoutMs) {
    return new Promise((resolve) => {
      const done = () => {
        browser.tabs.onUpdated.removeListener(listener);
        clearTimeout(timer);
        resolve();
      };
      const listener = (id, info) => {
        if (id === tabId && info.status === 'complete') done();
      };
      const timer = setTimeout(done, timeoutMs);
      browser.tabs.onUpdated.addListener(listener);
      browser.tabs.get(tabId).then((t) => t.status === 'complete' && done(), done);
    });
  }

  async function run(cmd) {
    if (cmd.action === 'configure') {
      await browser.storage.local.set({ port: cmd.port, token: cmd.token });
      const perms = await browser.permissions.getAll();
      return report({ action: 'configure', ok: true, origins: perms.origins, version: browser.runtime.getManifest().version });
    }
    if (cmd.action === 'save') {
      const tab = await browser.tabs.create({ url: cmd.url, active: true });
      await waitComplete(tab.id, 45000);
      await sleep(cmd.wait_ms || 2000);
      // test-only page preparation: each step clicks the first matching selector, e.g. dismiss
      // a consent dialog in the throwaway profile or follow a related video (SPA navigation)
      const steps = cmd.clicks || (cmd.click ? [cmd.click] : []);
      for (const sels of steps) {
        const clicked = await browser.scripting
          .executeScript({
            target: { tabId: tab.id },
            func: (list) => {
              for (const s of list) {
                const el = document.querySelector(s);
                if (el) {
                  el.click();
                  return s;
                }
              }
              return null;
            },
            args: [sels],
          })
          .catch((e) => [{ result: 'error: ' + e }]);
        await report({ action: 'click', url: cmd.url, clicked: clicked && clicked[0] && clicked[0].result });
        await waitComplete(tab.id, 30000);
        await sleep(cmd.wait_ms || 2000);
      }
      if (cmd.spa_video) {
        // YouTube's own in-app navigation (no page load): the page globals stay stale
        await browser.scripting.executeScript({
          target: { tabId: tab.id },
          world: 'MAIN',
          func: (id) => {
            const url = '/watch?v=' + id;
            document.querySelector('ytd-app').dispatchEvent(
              new CustomEvent('yt-navigate', {
                bubbles: true,
                composed: true,
                detail: { endpoint: { commandMetadata: { webCommandMetadata: { url, webPageType: 'WEB_PAGE_TYPE_WATCH', rootVe: 3832 } }, watchEndpoint: { videoId: id } } },
              }),
            );
          },
          args: [cmd.spa_video],
        });
        for (let i = 0; i < 40; i++) {
          await sleep(500);
          if ((await browser.tabs.get(tab.id)).url.includes(cmd.spa_video)) break;
        }
        await sleep(cmd.wait_ms || 2000);
      }
      // record whether the page globals belong to another video (stale after SPA navigation)
      const globals = await browser.scripting
        .executeScript({
          target: { tabId: tab.id },
          world: 'MAIN',
          func: () => {
            try {
              return window.ytInitialData.currentVideoEndpoint.watchEndpoint.videoId;
            } catch {
              return null;
            }
          },
        })
        .then((r) => r[0].result, () => null);
      const fresh = await browser.tabs.get(tab.id);
      const result = await globalThis.LiveNotesBackground.saveTab(fresh, cmd.selection);
      const badge = await browser.action.getBadgeText({ tabId: tab.id }).catch(() => null);
      const extraction = globalThis.LiveNotesBackground.lastExtraction();
      return report({ action: 'save', url: cmd.url, finalUrl: fresh.url, result, badge, extraction, ytInitialDataVideo: globals });
    }
    if (cmd.action === 'extract-youtube') {
      // run the YouTube extraction alone with some strategies disabled (nothing is sent)
      const tab = await browser.tabs.create({ url: cmd.url, active: true });
      await waitComplete(tab.id, 45000);
      await sleep(cmd.wait_ms || 4000);
      await browser.scripting.executeScript({
        target: { tabId: tab.id },
        world: 'MAIN',
        func: (skip) => {
          globalThis.__liveNotesTestSkipStrategies = skip;
        },
        args: [cmd.skip || []],
      });
      const res = await browser.scripting.executeScript({ target: { tabId: tab.id }, world: 'MAIN', files: ['lib/youtube.js', 'content/youtube-main.js'] });
      const r = res[0].result || {};
      const y = r.youtube || {};
      await browser.tabs.remove(tab.id);
      return report({ action: 'extract', url: cmd.url, attempts: r.attempts, source: y.transcript_source, lang: y.transcript_lang, segments: y.transcript ? y.transcript.length : null, first: y.transcript && y.transcript[0] });
    }
    if (cmd.action === 'probe-transcript-request') {
      // Diagnostics for when YouTube changes again: record which innertube endpoint its own
      // transcript panel calls and the response shape (no cookies or header values).
      const tab = await browser.tabs.create({ url: cmd.url, active: true });
      await waitComplete(tab.id, 45000);
      await sleep(cmd.wait_ms || 4000);
      const res = await browser.scripting.executeScript({
        target: { tabId: tab.id },
        world: 'MAIN',
        func: async () => {
          const seen = [];
          const shape = (o, d = 0) =>
            o && typeof o === 'object' && d < 3 ? Object.fromEntries(Object.entries(o).map(([k, v]) => [k, shape(v, d + 1)])) : typeof o;
          const origFetch = window.fetch;
          window.fetch = function (input, init) {
            try {
              const url = typeof input === 'string' ? input : input.url;
              if (/youtubei\/v1\/(get_panel|get_transcript)/.test(url)) {
                const hdrs = input instanceof Request ? [...input.headers.keys()] : Object.keys((init && init.headers) || {});
                const raw = input instanceof Request ? input.clone().blob() : Promise.resolve(new Blob([(init && init.body) || '']));
                const gz = hdrs.includes('content-encoding') || (init && init.headers && init.headers['Content-Encoding']);
                const p = raw.then((b) => (gz ? new Response(b.stream().pipeThrough(new DecompressionStream('gzip'))).text() : b.text()));
                const entry = { url: url.replace(/key=[^&]*/, 'key=…'), headers: hdrs };
                seen.push(entry);
                p.then((b) => {
                  try {
                    const body = JSON.parse(b);
                    entry.ctxKeys = Object.keys(body.context || {});
                    delete body.context;
                    entry.body = body;
                  } catch (e) {
                    entry.bodyError = String(e);
                  }
                });
                const r = origFetch.apply(this, arguments);
                r.then((res) => res.clone().text()).then((t) => {
                  entry.responseBytes = t.length;
                  const keys = new Set();
                  const walk = (o, d) => {
                    if (!o || typeof o !== 'object' || d > 40) return;
                    for (const [k, v] of Object.entries(o)) {
                      if (/segment|transcript|cue/i.test(k)) keys.add(k);
                      walk(v, d + 1);
                    }
                  };
                  try {
                    const j = JSON.parse(t);
                    walk(j, 0);
                    entry.responseKeys = [...keys].slice(0, 40);
                    const m = t.match(/"transcriptSegmentViewModel"|"transcriptSegmentRenderer"|"timedTextSegment/);
                    if (m) {
                      const i = t.indexOf('macroMarkersPanelItemViewModel');
                      entry.sample = t.slice(i, i + 2500);
                      const j = t.indexOf('sortFilterSubMenu');
                      entry.menu = j >= 0 ? t.slice(j, j + 1500) : null;
                      entry.topKeys = Object.keys(JSON.parse(t));
                    }
                  } catch (e) {
                    entry.respError = String(e);
                  }
                });
                return r;
              }
            } catch (e) {
              seen.push({ error: String(e) });
            }
            return origFetch.apply(this, arguments);
          };
          const origOpen = XMLHttpRequest.prototype.open;
          XMLHttpRequest.prototype.open = function (m, u) {
            if (String(u).includes('get_transcript')) seen.push({ via: 'xhr', url: String(u).replace(/key=[^&]*/, 'key=…') });
            return origOpen.apply(this, arguments);
          };
          const btn = document.querySelector('ytd-video-description-transcript-section-renderer button');
          const t0 = performance.now();
          if (btn) btn.click();
          await new Promise((r) => setTimeout(r, 5000));
          const resources = performance
            .getEntriesByType('resource')
            .filter((e) => e.startTime >= t0 && /youtubei|timedtext|transcript/.test(e.name))
            .map((e) => e.name.replace(/([?&](key|signature|pot|ei|sparams|expire)=)[^&]*/g, '$1…').slice(0, 300));
          const segTags = [...new Set([...document.querySelectorAll('[target-id] *')].map((e) => e.tagName.toLowerCase()).filter((t) => t.includes('transcript') || t.includes('segment')))];
          window.fetch = origFetch;
          XMLHttpRequest.prototype.open = origOpen;
          const legacy = document.querySelector('ytd-engagement-panel-section-list-renderer[target-id="engagement-panel-searchable-transcript"]');
          const legacyInfo = legacy && { visibility: legacy.getAttribute('visibility'), segments: legacy.querySelectorAll('ytd-transcript-segment-renderer').length, text: (legacy.innerText || '').slice(0, 300) };
          return { clicked: !!btn, btnCount: document.querySelectorAll('ytd-video-description-transcript-section-renderer button').length, legacyInfo, seen, resources, segTags, panels: [...document.querySelectorAll('ytd-engagement-panel-section-list-renderer')].map((p) => p.getAttribute('target-id') + ':' + p.getAttribute('visibility')) };
        },
      });
      return report({ action: 'probe', result: res[0] && res[0].result });
    }
    if (cmd.action === 'ping-options') {
      // exercise the options-page code path (LiveNotesApi.ping) from the background
      try {
        return report({ action: 'ping', ok: true, result: await globalThis.LiveNotesApi.ping(await globalThis.LiveNotesBackground.settings()) });
      } catch (e) {
        return report({ action: 'ping', ok: false, code: e.code });
      }
    }
    return report({ error: 'unknown action', cmd });
  }

  (async function loop() {
    for (;;) {
      try {
        const res = await fetch(CONTROL + 'next', { cache: 'no-store' });
        if (res.status === 200) {
          const cmd = await res.json();
          await run(cmd).catch((e) => report({ error: String(e && e.stack ? e.stack : e), cmd }));
          continue;
        }
      } catch {
        // mock not up yet
      }
      await sleep(1000);
    }
  })();
})();
