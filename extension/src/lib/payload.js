/*
 * Builds POST /v1/clip bodies (docs/clip-api.md). Pure functions; loaded as a classic script
 * in the background page and required from Node in tests.
 */
(function (root) {
  'use strict';

  // The app accepts 8 MB; stay clearly below so headers and JSON escaping never tip it over.
  const MAX_BYTES = 7 * 1024 * 1024;

  const nonEmpty = (v) => (typeof v === 'string' ? v.trim() || undefined : v == null ? undefined : v);

  function byteLength(s) {
    return new TextEncoder().encode(s).length;
  }

  /** Remove undefined keys (JSON.stringify does that too, but tests compare objects). */
  function compact(obj) {
    const out = {};
    for (const [k, v] of Object.entries(obj)) if (v !== undefined) out[k] = v;
    return out;
  }

  function pickSelection(extractedSelection, menuSelection) {
    return nonEmpty(menuSelection) || nonEmpty(extractedSelection);
  }

  /** extracted: result of content/page-extract.js; tab: {url, title}; menuSelection: menus info. */
  function buildPagePayload(extracted, tab = {}, menuSelection) {
    const e = extracted || {};
    return fitSize(
      compact({
        kind: 'page',
        url: e.url || tab.url,
        title: nonEmpty(e.title) || nonEmpty(tab.title) || e.url || tab.url,
        site_name: nonEmpty(e.site_name),
        byline: nonEmpty(e.byline),
        lang: nonEmpty(e.lang),
        text: e.text || '',
        html: nonEmpty(e.html),
        selection: pickSelection(e.selection, menuSelection),
      }),
    );
  }

  /** yt: result of content/youtube-main.js. */
  function buildYouTubePayload(yt, tab = {}, menuSelection) {
    const y = (yt && yt.youtube) || {};
    const transcript = Array.isArray(y.transcript) && y.transcript.length ? y.transcript : null;
    return fitSize(
      compact({
        kind: 'youtube',
        url: (yt && yt.url) || tab.url,
        title: nonEmpty(yt && yt.title) || nonEmpty(tab.title) || tab.url,
        site_name: 'YouTube',
        lang: nonEmpty(yt && yt.lang),
        // the transcript travels in youtube.transcript; text stays empty to avoid duplicates
        text: '',
        selection: nonEmpty(menuSelection),
        youtube: compact({
          video_id: y.video_id,
          channel: nonEmpty(y.channel),
          duration_s: Number.isFinite(y.duration_s) ? Math.round(y.duration_s) : undefined,
          description: nonEmpty(y.description),
          transcript_lang: nonEmpty(y.transcript_lang),
          transcript_source: transcript ? y.transcript_source : undefined,
          transcript,
        }),
      }),
    );
  }

  /** Minimal payload when the page could not be scripted (e.g. about:, PDF viewer, AMO). */
  function buildFallbackPayload(tab, menuSelection) {
    return compact({
      kind: 'page',
      url: tab.url,
      title: tab.title || tab.url,
      text: '',
      selection: nonEmpty(menuSelection),
    });
  }

  /** Keep the JSON body below MAX_BYTES: drop html first, then shorten text. */
  function fitSize(p, max = MAX_BYTES) {
    if (byteLength(JSON.stringify(p)) <= max) return p;
    if (p.html) {
      p = Object.assign({}, p);
      delete p.html;
      if (byteLength(JSON.stringify(p)) <= max) return p;
    }
    if (p.kind === 'youtube' && p.youtube && p.youtube.transcript) {
      // drop trailing segments until it fits (only ever relevant for very long streams)
      const segs = p.youtube.transcript.slice();
      while (segs.length && byteLength(JSON.stringify(p)) > max) {
        segs.splice(Math.floor(segs.length * 0.9));
        p = Object.assign({}, p, { youtube: Object.assign({}, p.youtube, { transcript: segs.slice() }) });
      }
      return p;
    }
    const text = p.text || '';
    const rest = byteLength(JSON.stringify(Object.assign({}, p, { text: '' })));
    // estimate chars to keep from the text's average bytes per char, then shrink until it fits
    const perChar = byteLength(JSON.stringify(text)) / Math.max(1, text.length);
    let keep = Math.max(0, Math.min(text.length, Math.floor((max - rest) / perChar)));
    let cut = Object.assign({}, p, { text: text.slice(0, keep) });
    while (keep > 0 && byteLength(JSON.stringify(cut)) > max) {
      keep = Math.floor(keep * 0.9);
      cut = Object.assign({}, p, { text: text.slice(0, keep) });
    }
    return cut;
  }

  const api = { buildPagePayload, buildYouTubePayload, buildFallbackPayload, fitSize, MAX_BYTES };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.LiveNotesPayload = api;
})(typeof globalThis !== 'undefined' ? globalThis : this);
