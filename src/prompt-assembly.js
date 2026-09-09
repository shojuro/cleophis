// src/prompt-assembly.js — what the model is shown, in one place.
//
// Pulled out of app.js so it can be tested: the difference between a
// supervised (triage) entry and the tutor is precisely the difference
// between the context the triage model was GATED under and the context the
// tutor app grew. For a supervised entry the system content is the catalog's
// systemPrompt and nothing else, and the greeting is UI only.
export function assembleMessages({ entry, groundedPrompt, sent, ungroundedNote, fingerprint = null }) {
  if (entry && entry.supervised) {
    const system = String(entry.systemPrompt ?? '');
    if (fingerprint && entry.promptFingerprint && fingerprint(system) !== entry.promptFingerprint) {
      throw new Error(`prompt fingerprint mismatch: catalog says ${entry.promptFingerprint}, prompt is ${fingerprint(system)}`);
    }
    return { system, messages: [{ role: 'system', content: system }, ...sent] };
  }
  const system = groundedPrompt != null ? groundedPrompt : `${entry.systemPrompt}${ungroundedNote}`;
  return {
    system,
    messages: [
      { role: 'system', content: system },
      { role: 'assistant', content: entry.greeting },
      ...sent,
    ],
  };
}
