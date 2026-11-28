import { EditorView, GutterMarker, drawSelection, keymap, lineNumberMarkers, lineNumbers } from '@codemirror/view';
import { RangeSet, StateEffect, StateField } from '@codemirror/state';
import { HighlightStyle, indentUnit, syntaxHighlighting } from '@codemirror/language';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { yaml } from '@codemirror/lang-yaml';
import { tags } from '@lezer/highlight';

const setBad = StateEffect.define();

class BadLine extends GutterMarker {
  elementClass = 'cm-bad-line';
}
const bad = new BadLine();

const badLines = StateField.define({
  create: () => RangeSet.empty,
  update(marks, tr) {
    for (const e of tr.effects) {
      if (e.is(setBad)) {
        const doc = tr.state.doc;
        return RangeSet.of(e.value.filter((n) => n >= 1 && n <= doc.lines).map((n) => bad.range(doc.line(n).from)));
      }
    }
    return marks.map(tr.changes);
  },
  provide: (f) => lineNumberMarkers.from(f),
});

const quiet = HighlightStyle.define([{ tag: tags.comment, color: 'var(--faint)' }]);

function open(host) {
  const field = document.getElementById('doc');
  const view = new EditorView({
    doc: field.value.replace(/\n$/, ''),
    parent: host,
    extensions: [
      lineNumbers(),
      history(),
      drawSelection(),
      indentUnit.of('  '),
      keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
      yaml(),
      syntaxHighlighting(quiet),
      badLines,
      EditorView.updateListener.of((u) => {
        if (!u.docChanged) return;
        field.value = u.state.doc.toString() + '\n';
        field.dispatchEvent(new Event('input', { bubbles: true }));
      }),
    ],
  });
  host.view = view;
}

function markLines() {
  const errs = document.getElementById('errs');
  const host = document.querySelector('.cm-host');
  if (!errs || !host || !host.view) return;
  const lines = (errs.dataset.lines || '').split(',').filter(Boolean).map(Number);
  host.view.dispatch({ effects: setBad.of(lines) });
}

function start(root) {
  root.querySelectorAll('.cm-host').forEach((h) => { if (!h.view) open(h); });
}

document.addEventListener('htmx:load', (e) => start(e.target));
document.addEventListener('htmx:afterSettle', markLines);
document.addEventListener('DOMContentLoaded', () => start(document));
