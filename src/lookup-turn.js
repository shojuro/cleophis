// src/lookup-turn.js — the reference LOOKUP turn, out of the DOM (Phase 1h M6).
//
// A health worker on the supervised screen can look up a drug or condition in
// the signed reference pack bundled with the triage build. The model may state
// a dose only when it is cited from that pack. This module owns the turn's
// DECISIONS, with `invoke` and the model injected, so `node --test src/` runs
// every branch; `app.js` is left with the DOM and the persistence calls.
//
// The decision order, and each step's reason:
//   1. CRISIS FIRST, on the user's own words, before any retrieval: a
//      disclosure gets the product's fixed block (the founder's replace rule,
//      with the entry's crisis line). No retrieval, no model.
//   2. No reference pack on the entry: a plain message. No retrieval, no model.
//   3. `rag_lookup` (lexical only). noEvidence -> the scripted refusal;
//      didYouMean -> the list of close titles; unavailable, an error, or a
//      status nobody knows -> the plain message. None of these runs the model.
//   4. grounded -> the budget check (a hard error when over), then the model
//      with the Rust-assembled prompt and the query as its single user turn,
//      then the LOOKUP guard (`applyLookupGuard`) — never `applyGuard`.
import { detectCrisisStatement } from './triage/detectors.mjs';
import { CRISIS_BLOCK_DEFAULT, crisisReplaceBlock } from './triage/guard.js';
import {
  LOOKUP_NO_EVIDENCE_TEXT, LOOKUP_RULE, applyLookupGuard,
} from './triage/lookup-guard.js';
import pin from './triage/detectors.pin.js';
import { LookupBudgetError, assembleLookupMessages } from './prompt-assembly.js';

export const LOOKUP_UNAVAILABLE_TEXT =
  'The reference pack is not available on this device, so nothing was looked up.';
export const LOOKUP_OVER_BUDGET_TEXT =
  'This reference entry is too long to check on this device, so no answer is given. Ask a pharmacist or clinician.';

/** The did-you-mean reply: close page titles, scripted, no model. */
export function didYouMeanText(candidates) {
  return `No exact entry for that name. Did you mean: ${candidates.join('; ')}? Look it up again using one of these names.`;
}

/**
 * The reference pack this entry's lookup reads, or null. Only a supervised
 * entry has a lookup; an entry whose catalog record names no pack (this round,
 * until M4b signs one) has none, and `app.js` HIDES the lookup entry for it.
 */
export function referencePackId(entry) {
  if (!entry || entry.supervised !== true) return null;
  const id = entry.referencePack && entry.referencePack.id;
  return typeof id === 'string' && id.trim() ? id : null;
}

// ── Recovering the source texts ─────────────────────────────────────────────
//
// `rag_lookup`'s citations carry title, section path and locator but not the
// chunk text, and the guard needs the text for its verbatim dose check. The
// text is in the prompt Rust assembled, one line per source rendered with the
// contract's `citation_line`, so it is recovered from there — with the
// template pinned to the contract TOML by a test, never trusted by eye. Any
// source that cannot be placed exactly empties EVERY text, and an empty text
// fails every dose cited to it: the failure withholds, it never passes.

/** The triage contract's `citation_line` and `citation_source_sep` (test-pinned to the TOML). */
export const CITATION_LINE_TEMPLATE = '[{n}] ({source}): {text}';
export const CITATION_SOURCE_SEP = ', ';

function sourceHead(c) {
  const source = [c.docTitle, c.sectionPath, c.locator]
    .filter((p) => typeof p === 'string' && p !== '')
    .join(CITATION_SOURCE_SEP);
  const [before] = CITATION_LINE_TEMPLATE.split('{text}');
  return before.replace('{n}', String(c.n)).replace('{source}', source);
}

/**
 * `[{ ...citation, text }]` in citation order, each text exactly as rendered.
 */
export function sourcesFromPrompt(prompt, citations) {
  const list = Array.isArray(citations) ? citations : [];
  const empty = () => list.map((c) => ({ ...c, text: '' }));
  if (typeof prompt !== 'string') return empty();
  const starts = [];
  let from = 0;
  for (const c of list) {
    const head = `\n${sourceHead(c)}`;
    const at = prompt.indexOf(head, from);
    if (at === -1) return empty();
    starts.push({ at, textAt: at + head.length });
    from = at + head.length;
  }
  return list.map((c, i) => ({
    ...c,
    text: prompt.slice(starts[i].textAt, i + 1 < starts.length ? starts[i + 1].at : prompt.length),
  }));
}

// ── The turn ────────────────────────────────────────────────────────────────

function scripted(outcome, displayText, extra = {}) {
  return {
    outcome,
    displayText,
    modelCalled: false,
    messageId: null,
    citations: [],
    ...(extra.candidates ? { candidates: extra.candidates } : {}),
    verdict: {
      kind: 'lookup',
      rule: LOOKUP_RULE,
      outcome,
      displayText,
      rawReply: null,
      kept: [],
      withheld: [],
      citations: [],
      detectorsSha: pin.sha256,
      sources: [],
      ...extra,
    },
  };
}

// `url` and `retrievedAt` are the field names this app expects on a
// `rag_lookup` citation once M4b's pack build carries them (camelCase over IPC,
// so `url` / `retrieved_at` Rust-side). Kept on the persisted source record as
// null when absent, so every record has one shape.
const optString = (v) => (typeof v === 'string' && v.trim() ? v : null);
const sourceRecord = (c) => ({
  n: c.n, packId: c.packId, chunkId: c.chunkId, docTitle: c.docTitle, sectionPath: c.sectionPath, locator: c.locator,
  url: optString(c.url), retrievedAt: optString(c.retrievedAt),
});

// ── How a lookup row shows its sources (controller ruling, from M4a's NHS
// reuse-terms finding) ──────────────────────────────────────────────────────

/**
 * The fixed footer under every GROUNDED lookup reply. One constant: the
 * founder may reword it here and nowhere else. Never under the scripted
 * refusal, the did-you-mean list, the unavailable message or the crisis block.
 */
export const LOOKUP_SOURCE_FOOTER =
  "Reference pages: NHS website, Open Government Licence v3.0. The wording above is the assistant's, not the NHS's.";

/** The footer for a lookup outcome, or null. */
export function lookupFooterFor(outcome) {
  return outcome === 'grounded' ? LOOKUP_SOURCE_FOOTER : null;
}

// Only an https URL is ever a link: a pack is signed, but the renderer does not
// lean on that for what it lets a tap open.
function httpsUrl(v) {
  if (typeof v !== 'string') return null;
  try {
    const u = new URL(v.trim());
    return u.protocol === 'https:' && u.hostname ? u.href : null;
  } catch (_) {
    return null;
  }
}

/**
 * One rendered citation: title, section path, the source URL as a link (the
 * citation's `url`, else a `locator` that is itself an https URL) and
 * "as at <retrievedAt>" when the source carries a retrieval date. What the
 * source does not carry is null, and the renderer leaves it out.
 */
export function lookupCitationRow(c) {
  const date = optString(c && c.retrievedAt);
  return {
    n: c.n,
    title: optString(c.docTitle) ?? '',
    sectionPath: optString(c.sectionPath),
    url: httpsUrl(c.url) ?? httpsUrl(c.locator),
    asAt: date ? `as at ${date}` : null,
  };
}

/**
 * Run one lookup turn.
 *
 * @param {object} p
 * @param {string} p.text      the user's query
 * @param {object} p.entry     the catalog entry the turn is SENT under
 * @param {Function} p.invoke  Tauri's invoke (mocked in tests)
 * @param {Function} p.generate ({system, messages}) -> Promise<{content, messageId}>;
 *   `content` already stripped of any leading empty think block.
 * @returns {Promise<{outcome: string, displayText: string, verdict: object|null,
 *   citations: object[], modelCalled: boolean, messageId: number|null,
 *   candidates?: string[], error?: string}>}
 *   `verdict` is null only for `overBudget`, which is an error and is not persisted.
 */
export async function runLookupTurn({ text, entry, invoke, generate }) {
  const query = String(text ?? '').trim();

  if (detectCrisisStatement(query).found) {
    const line = (entry && entry.crisisLine) || CRISIS_BLOCK_DEFAULT;
    return scripted('crisis', crisisReplaceBlock(line), { crisisOnInput: true });
  }

  const packId = referencePackId(entry);
  if (!packId) return scripted('unavailable', LOOKUP_UNAVAILABLE_TEXT);

  let rag;
  try {
    rag = await invoke('rag_lookup', { query, packId });
  } catch (e) {
    return scripted('unavailable', LOOKUP_UNAVAILABLE_TEXT, { error: String(e?.message ?? e) });
  }

  const status = rag && rag.status;
  if (status === 'noEvidence') return scripted('noEvidence', LOOKUP_NO_EVIDENCE_TEXT);
  if (status === 'didYouMean') {
    const candidates = (Array.isArray(rag.candidates) ? rag.candidates : [])
      .filter((c) => typeof c === 'string' && c.trim());
    return candidates.length
      ? scripted('didYouMean', didYouMeanText(candidates), { candidates })
      : scripted('noEvidence', LOOKUP_NO_EVIDENCE_TEXT);
  }
  if (status !== 'grounded') return scripted('unavailable', LOOKUP_UNAVAILABLE_TEXT);

  const citations = Array.isArray(rag.citations) ? rag.citations : [];
  if (!citations.length) return scripted('noEvidence', LOOKUP_NO_EVIDENCE_TEXT);

  let assembled;
  try {
    assembled = assembleLookupMessages({ groundedPrompt: rag.prompt, query });
  } catch (e) {
    if (e instanceof LookupBudgetError) {
      return {
        outcome: 'overBudget', displayText: LOOKUP_OVER_BUDGET_TEXT, verdict: null,
        citations: [], modelCalled: false, messageId: null, error: e.message,
      };
    }
    return scripted('unavailable', LOOKUP_UNAVAILABLE_TEXT, { error: String(e?.message ?? e) });
  }

  const sources = sourcesFromPrompt(rag.prompt, citations);
  const reply = await generate({ system: assembled.system, messages: assembled.messages });
  const verdict = {
    ...applyLookupGuard({ replyText: reply && reply.content, sources }),
    sources: citations.map(sourceRecord),
  };
  return {
    outcome: verdict.outcome,
    displayText: verdict.displayText,
    verdict,
    citations: citations.filter((c) => verdict.citations.includes(c.n)),
    modelCalled: true,
    messageId: (reply && reply.messageId) ?? null,
  };
}

/**
 * The lookup verdict as PERSISTED: model and pack provenance by sha, stamped
 * on a copy from the entry the turn was sent under (the same reason
 * `guardForPersistence` stamps the triage verdict). No prompt fingerprint:
 * the lookup's prompt is the Rust-assembled grounded one, not the catalog's.
 */
export function lookupForPersistence(verdict, entry = null) {
  if (!verdict) return null;
  const str = (v) => (typeof v === 'string' ? v : '');
  const pack = (entry && entry.referencePack) || {};
  return {
    ...verdict,
    modelSha: str(entry && entry.sha256),
    adapterSha: str(entry && entry.adapterSha256),
    referencePack: {
      id: str(pack.id), sha256: str(pack.sha256), contentSha256: str(pack.contentSha256), version: str(pack.version),
    },
  };
}
