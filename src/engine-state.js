// What the local engine is doing, said out loud (task 2.2).
//
// # Why this module exists at all
//
// Before it, `#chatStatusPill` appeared exactly once in the whole codebase —
// in `index.html`, as the hard-coded string "Local · offline", permanently
// teal. Nothing read it and nothing wrote it. So the answer to CP0's
// "the founder could not identify the engine-state element" is that there was
// no engine-state element: the pill was decoration shaped like a status. The
// only thing that ever reported engine state was `#engineBanner`, which fires
// on `engine-restarting` and `engine-failed` and nothing else — meaning the
// ordinary path (Starting → Ready) rendered *nothing at all*.
//
// That also explains the copy defect recorded in the milestone: a permanently
// green "Local · offline" next to a model that is not on disk reads as
// "running fine, locally", which is how someone comes to believe a download
// happened when none did.
//
// # Why the logic lives here rather than in app.js
//
// Decision D-3: logic whose failure mode is silent goes where the tests run.
// A status line that says the wrong thing produces no error, throws nothing,
// and looks exactly like a status line that says the right thing — so the
// mapping and the sequencing are pure functions with injected time, and
// `app.js` keeps only the DOM writes.
//
// # The two halves
//
//   describeEngineState(facts) -> presentation     — what is true right now
//   createReadableSequence(...)                    — shown long enough to read

/// Minimum time a state stays on screen before the next one may replace it.
///
/// This is a product decision, not a tuning constant. The milestone's finding:
/// "this product's pitch is that the model is yours and on your device, and
/// watching it be verified and loaded is exactly the moment that claim becomes
/// credible. Fast is the wrong optimization for the one transition worth
/// showing." Transitions that flash past spend that moment in silence.
export const MIN_DWELL_MS = 700;

/// How far behind reality the *display* may fall while catching up on a queue.
///
/// The dwell buys readability; without a bound it would also buy staleness,
/// and a status line describing something that stopped being true three
/// seconds ago is worse than a skipped beat.
export const MAX_LAG_MS = 2500;

/// How long a turn may go without its first token before we explain the wait.
///
/// Derived from a real signal — a request is in flight and no delta has
/// arrived — rather than from a guess about what the engine is doing. That is
/// prefill, and on the reference device the first turn of a conversation is
/// genuinely slow (CP1: "first turn very slow, subsequent turns lightning
/// fast"). A user who is shown nothing here concludes the app is broken.
export const PREFILL_EXPLAIN_MS = 1200;

const pct = (done, total) => (total > 0 ? Math.min(100, Math.floor((done / total) * 100)) : 0);

/// Format a byte count, choosing the unit from `basis` rather than from the
/// value itself. Both halves of "385 MB of 770 MB" must use one unit: picking
/// per-value straddles the 1 GB boundary mid-download and makes the number
/// appear to shrink as it grows.
function bytesIn(value, basis = value) {
  if (basis / 2 ** 30 >= 1) return (value / 2 ** 30).toFixed(2) + ' GB';
  return Math.round(value / 2 ** 20) + ' MB';
}

function rate(bytesPerSec) {
  if (!bytesPerSec) return null;
  const m = bytesPerSec / 2 ** 20;
  return (m >= 10 ? Math.round(m) : m.toFixed(1)) + ' MB/s';
}

/// Describe the engine's current state for a human.
///
/// Every branch is derived from something the backend actually reports —
/// `EngineStatus` (Starting|Ready|Restarting|Failed|NoModel), the
/// `download-progress` phase (requesting|downloading|verifying|done|failed|
/// cancelled), whether the hero model is installed, and the timing of the
/// in-flight turn. Nothing here guesses.
///
/// Returns `{ kind, label, short, detail, tone, prominent }`:
///   kind       stable identifier — CSS hook and the thing tests assert on
///   label      the headline for the full-width row
///   short      the chat-bar pill's version. A separate field because the two
///              have genuinely different budgets: the row has the width of the
///              screen, the pill has ~170px beside a model name. Writing one
///              string for both is what produced "Ready — r" on a 360px
///              device — the element was inside the viewport (so the
///              measurement passed) and the text inside it was still cut.
///   detail     one honest sentence, or null
///   tone       'busy' | 'ready' | 'warn' | 'error'
///   prominent  true when this deserves the full-width row rather than the
///              compact pill. Prominence is how a first-time user finds the
///              element at all: it is loud exactly when something is happening
///              and quiet once there is nothing to say.
///   action     'download' | 'resume' | undefined — a NAME, not a handler, so
///              this module stays free of the DOM and of app.js's state. The
///              renderer maps the name to a click.
export function describeEngineState({
  engineStatus = null,
  download = null,
  installed = false,
  partBytes = 0,
  downloadActive = false,
  turn = null,
  supported = true,
} = {}) {
  // Unsupported hardware outranks everything: no amount of engine state is
  // meaningful on a device that cannot run a model (2.3's supported=false).
  if (!supported) {
    return {
      kind: 'unsupported',
      label: 'This device cannot run a model yet',
      short: 'Not supported',
      detail: 'Cleophis needs about 4 GB of memory. Your chats and library still work.',
      tone: 'warn',
      prominent: true,
    };
  }

  // A live download is the loudest true thing and the most informative, so it
  // outranks engine state — the engine being NoModel *during* a download is
  // not news, it is the reason for the download.
  if (download && download.phase) {
    const p = download.phase;
    if (p === 'requesting') {
      return {
        kind: 'download-starting',
        label: 'Starting the download',
        short: 'Starting…',
        detail: 'Asking for your copy of the model.',
        tone: 'busy',
        prominent: true,
      };
    }
    if (p === 'downloading') {
      const done = download.bytesDownloaded || 0;
      const total = download.totalBytes || 0;
      const r = rate(download.bytesPerSec);
      return {
        kind: 'downloading',
        label: `Downloading your model — ${pct(done, total)}%`,
        short: `${pct(done, total)}%`,
        detail: total
          ? `${bytesIn(done, total)} of ${bytesIn(total)}${r ? ` · ${r}` : ''}`
          : 'Fetching the model file.',
        tone: 'busy',
        prominent: true,
      };
    }
    if (p === 'verifying') {
      // The trust moment. This is the step that makes "the model is yours and
      // on your device" a checkable claim rather than a slogan, so it is named
      // plainly instead of being folded into a generic spinner.
      return {
        kind: 'verifying',
        label: 'Checking the download',
        short: 'Verifying',
        detail: 'Confirming every byte matches the publisher’s hash.',
        tone: 'busy',
        prominent: true,
      };
    }
    if (p === 'failed') {
      return {
        kind: 'download-failed',
        label: 'The download did not finish',
        short: 'Download failed',
        detail: download.error || 'You can pick up where it stopped.',
        tone: 'error',
        prominent: true,
        action: 'resume',
      };
    }
    if (p === 'cancelled') {
      return {
        kind: 'download-paused',
        label: 'Download paused',
        short: 'Paused',
        detail: 'The part you already have is kept — resuming continues from there.',
        tone: 'warn',
        prominent: true,
        action: 'resume',
      };
    }
    // 'done' falls through to engine state: the bytes are here, and what
    // matters next is whether the engine picked them up.
  }

  // A download that stopped and left bytes behind. Distinct from "no model"
  // because the user already started, and distinct from a live download
  // because nothing is happening right now — without this state the app looks
  // identical to a fresh install, silently discarding the fact that most of a
  // multi-gigabyte file is already on disk.
  //
  // `download_status` has always reported `partBytes`, and the drawer has
  // always offered "Resume download · X of Y GiB" — but only inside the
  // model's drawer, which a user in the chat view never opens.
  if (!installed && partBytes > 0 && !downloadActive) {
    return {
      kind: 'download-interrupted',
      label: 'Download stopped partway',
      short: 'Paused',
      detail: `${bytesIn(partBytes)} already saved — resuming picks up from there.`,
      tone: 'warn',
      prominent: true,
      action: 'resume',
    };
  }

  if (engineStatus === 'Failed') {
    return {
      kind: 'failed',
      label: 'The engine could not start',
      short: 'Engine failed',
      detail: 'Your chats are safe. Re-downloading the model usually fixes this.',
      tone: 'error',
      prominent: true,
    };
  }

  // Said before any "loading" state, because a device with no model is not
  // loading anything and must never look like it is. This is the copy defect
  // the milestone recorded, stated as its opposite.
  if (engineStatus === 'NoModel' || !installed) {
    return {
      kind: 'no-model',
      label: 'No model on this device yet',
      short: 'No model yet',
      detail: 'Download one to start chatting — it runs entirely offline afterwards.',
      tone: 'warn',
      prominent: true,
      action: 'download',
    };
  }

  if (engineStatus === 'Restarting') {
    return {
      kind: 'restarting',
      label: 'Restarting the engine',
      short: 'Restarting',
      detail: 'Loading the model again.',
      tone: 'busy',
      prominent: true,
    };
  }

  if (engineStatus === 'Starting' || engineStatus == null) {
    return {
      kind: 'loading',
      label: 'Loading your model',
      short: 'Loading model',
      detail: 'Reading it into memory. This takes longest the first time.',
      tone: 'busy',
      prominent: true,
    };
  }

  // Ready, and a turn is in flight. `turn.firstDeltaAt` is the real signal:
  // until the first token arrives the engine is prefilling — reading the
  // conversation so far — and on the reference device that is the slow part.
  if (turn && turn.startedAt != null && turn.firstDeltaAt == null) {
    const waited = (turn.now ?? turn.startedAt) - turn.startedAt;
    if (waited >= PREFILL_EXPLAIN_MS) {
      return {
        kind: 'preparing',
        label: 'Reading your conversation',
        short: 'Reading…',
        detail: 'The first reply in a chat takes longest — later ones are much faster.',
        tone: 'busy',
        prominent: true,
      };
    }
    return { kind: 'working', label: 'Thinking', short: 'Thinking', detail: null, tone: 'busy', prominent: false };
  }

  if (turn && turn.firstDeltaAt != null) {
    return { kind: 'generating', label: 'Writing the reply', short: 'Writing', detail: null, tone: 'busy', prominent: false };
  }

  return {
    kind: 'ready',
    label: 'Ready — running on this device',
    short: 'Ready',
    detail: null,
    tone: 'ready',
    prominent: false,
  };
}

/// Show states long enough to be read, without ever lying about the present.
///
/// Three rules, each of which exists because the obvious implementation gets
/// it wrong:
///
///  1. **Same kind updates immediately.** A download ticking from 41% to 42%
///     is the same state with new numbers, not a transition. Queueing those
///     would freeze the percentage for `minDwellMs` at a time and make a
///     working download look stuck.
///  2. **Errors preempt.** An error-tone state clears the queue and shows at
///     once. Making someone watch a backlog of "loading" before being told it
///     failed is the one case where readability and honesty conflict, and
///     honesty wins.
///  3. **The queue is bounded by lag, not by length.** If catching up would
///     put the display more than `maxLagMs` behind the truth, intermediate
///     states are dropped and the newest is shown. A skipped beat beats a
///     status line describing something that stopped being true.
///
/// Time and scheduling are injected so the whole thing is testable without a
/// clock: `now()` returns milliseconds, `schedule(fn, ms)` returns a handle
/// and `cancel(handle)` clears it.
export function createReadableSequence({
  minDwellMs = MIN_DWELL_MS,
  maxLagMs = MAX_LAG_MS,
  now,
  schedule,
  cancel,
  render,
}) {
  let shown = null;
  let shownAt = 0;
  let queue = [];
  let timer = null;

  function show(next) {
    shown = next;
    shownAt = now();
    render(next);
  }

  function armTimer() {
    if (timer != null || queue.length === 0) return;
    const wait = Math.max(0, minDwellMs - (now() - shownAt));
    timer = schedule(() => {
      timer = null;
      advance();
    }, wait);
  }

  function advance() {
    if (queue.length === 0) return;
    // Rule 3: never let the backlog put the display further behind than
    // maxLagMs. Everything but the newest is dropped when it would.
    if ((queue.length - 1) * minDwellMs > maxLagMs) queue = [queue[queue.length - 1]];
    show(queue.shift());
    armTimer();
  }

  return {
    /// Offer a new presentation. It becomes visible now or after the current
    /// one has had its dwell.
    push(next) {
      if (shown && next.kind === shown.kind) {
        // Rule 1 — same state, fresh numbers.
        show(next);
        return;
      }
      if (next.tone === 'error') {
        // Rule 2 — preempt.
        queue = [];
        if (timer != null) { cancel(timer); timer = null; }
        show(next);
        return;
      }
      if (shown == null) {
        show(next);
        return;
      }
      // Collapse a repeat of whatever is already last in line: a state offered
      // twice while queued is one transition, not two.
      const last = queue.length ? queue[queue.length - 1] : shown;
      if (last.kind === next.kind) {
        if (queue.length) queue[queue.length - 1] = next;
        return;
      }
      queue.push(next);
      armTimer();
    },
    /// What the user is looking at (not necessarily what is true).
    current() { return shown; },
    /// How many states are waiting their turn.
    pending() { return queue.length; },
    /// Drop everything queued and stop. Used when the view is torn down.
    reset() {
      queue = [];
      if (timer != null) { cancel(timer); timer = null; }
      shown = null;
      shownAt = 0;
    },
  };
}
