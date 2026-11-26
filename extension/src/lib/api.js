/*
 * Client for the Live Notes clip API (docs/clip-api.md). Classic script for the background
 * page and the options page; requires `fetch` (passed in for tests).
 */
(function (root) {
  'use strict';

  const DEFAULT_PORT = 47823;

  class ClipError extends Error {
    /** code: not_running | unauthorized | forbidden | too_large | disabled | not_found | http | bad_settings */
    constructor(code, message, status) {
      super(message || code);
      this.code = code;
      this.status = status;
    }
  }

  function baseUrl(settings) {
    const port = Number(settings && settings.port) || DEFAULT_PORT;
    if (!Number.isInteger(port) || port < 1 || port > 65535) throw new ClipError('bad_settings', 'invalid port');
    return 'http://127.0.0.1:' + port;
  }

  async function request(settings, method, path, body, fetchImpl) {
    const f = fetchImpl || root.fetch.bind(root);
    const token = String((settings && settings.token) || '').trim();
    if (!token) throw new ClipError('unauthorized', 'no token');
    let res;
    try {
      res = await f(baseUrl(settings) + path, {
        method,
        headers: Object.assign(
          { Authorization: 'Bearer ' + token, Accept: 'application/json' },
          body !== undefined ? { 'Content-Type': 'application/json' } : {},
        ),
        body: body !== undefined ? JSON.stringify(body) : undefined,
        cache: 'no-store',
      });
    } catch (e) {
      if (e instanceof ClipError) throw e;
      // fetch rejects with TypeError on connection refused / no listener
      throw new ClipError('not_running', String((e && e.message) || e));
    }
    let data = null;
    const txt = await res.text();
    try {
      data = txt ? JSON.parse(txt) : null;
    } catch {
      data = null;
    }
    if (res.ok) return data;
    const detail = (data && (data.error || data.message)) || txt.slice(0, 200);
    if (res.status === 401) throw new ClipError('unauthorized', detail, 401);
    if (res.status === 403) throw new ClipError('forbidden', detail, 403);
    if (res.status === 413) throw new ClipError('too_large', detail, 413);
    if (res.status === 404 && path.startsWith('/v1/clip/')) throw new ClipError('not_found', detail, 404);
    if (res.status === 404 || res.status === 503) throw new ClipError('disabled', detail, res.status);
    throw new ClipError('http', 'HTTP ' + res.status + (detail ? ': ' + detail : ''), res.status);
  }

  const ping = (s, f) => request(s, 'GET', '/v1/ping', undefined, f);
  const clip = (s, payload, f) => request(s, 'POST', '/v1/clip', payload, f);
  const job = (s, id, f) => request(s, 'GET', '/v1/clip/' + encodeURIComponent(id), undefined, f);

  const api = { DEFAULT_PORT, ClipError, ping, clip, job, baseUrl };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.LiveNotesApi = api;
})(typeof globalThis !== 'undefined' ? globalThis : this);
