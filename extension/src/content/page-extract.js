/*
 * Runs in the page (isolated world) after vendor/Readability.js. The last expression is the
 * result for scripting.executeScript.
 */
(() => {
  'use strict';
  const BLOCK = /^(ADDRESS|ARTICLE|ASIDE|BLOCKQUOTE|DD|DIV|DL|DT|FIGCAPTION|FIGURE|FOOTER|H[1-6]|HEADER|HR|LI|MAIN|NAV|OL|P|PRE|SECTION|TABLE|TR|UL)$/;

  /** Text of an element with paragraphs separated by blank lines. */
  function blockText(rootEl) {
    const parts = [];
    let cur = '';
    const flush = () => {
      const t = cur.replace(/[ \t\r\f\v ]+/g, ' ').replace(/ *\n */g, '\n').trim();
      if (t) parts.push(t);
      cur = '';
    };
    const walk = (node) => {
      for (const n of node.childNodes) {
        if (n.nodeType === 3) {
          cur += n.parentElement && n.parentElement.closest('pre') ? n.nodeValue : n.nodeValue.replace(/\s+/g, ' ');
        } else if (n.nodeType === 1) {
          const tag = n.tagName;
          if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'NOSCRIPT') continue;
          if (tag === 'BR') {
            cur += '\n';
            continue;
          }
          const block = BLOCK.test(tag);
          if (block) flush();
          walk(n);
          if (block) flush();
          else if (tag === 'TD' || tag === 'TH') cur += ' ';
        }
      }
    };
    walk(rootEl);
    flush();
    return parts.join('\n\n');
  }

  const selection = String(window.getSelection ? window.getSelection() : '').trim() || null;
  const meta = (sel) => {
    const el = document.querySelector(sel);
    return el ? (el.getAttribute('content') || '').trim() || null : null;
  };

  let article = null;
  try {
    // Readability mutates the document it gets, so give it a clone.
    article = new Readability(document.cloneNode(true)).parse();
  } catch (e) {
    article = null;
  }

  let text = '';
  let html = null;
  if (article && article.content) {
    const doc = new DOMParser().parseFromString(article.content, 'text/html');
    text = blockText(doc.body);
    html = article.content;
  }
  if (!text && article && article.textContent) text = article.textContent.trim();
  if (!text && document.body) text = blockText(document.body);

  return {
    url: location.href,
    title: (article && article.title) || document.title || location.href,
    site_name: (article && article.siteName) || meta('meta[property="og:site_name"]'),
    byline: (article && article.byline) || meta('meta[name="author"]'),
    lang: document.documentElement.lang || (article && article.lang) || null,
    text,
    html,
    selection,
    readability: !!article,
  };
})();
