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
    if (fingerprint) {
      // A supervised entry that pins NO fingerprint is not an entry that
      // passes the check — it is one that cannot be checked, and a prompt
      // nobody can check is not the prompt the gate was run under. Loud, in
      // the same shape as a mismatch, so the send path surfaces both the same
      // way and neither can become a silent send.
      if (!entry.promptFingerprint) {
        throw new Error('prompt fingerprint missing: this supervised entry pins none, so the prompt cannot be checked');
      }
      if (fingerprint(system) !== entry.promptFingerprint) {
        throw new Error(`prompt fingerprint mismatch: catalog says ${entry.promptFingerprint}, prompt is ${fingerprint(system)}`);
      }
    }
    return { system, messages: [{ role: 'system', content: system }, ...wire(sent)] };
  }
  const system = groundedPrompt != null ? groundedPrompt : `${entry.systemPrompt}${ungroundedNote}`;
  return {
    system,
    messages: [
      { role: 'system', content: system },
      { role: 'assistant', content: entry.greeting },
      ...wire(sent),
    ],
  };
}

// WHAT GOES ON THE WIRE, and nothing else. `sent` is a window over the app's
// OWN message objects — the same ones `state.chat.messages` holds — and those
// carry rendering and bookkeeping state that has no business in a prompt: the
// row `id`, replayed `citations` and `calculations`, and on a supervised turn
// the entire guard verdict, `rawReply` included. Spreading them put all of it
// into `chat_stream`'s payload and into the desktop sidecar's request body:
// a different wire shape than before any of this existed, and a supervised
// turn re-sending every earlier RAW reply to the model.
//
// So the history is projected, once, here — the one place both platforms and
// both entry kinds pass through. Anything a future field needs to reach the
// model must be added deliberately, which is the point.
function wire(sent) {
  return (sent ?? []).map((msg) => ({ role: msg.role, content: msg.content }));
}
