/* Live Notes, drawn live: a recording from the start button to the summary.
   Every frame is draw(t) on a 1200 × 750 stage (see step-window.js).

   References: docs/screenshots/08-recording.png, 04-recorded-note.png and the
   tour's transcript (target/verify/tour-data, recorded from
   target/test-corpus/mixed.wav with the real app). Labels: crates/ln-app/locales/de.ftl.
   Example data: the tour's seed notes ("Teamrunde Budget", "Angebot Shenzhen").
   Classes use the prefix `lnx-`. */
(function () {
  const { ease, cue, held, glide } = HTMLDemo;

  // ---- Real content -----------------------------------------------------------------------
  // The transcript the app wrote for the five-language test conversation (tour, 21.10.2026).
  const LINES = [
    { at: '00:00:00', who: 'Ich', me: true, lang: 'de', text: 'Können wir das Budget für das erste Quartal bis Freitag festlegen? Ich schicke dir morgen die Zahlen.' },
    { at: '00:00:08', who: 'Andere', lang: 'zh', text: '我们下周再讨论日算，我周五前把报告叫发给你，交货时间还需要确认。' },
    { at: '00:00:15', who: 'Ich', me: true, lang: 'en', text: 'Could you send me an updated offer by Friday? We still need to agree on the delivery time.' },
    { at: '00:00:22', who: 'Andere', lang: 'fr', text: 'Le budget du prochain trimestre sera discuté la semaine prochaine. Pouvez-vous m’envoyer le contrat ?' },
    { at: '00:00:29', who: 'Andere', lang: 'es', text: 'El presupuesto se discutirá la próxima semana. ¿Puedes enviarme la oferta antes del viernes?' },
  ];
  // The live suggestion the app showed for that conversation (query = the French section,
  // hit = the seed note "Teamrunde Budget").
  const CARD = {
    query: LINES[3].text,
    title: 'Teamrunde Budget',
    snippet: 'Zusammenfassung - Das Budget für Q1 wird nächste Woche final besprochen - Frau Li schickt das aktualisierte Angebot bis Freitag',
  };
  // Summary of this transcript. Replace with the app's own output once generated
  // (Gemma 4 or the ChatGPT plan); see site/README.md.
  const SUMMARY = {
    title: 'Budget Q1 und Angebot',
    points: [
      'Das Budget für das erste Quartal soll bis Freitag feststehen.',
      'Das Budget des nächsten Quartals wird nächste Woche besprochen.',
      'Die Lieferzeit ist noch offen.',
    ],
    tasks: ['Zahlen schicken — Ich, morgen', 'Aktualisiertes Angebot schicken — Andere, Freitag', 'Vertrag schicken — Andere'],
  };
  const NOTE_NAME = '2026-10-21 1901 Aufnahme';
  const LANGS = ['de', 'zh', 'en', 'fr', 'es'];

  // ---- Shared markup ------------------------------------------------------------------------
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
  const MIC =
    '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5 11a7 7 0 0 0 14 0M12 18v3"/></svg>';
  const SEARCH =
    '<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="11" cy="11" r="6.5"/><path d="m16 16 4 4"/></svg>';
  const FOLDER =
    '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"><path d="M3 7.5V18a1.5 1.5 0 0 0 1.5 1.5h15A1.5 1.5 0 0 0 21 18V9a1.5 1.5 0 0 0-1.5-1.5H12L10 5H4.5A1.5 1.5 0 0 0 3 6.5z"/></svg>';
  const GEAR =
    '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7"><circle cx="12" cy="12" r="3"/><path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3M5.3 5.3l2.1 2.1M16.6 16.6l2.1 2.1M5.3 18.7l2.1-2.1M16.6 7.4l2.1-2.1"/></svg>';
  const PAUSE = '<svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><rect x="7" y="5" width="3" height="14" rx="1"/><rect x="14" y="5" width="3" height="14" rx="1"/></svg>';
  const SPIN = '<span class="lnx-spin" data-spin></span>';

  /** Sidebar; parts that change carry data-* hooks. */
  function sidebar({ newNote = false } = {}) {
    return `<aside class="lnx-side">
      <div class="lnx-search">${SEARCH}<span>Suchen</span><kbd>⌘K</kbd></div>
      <div class="lnx-rec" data-rec data-aim="record">${MIC}<span class="lnx-recdot" data-recdot>●</span><span data-reclabel>Aufnahme starten</span></div>
      <div class="lnx-sechead"><b>Notizen</b><span>+</span></div>
      <div class="lnx-group">Heute</div>
      ${newNote ? `<div class="lnx-item" data-newnote><span data-newname>${NOTE_NAME}</span><em data-newstate></em></div>` : ''}
      <div class="lnx-item">Einkauf</div>
      <div class="lnx-group">Gestern</div>
      <div class="lnx-item">Teamrunde Budget<em>zh de</em></div>
      <div class="lnx-group">Projekte/</div>
      <div class="lnx-item">Angebot Shenzhen</div>
      <div class="lnx-foot">${FOLDER}<span>Quellen</span><i>${GEAR}</i></div>
    </aside>`;
  }

  function line(l, i) {
    return `<div class="lnx-line" data-line="${i}">
      <div class="lnx-meta"><span>${l.at}</span><b class="${l.me ? '' : 'lnx-other'}">${l.who}</b><span>${l.lang}</span></div>
      <div class="lnx-text">${esc(l.text)}</div>
    </div>`;
  }

  /** The recording view as in 08-recording.png. */
  function recordingView({ panel = false } = {}) {
    return `<section class="lnx-recview">
      <div class="lnx-col">
        <header class="lnx-rechead">
          <span class="lnx-reddot">●</span><b>Aufnahme läuft</b>
          <span class="lnx-timer" data-timer>00:00</span>
          <span class="lnx-connect" data-connect>${SPIN}Mikrofon und Systemaudio werden verbunden …</span>
          <span class="lnx-langs" data-langs></span>
        </header>
        <div class="lnx-mynotes">Meine Notizen …</div>
        <div class="lnx-lines">${LINES.map(line).join('')}</div>
        <footer class="lnx-recfoot">
          <span>Ich</span><span class="lnx-meter"><i data-meter-me></i></span>
          <span>Andere</span><span class="lnx-meter"><i data-meter-others></i></span>
          <span class="lnx-fill"></span>
          <span class="lnx-pause">${PAUSE}</span>
          <span class="lnx-stop" data-aim="stop" data-stop>Beenden</span>
        </footer>
      </div>
      ${
        panel
          ? `<aside class="lnx-panel">
        <div class="lnx-panelhead">Passend dazu</div>
        <div class="lnx-card" data-card>
          <div class="lnx-query">${esc(CARD.query)}</div>
          <div class="lnx-hit">${CARD.title}</div>
          <div class="lnx-snip">${esc(CARD.snippet)}</div>
          <div class="lnx-btns"><span class="lnx-btn" data-pin data-aim="pin">Anheften</span><span class="lnx-btn lnx-ghost">Verwerfen</span></div>
        </div>
      </aside>`
          : ''
      }
    </section>`;
  }

  /** The recorded note after stopping, as in 04-recorded-note.png. */
  function noteView() {
    return `<section class="lnx-note">
      <div class="lnx-notehead"><span>2026/10</span><span class="lnx-fill"></span><span>Bearbeiten</span></div>
      <div class="lnx-status" data-status>${SPIN}<span data-statustext></span></div>
      <div class="lnx-notebody">
        <div class="lnx-date">21.10.2026 19:01&nbsp;&nbsp; 00:00:40&nbsp;&nbsp; de · zh · en · fr · es</div>
        <div class="lnx-sum" data-sum>
          <h2>Zusammenfassung</h2>
          <ul>${SUMMARY.points.map((p) => `<li>${esc(p)}</li>`).join('')}</ul>
          <h2>Aufgaben</h2>
          <div class="lnx-tasks">${SUMMARY.tasks.map((p) => `<div><i></i>${esc(p)}</div>`).join('')}</div>
        </div>
        <div data-rest>
          <h2>Während des Gesprächs</h2>
          <ul><li>${esc(CARD.query)} — <u>${CARD.title}</u></li></ul>
          <h3>Transkript</h3>
          <div class="lnx-lines lnx-small">${LINES.map(line).join('')}</div>
        </div>
      </div>
    </section>`;
  }

  const stage = (side, main) => `<div class="lnx-app">${side}<main class="lnx-main">${main}</main></div>`;

  // ---- Small drawing helpers -------------------------------------------------------------------
  const show = (el, on) => el && (el.style.display = on ? '' : 'none');
  const fade = (el, k, dy = 6) => {
    if (!el) return;
    el.style.opacity = k;
    el.style.transform = `translateY(${(1 - k) * dy}px)`;
  };
  const mmss = (s) => `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(Math.floor(s % 60)).padStart(2, '0')}`;
  /** Level meters from t (deterministic, no random). */
  const level = (t, f) => 0.25 + 0.6 * Math.abs(Math.sin(t * f) * Math.sin(t * f * 0.37 + 1.3));
  function recording($, { connected, seconds, t, me = true, others = true }) {
    $('[data-rec]').classList.add('lnx-on');
    $('[data-reclabel]').textContent = `Aufnahme läuft  ${mmss(seconds)}`;
    show($('[data-connect]'), !connected);
    show($('[data-timer]'), connected);
    $('[data-timer]').textContent = mmss(seconds);
    $('[data-meter-me]').style.width = `${connected && me ? level(t, 7.1) * 100 : 0}%`;
    $('[data-meter-others]').style.width = `${connected && others ? level(t + 0.7, 5.3) * 100 : 0}%`;
  }
  /** Lines visible so far, newest fading in. */
  function lines($, times, t, sel = '.lnx-recview') {
    let langs = [];
    LINES.forEach((l, i) => {
      const k = times[i] === undefined ? 1 : ease(t, times[i], times[i] + 0.45);
      const el = $(`${sel} [data-line="${i}"]`);
      show(el, k > 0);
      fade(el, k);
      if (k > 0 && !langs.includes(l.lang)) langs.push(l.lang);
    });
    const ls = $('[data-langs]');
    if (ls) ls.textContent = langs.join(' · ');
  }

  // ---- Step 1: start ---------------------------------------------------------------------------
  const S1 = { CLICK: 1.75, VIEW: 1.85, CONNECTED: 3.4 };
  const recordBtn = [130, 61];
  const step1 = {
    id: 'start',
    name: 'Aufnahme starten',
    dur: 5,
    html: stage(sidebar({ newNote: true }), `<div class="lnx-empty" data-empty>Wähle links eine Notiz aus.</div>${recordingView()}`),
    draw(t, $) {
      const on = t >= S1.VIEW;
      show($('[data-empty]'), !on);
      show($('.lnx-recview'), on);
      show($('[data-newnote]'), on);
      $('[data-rec]').classList.toggle('lnx-press', held(t, S1.CLICK));
      if (on) {
        const connected = t >= S1.CONNECTED;
        recording($, { connected, seconds: Math.max(0, t - S1.CONNECTED) * 1.0, t });
        $('[data-newstate]').innerHTML = connected ? '' : `${SPIN}verbindet …`;
        $('[data-newnote]').classList.add('lnx-sel');
        lines($, [99, 99, 99, 99, 99], t);
      } else {
        $('[data-rec]').classList.remove('lnx-on');
        $('[data-reclabel]').textContent = 'Aufnahme starten';
      }
      const c = glide(t, [[0, 0, 760, 520], [0.6, 1.4, ...recordBtn], [3.9, 4.6, 640, 560]], [S1.CLICK]);
      return { cursor: { ...c, hide: t > 4.4 } };
    },
  };

  // ---- Step 2: live transcript -------------------------------------------------------------------
  // Each line appears a little after its section ended, as the app shows it (2 s sections).
  const S2 = { TIMES: [0.5, 1.9, 3.3, 4.7, 6.1], DUR: 8 };
  const step2 = {
    id: 'live',
    name: 'Live-Transkript',
    dur: S2.DUR,
    html: stage(sidebar({ newNote: true }), recordingView()),
    draw(t, $) {
      // 38 s of conversation in 8 s of demo (4.75×); the clock shows real time.
      recording($, { connected: true, seconds: 1 + t * 4.75, t });
      $('[data-newnote]').classList.add('lnx-sel');
      lines($, S2.TIMES, t);
      return {};
    },
  };

  // ---- Step 3: suggestion ----------------------------------------------------------------------
  const S3 = { CARD: 0.7, PIN: 3.6 };
  const step3 = {
    id: 'related',
    name: 'Passend dazu',
    dur: 6,
    html: stage(sidebar({ newNote: true }), recordingView({ panel: true })),
    draw(t, $) {
      recording($, { connected: true, seconds: 39 + t, t });
      $('[data-newnote]').classList.add('lnx-sel');
      lines($, [], t);
      const k = ease(t, S3.CARD, S3.CARD + 0.5);
      const gone = ease(t, S3.PIN + 0.15, S3.PIN + 0.5);
      fade($('[data-card]'), k * (1 - gone), 10);
      $('[data-pin]').classList.toggle('lnx-press', held(t, S3.PIN));
      const c = glide(t, [[0, 0, 700, 600], [2.4, 3.2, 950, 266]], [S3.PIN]);
      return { cursor: { ...c, hide: t > 4.6 } };
    },
  };

  // ---- Step 4: stop and summary ------------------------------------------------------------------
  const S4 = { STOP: 1.2, NOTE: 1.3, SUMMARIZING: 2.3, DONE: 4.6 };
  const step4 = {
    id: 'summary',
    name: 'Zusammenfassung',
    dur: 7.5,
    html: stage(sidebar({ newNote: true }), recordingView() + noteView()),
    draw(t, $) {
      const note = t >= S4.NOTE;
      show($('.lnx-recview'), !note);
      show($('.lnx-note'), note);
      $('[data-newnote]').classList.add('lnx-sel');
      if (!note) {
        recording($, { connected: true, seconds: 45 + t, t });
        lines($, [], t, '.lnx-recview');
        $('[data-stop]').classList.toggle('lnx-press', held(t, S4.STOP));
      } else {
        $('[data-rec]').classList.remove('lnx-on');
        $('[data-reclabel]').textContent = 'Aufnahme starten';
      }
      const phase = cue(t, [[S4.NOTE, 'finishing'], [S4.SUMMARIZING, 'summarizing'], [S4.DONE, 'done']], 'rec');
      const text = { finishing: 'Transkript wird fertiggestellt …', summarizing: 'Zusammenfassung wird erstellt …' }[phase] || '';
      show($('[data-status]'), note && phase !== 'done');
      $('[data-statustext]').textContent = text;
      $('[data-newstate]').innerHTML =
        phase === 'finishing' ? `${SPIN}transkribiert …` : phase === 'summarizing' ? `${SPIN}fasst zusammen …` : '';
      // The summary draws in from the top, then the file is renamed after its title.
      const k = ease(t, S4.DONE, S4.DONE + 0.9);
      const sum = $('[data-sum]');
      sum.style.clipPath = `inset(0 0 ${(1 - k) * 100}% 0)`;
      sum.style.maxHeight = `${k * 340}px`;
      sum.style.opacity = 0.3 + 0.7 * k;
      $('[data-newname]').textContent = t >= S4.DONE + 0.6 ? SUMMARY.title : NOTE_NAME;
      const c = glide(t, [[0, 0, 640, 560], [0.3, 0.9, 795, 702]], [S4.STOP]);
      return { cursor: { ...c, hide: t > 2.2 } };
    },
  };

  const STEPS = [step1, step2, step3, step4];
  // Spinners: a pure function of the page clock is not needed for a loading indicator, but it must
  // stop under reduced motion (CSS below).
  window.LiveNotesDemo = {
    mount(el) {
      return HTMLDemo.mount(el, { steps: STEPS, title: 'Live Notes', label: 'Live Notes nimmt ein Gespräch auf', stageClass: 'lnx' });
    },
  };
})();
