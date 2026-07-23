// Self-contained completion + tool-call loop. NO Tauri, NO DOM — orchestration
// only; the math is `runCalc` (injected). This is the reference implementation
// the Rust EngineHandle port (mobile) transcribes; keep it that way.
export async function streamWithTools({
  url, baseBody, messages, tools, runCalc, onContentDelta,
  signal, maxRounds = 5, fetchImpl = fetch,
}) {
  const convo = [...messages];
  const calculations = [];
  let content = '';

  for (let round = 0; round < maxRounds; round++) {
    const res = await fetchImpl(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      signal,
      body: JSON.stringify({ ...baseBody, messages: convo, tools, stream: true }),
    });
    if (!res.ok) throw new Error(`engine returned ${res.status}`);

    const reader = res.body.getReader();
    const dec = new TextDecoder();
    let buf = '';
    const toolAcc = new Map(); // index -> { id, name, args }

    outer: while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let nl;
      while ((nl = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, nl).trim();
        buf = buf.slice(nl + 1);
        if (!line.startsWith('data: ')) continue;
        const data = line.slice(6);
        if (data === '[DONE]') break outer;
        let choice;
        try { choice = JSON.parse(data).choices?.[0]; } catch { continue; }
        if (!choice) continue;
        const d = choice.delta || {};
        if (d.content) { content += d.content; onContentDelta(d.content); }
        for (const tc of d.tool_calls || []) {
          const cur = toolAcc.get(tc.index) || { id: tc.id, name: '', args: '' };
          if (tc.id) cur.id = tc.id;
          if (tc.function?.name) cur.name = tc.function.name;
          if (tc.function?.arguments) cur.args += tc.function.arguments;
          toolAcc.set(tc.index, cur);
        }
      }
    }

    if (toolAcc.size === 0) {
      if (!content && calculations.length) content = 'Calculation complete.';
      return { content, calculations };
    }

    // Execute each tool call; append the assistant tool-call turn + tool results.
    const calls = [...toolAcc.entries()].sort((a, b) => a[0] - b[0]).map(([, v]) => v);
    convo.push({
      role: 'assistant',
      content: null,
      tool_calls: calls.map(c => ({ id: c.id, type: 'function', function: { name: c.name, arguments: c.args } })),
    });
    for (const c of calls) {
      let expression = '';
      try { expression = JSON.parse(c.args).expression ?? ''; } catch {}
      let result;
      try {
        const r = await runCalc(expression);
        calculations.push({ expression, display: r.display });
        result = r.display;
      } catch (e) {
        const msg = String(e.message || e);
        calculations.push({ expression, error: msg });
        result = msg; // error becomes the tool result so the model self-corrects
      }
      convo.push({ role: 'tool', tool_call_id: c.id, content: result });
    }
  }
  // Hit the round cap — return what we have rather than looping forever.
  if (!content && calculations.length) content = 'I reached the calculation limit for this turn.';
  return { content, calculations };
}
