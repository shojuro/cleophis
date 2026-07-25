# The layout harness — measuring the frontend without a device

`layout-harness.mjs` serves the real `src/` tree over HTTP and synthesizes one
extra route, `/harness.html`, which is `src/index.html` with a Tauri stub
injected as a classic `<script>` before the module script. Classic scripts run
first, so `window.__TAURI__` exists by the time `app.js` evaluates.

**Nothing in `src/` is modified.** That is the point: what you measure is the
shipping file, not a copy of it.

```bash
node docs/superpowers/mobile-tools/layout-harness.mjs "$PWD" 8731
# then open http://127.0.0.1:8731/harness.html?ua=android&installed=1
```

Query parameters:

| param | effect |
|---|---|
| `ua=android` | `navigator.userAgent` reports Android, so `transport.js` picks the mobile half and `app.js` sets `.is-mobile` |
| `installed=1` | `download_status` reports the hero installed |
| `supported=0` | `detect_hardware` reports `supported:false` (drives the "not yet" screen) |
| `engine=Starting` | `engine_info` status — `Ready` by default |

`window.__HARNESS__.emit(name, payload)` fires a Tauri event into the page, so
`engine-ready` / `engine-failed` / `download-progress` sequences can be driven
without a backend. `window.__HARNESS__.calls` records every `invoke`.

## What this can and cannot settle

**Can** — anything geometric, and it settles it as *fact*:
`document.documentElement.scrollWidth > clientWidth` is horizontal overflow;
`getBoundingClientRect()` says whether an element is inside the viewport. This
is what disproved the 2.2 handoff's overshoot hypothesis (`.views` measured
exactly 720px at a 360px viewport, and forcing a 900px sidebar into the pane
did not move it) and what found `#sendBtn` sitting 230.6px off-screen.

**Cannot** — whether the result *reads* right. Two defects in the 2.2 session
passed their bounds assertions and were caught only by screenshotting the page
and looking at it: the pill rendering `"Ready — r"` (rect inside the viewport,
text clipped inside the rect) and `.grow` splitting the title's flex space.
That is this repo's recurring failure mode — *numeric assertions confirm the
property you thought to measure, never the one you didn't* — so **render it and
look** is not optional here, it is the second half of the method.

Visual acceptance stays with the founder on real hardware.

## The desktop byte-identity check

The most valuable single measurement, because it is the constraint that is
easiest to violate silently. Load **without** `ua=android`, at a wide window
**and at 360px**, and confirm:

- `document.documentElement.classList.contains('is-mobile')` is `false`
- `#chatSidebar` is `position:static`, 260px
- `#chatMenuBtn`, `#sidebarScrim`, `#engineState`, `#engineStateProg` are all
  `display:none`; `#notYetScreen` is hidden
- `--app-h` is unset
- after `__HARNESS__.emit('engine-ready', …)` and a `download-progress`, the
  pill **still reads `"Local · offline"`**

The last line is the one that matters: it proves the `if (!IS_MOBILE) return`
guard actually stops the mobile path rather than merely being present. The
360px run is what a width-keyed media query would have broken, which is why the
mobile rules are scoped on a class instead.
