// Mock of the Live Notes clip API (docs/clip-api.md) for tests and manual runs.
//
//   node test/mock-server.mjs [--port 47899] [--token test-token]
//
// Besides /v1/* it serves /__test/* (no auth) so the e2e test can queue commands for the
// test build's test-hook.js and read back what the "app" received.
import http from 'node:http';
import { pathToFileURL } from 'node:url';
import crypto from 'node:crypto';

const MAX_BODY = 8 * 1024 * 1024;

export function startMockServer({ port = 47899, token = 'test-token', jobDelayMs = 1200, log = () => {} } = {}) {
  const state = { clips: [], jobs: new Map(), requests: [], reports: [], queue: [] };

  const json = (res, status, obj) => {
    res.writeHead(status, { 'Content-Type': 'application/json' });
    res.end(obj === undefined ? '' : JSON.stringify(obj));
  };

  const server = http.createServer((req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const chunks = [];
    let size = 0;
    let tooLarge = false;
    req.on('data', (c) => {
      size += c.length;
      if (size > MAX_BODY) tooLarge = true;
      else chunks.push(c);
    });
    req.on('end', () => {
      const body = Buffer.concat(chunks).toString('utf8');

      // ---- test control channel ----
      if (url.pathname.startsWith('/__test/')) {
        if (url.pathname === '/__test/next') {
          const cmd = state.queue.shift();
          return cmd ? json(res, 200, cmd) : json(res, 204);
        }
        if (url.pathname === '/__test/report' && req.method === 'POST') {
          state.reports.push(JSON.parse(body || 'null'));
          log('report', body.slice(0, 500));
          return json(res, 204);
        }
        if (url.pathname === '/__test/enqueue' && req.method === 'POST') {
          state.queue.push(JSON.parse(body));
          return json(res, 204);
        }
        if (url.pathname === '/__test/state') return json(res, 200, { clips: state.clips, reports: state.reports, requests: state.requests });
        return json(res, 404, { error: 'unknown test route' });
      }

      // ---- API ----
      const origin = req.headers.origin;
      state.requests.push({ method: req.method, path: url.pathname, origin: origin ?? null, auth: req.headers.authorization ? 'present' : 'missing' });
      log(req.method, url.pathname, 'origin=' + origin);
      if (req.method === 'OPTIONS') {
        res.writeHead(204);
        return res.end();
      }
      if (origin && !/^(moz|chrome)-extension:\/\//.test(origin)) return json(res, 403, { error: 'origin not allowed' });
      if (req.headers.authorization !== 'Bearer ' + token) return json(res, 401, { error: 'unauthorized' });

      if (req.method === 'GET' && url.pathname === '/v1/ping') return json(res, 200, { app: 'live-notes', version: '0.0.0-mock', api: 1 });

      if (req.method === 'POST' && url.pathname === '/v1/clip') {
        if (tooLarge) return json(res, 413, { error: 'body too large' });
        let p;
        try {
          p = JSON.parse(body);
        } catch {
          return json(res, 400, { error: 'invalid json' });
        }
        if (!p || (p.kind !== 'page' && p.kind !== 'youtube') || !p.url) return json(res, 400, { error: 'kind and url required' });
        const id = crypto.randomUUID();
        state.clips.push({ job_id: id, bytes: size, payload: p });
        const job = { status: 'pending', stage: 'summarizing' };
        state.jobs.set(id, job);
        setTimeout(() => {
          Object.assign(job, { status: 'done', stage: 'done', title: p.title, path: `/tmp/mock-notes/${p.kind === 'youtube' ? 'YouTube' : 'Web'}/${p.title}.md` });
        }, jobDelayMs);
        return json(res, 202, { job_id: id });
      }

      const m = url.pathname.match(/^\/v1\/clip\/([^/]+)$/);
      if (req.method === 'GET' && m) {
        const job = state.jobs.get(decodeURIComponent(m[1]));
        return job ? json(res, 200, job) : json(res, 404, { error: 'unknown job' });
      }
      return json(res, 404, { error: 'not found' });
    });
  });

  return new Promise((resolve) => {
    server.listen(port, '127.0.0.1', () => resolve({ server, state, port, token, close: () => new Promise((r) => server.close(r)) }));
  });
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const arg = (name, def) => {
    const i = process.argv.indexOf('--' + name);
    return i > 0 ? process.argv[i + 1] : def;
  };
  const m = await startMockServer({ port: Number(arg('port', 47899)), token: arg('token', 'test-token'), log: (...a) => console.log(...a) });
  console.log(`mock Live Notes on http://127.0.0.1:${m.port} (token "${m.token}")`);
}
