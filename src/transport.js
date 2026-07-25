// The chat transport seam (task 2.1).
//
// Desktop reaches the model over loopback HTTP: `llama-server` is a sidecar
// process, the tool loop runs here in JS (`calc-loop.js`), and tokens arrive as
// SSE. Android has no server to reach — 127.0.0.1 has nothing behind it and is
// CSP-blocked besides — so the same turn is an `invoke('chat_stream')` with
// tokens arriving on a Tauri `Channel`, and the tool loop runs Rust-side.
//
// Both are described here as the same two operations, so `app.js` asks for a
// TURN rather than for a fetch:
//
//   streamTurn(req) -> Promise<{ content, calculations }>   (tokens via onDelta)
//   complete(req)   -> Promise<string>
//
// # Desktop behaviour is byte-identical, on purpose
//
// The desktop implementations are the previous inline call sites moved, not
// rewritten: the same `streamWithTools` call with the same arguments, and the
// same `fetch` with the same body. This file is a seam, and a seam that also
// changes behaviour is two changes wearing one commit.
//
// # Everything is injected
//
// Nothing here reads a global. `createTransport` takes its dependencies, the
// same way `calc-loop.js` takes `fetchImpl` — which is what lets the choice
// itself be tested, including the one property that matters most: **a desktop
// transport never invokes the mobile commands.** The Rust-side desktop refusal
// (`DESKTOP_REFUSAL`) is a backstop for a bug, not the mechanism (decision
// D-1, tightening 2).

/// Build the transport for this platform. Called ONCE at startup; the result
/// is the only thing the rest of the app talks to.
///
/// `mobile` decides which half is live and is not re-evaluated per turn — a
/// transport that could change its mind mid-session is a transport that can be
/// wrong halfway through a conversation.
export function createTransport({
  mobile,
  invoke,
  Channel,
  fetchImpl = fetch,
  streamImpl,
  newRequestId = defaultRequestId,
}) {
  return Object.freeze(
    mobile
      ? mobileTransport({ invoke, Channel, newRequestId })
      : desktopTransport({ fetchImpl, streamImpl }),
  );
}

/// Is this build running on Android?
///
/// A deliberately dumb check, evaluated once. It is safe to be dumb here
/// because **the failure is loud in both directions**: a desktop build that
/// picked `mobile` invokes `chat_stream` and gets Rust's explicit refusal
/// naming the platform, and an Android build that picked desktop fetches
/// 127.0.0.1, where nothing is listening and the mobile CSP forbids it anyway.
/// Neither can be mistaken for working, which is the property that decides how
/// much a detection mechanism needs to be worth.
export function isAndroid(userAgent) {
  return /android/i.test(String(userAgent ?? ''));
}

/* ---------------- desktop: sidecar over loopback ---------------- */

function desktopTransport({ fetchImpl, streamImpl }) {
  return {
    kind: 'desktop',

    // Verbatim the previous `sendCompletion` call, with the URL built from the
    // sidecar port the caller passes in.
    streamTurn({ port, messages, tools, baseBody, runCalc, onDelta, signal }) {
      return streamImpl({
        url: `http://127.0.0.1:${port}/v1/chat/completions`,
        baseBody,
        messages,
        tools,
        runCalc,
        onContentDelta: onDelta,
        signal,
      });
    },

    // Verbatim the previous `maybeAutoTitle` fetch. Returns the raw assistant
    // text; every bit of title sanitising stays with the caller, which is where
    // it was and where it belongs.
    async complete({ port, messages, maxTokens, temperature, signal }) {
      const res = await fetchImpl(`http://127.0.0.1:${port}/v1/chat/completions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        signal,
        body: JSON.stringify({
          messages,
          stream: false,
          max_tokens: maxTokens,
          temperature,
          cache_prompt: false,
        }),
      });
      if (!res.ok) return null;
      const data = await res.json();
      return data.choices?.[0]?.message?.content ?? null;
    },
  };
}

/* ---------------- mobile: in-process engine over invoke ---------------- */

function mobileTransport({ invoke, Channel, newRequestId }) {
  return {
    kind: 'mobile',

    // `tools`, `baseBody` and `runCalc` are accepted and ignored: in-process,
    // the tool contract is injected into the prompt and the calc loop runs on
    // the inference thread, because every round needs the live session. The
    // caller passes them anyway so the two transports keep one signature.
    streamTurn({ chatId, messages, onDelta, signal }) {
      const requestId = newRequestId();
      const events = new Channel();

      let settle;
      const turn = new Promise((resolve, reject) => {
        settle = { resolve, reject };
      });

      events.onmessage = (ev) => {
        switch (ev?.event) {
          case 'delta':
            onDelta(ev.data.text);
            break;
          case 'done':
            settle.resolve({
              content: ev.data.content,
              calculations: ev.data.calculations ?? [],
            });
            break;
          case 'error':
            settle.reject(new Error(ev.data.message));
            break;
        }
      };

      // Cooperative cancel: the flag is checked per token, so Stop lands
      // mid-generation rather than at the end of a turn. The name matches
      // desktop's abort so `sendCompletion` keeps one code path.
      const onAbort = () => {
        void invoke('chat_cancel', { requestId });
      };
      signal?.addEventListener('abort', onAbort, { once: true });

      // Resolution comes from the `done` event, never from this promise:
      // `chat_stream` sends Done and only then returns Ok, so awaiting the
      // invoke as well would be racing the two IPC paths for no benefit. A
      // rejection here IS meaningful — it is the turn failing before it began
      // (engine not loaded, thread gone).
      invoke('chat_stream', { requestId, chatId, messages, onEvent: events }).catch((e) =>
        settle.reject(e instanceof Error ? e : new Error(String(e))),
      );

      return turn.finally(() => signal?.removeEventListener('abort', onAbort));
    },

    // No streaming, no cancel: the two callers are fire-and-forget. `chatId` is
    // deliberately NOT passed — a transient completion is not a continuation of
    // the chat it is about, and keying it to one would evict the KV prefix that
    // chat's next turn wants (task 1.5).
    async complete({ messages }) {
      return await invoke('chat_complete', { messages });
    },
  };
}

function defaultRequestId() {
  // `randomUUID` needs a secure context, which a Tauri WebView is, but the
  // fallback keeps this usable from a plain test runner.
  return globalThis.crypto?.randomUUID?.() ?? `req-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}
