# Handoff — task 2.2 (mobile UI)

Written at the 2.1/2.3 boundary by the third-generation `mobile-p1` instance,
which stopped here deliberately rather than start 2.2 near its context limit.
Phase 2 state at the time of writing: **2.1 DONE (`922f40b`), 2.3 DONE
(`f78424d`), 2.2 not started.** Tree clean at `f78424d`.

---

## Why 2.2 was left rather than started

**2.2 is the only task in this phase whose acceptance criterion is visual, and
no agent on this track can see.** Every other task has been closed by a test, a
compiler, or a digest. "The sidebar takes two thirds of the width", "the
transition slides a quarter and stops", "the founder could not identify the
engine-state element" are all observations a person made while holding a phone.

That has a consequence worth internalising before writing any CSS: **2.2 cannot
be verified the way the rest of this phase was**, so the usual move — build it,
prove it, report it — does not work. The two things that substitute for it are
below, and the second is the one that actually closes the loop.

---

## What 2.2 has to fix (founder field observations)

From the CP0-era screenshots and steering's ruling. These are reports of what a
person saw, not a spec — treat them as symptoms.

1. **The sidebar takes ~2/3 of phone width.** Should be a drawer.
2. **The view transition slides ~1/4 and stops.** Steering's ruling is explicit:
   it must be **disabled or replaced, not restyled.**
3. **The status pill clips off-screen**, and must "get its moment" — the founder
   could not identify the engine-state element at all.
4. **Post-download transitions flash past unreadably.**
5. Plus the un-observed spec items: safe-area insets, IME behaviour, font
   scaling, unmetered-download policy + charge notice, share-sheet export, and
   the **"not yet" onboarding screen** that 2.3's `HardwareInfo.supported=false`
   now drives but nothing renders.

### A read of (1) and (2) that is worth checking before rewriting anything

Both smell like one cause: **fixed pixel widths designed for a desktop window.**

```css
.views{display:flex;width:200%;height:100vh;transform:translateX(0);
  transition:transform .5s cubic-bezier(.22,.9,.34,1)}
.views.in-chat{transform:translateX(-50%)}
```

`translateX(-50%)` on a 200%-wide flex row is exactly one viewport — correct
arithmetic, *if* each pane is exactly half of `.views`. If the chat pane's
contents (a fixed-width sidebar plus the message column) overflow a phone
viewport, `.views` is wider than `2 × 100vw`, and then 50% of it is **more than
one viewport** — the pane lands short of where it should and appears to stop
part-way. That would make (2) a *consequence* of (1) rather than an independent
bug, and would mean the sliding model is not broken so much as inapplicable.

**Check this before designing.** If it holds, the honest fix for (2) is the one
steering already ruled: on a narrow viewport, drop the two-pane transform model
entirely and switch views by display, with the sidebar as an overlay drawer. If
it does not hold, the reasoning above is wrong and the real cause is still
unfound — do not paper over it with a transition tweak.

---

## The verification problem, and the two things that address it

### 1. A headless harness gets you layout facts without a device

`src/index.html` + `styles.css` can be loaded in a real browser at a phone
viewport (360×800) and measured — `document.documentElement.scrollWidth >
clientWidth` is a *fact* about horizontal overflow, and an element's
`getBoundingClientRect()` is a fact about whether the status pill is inside the
viewport. That converts three of the five field reports from "the founder said"
into "asserted".

`app.js` will throw on import without the Tauri globals, so the harness needs a
small stub for `window.__TAURI__.core.invoke` / `.event.listen` returning canned
values. That stub is worth writing once; it is also what any future frontend
test will need.

**What this cannot tell you** is whether the result looks right, reads right, or
feels right on a real phone — which is most of 2.2. It catches clipping and
overflow, not "the founder could not find the engine state."

### 2. Sequence 2.2 *after* the founder has used the app once

There is a real chat build available now (2.1 landed the transport). The
CP1/session-A proposal in the report to steering exists partly for this reason:
field requirements derived from screenshots of an app that could not chat are
weaker evidence than an hour of a person actually trying to chat on it. **Let
session A regenerate the 2.2 requirements**, then build against those.

If 2.2 is started before that session, prefer changes that are cheap to revert
and obviously scoped (one `@media` block, one body class) over a re-architecture
of the view model.

---

## Constraints that still apply

- **Desktop must stay byte-identical.** For CSS that means mobile rules live
  behind a `@media` query or a mobile-only class, never by editing a shared
  rule's values. The desktop suite cannot catch a CSS regression, so this one is
  on review discipline rather than on a gate.
- **H1 rendering hygiene** was audited clean in 2.1 and the invariant to
  preserve is written down in the milestone doc: **model output has exactly one
  escaped path into markup (the chat title, via `escapeHtml` in `chatRowHtml`)
  and every other path is `textContent`.** Any new renderer 2.2 adds must keep
  that true. In a Tauri WebView a DOM XSS is the whole `invoke()` surface.
- **`npm test`** now exists (`node --test src/`) — 14 tests, and it is where a
  frontend test belongs.
- Founder-serialized, never attempt: device installs, signing, registrations.

## Loose ends inherited from 2.1 / 2.3

- `HardwareInfo.supported=false` has **no UI**. 2.3 produces the fact; the
  "not yet" screen is 2.2's.
- The **Stop-button race** documented under 2.1 is unreachable by a human today.
  If 2.2 adds a programmatic abort (e.g. abort-on-chat-switch on mobile), it
  becomes reachable and needs the fix described there.
- `state.engine.port` is meaningless on Android but still populated, because
  `free_port()` runs on both platforms in `lib.rs` setup. Harmless — the mobile
  transport ignores it — but it is why the `!state.engine?.port` guard in
  `maybeAutoTitle` still passes on a phone. Do not "clean it up" without
  checking that guard.
