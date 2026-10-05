# Cleophis mobile — Phase 2 complete

**For the founder.** Where the Android app stands, what still needs your
device, and what Phase 4 needs from you.

---

## Done

**The app is a working Android chat app.** It installs as `com.cleophis.app`,
the library populates, covers render, conversations persist, and the model runs
**in-process on the phone** — no server, no network for inference.

| | |
|---|---|
| **Engine** | Runs on-device with tool calling, streaming, the calculator loop, partial-turn recovery, and conversation-prefix reuse so replies don't get slower as a chat grows. |
| **Chat** | Full in-app chat, replacing the desktop's server transport. |
| **Layout** | Drawer sidebar, on-screen Send (it was *off-screen in portrait* before), keyboard handling, safe areas. |
| **Engine state** | The app now says what it is doing — verifying, loading, ready — instead of a decorative label. There was previously no element that reported this at all. |
| **Tiering** | Picks the model size from the phone's actual CPU, not just its RAM. Your A22 correctly gets the small model. |
| **Sign-in that survives** | Credentials encrypted with AES-256-GCM under a key held in the phone's hardware keystore. **This is what makes a force-stop stop logging you out.** |
| **Downloads** | Won't pull a multi-gigabyte model over mobile data without asking, with a per-download override and a charge suggestion for big ones. |
| **Sharing** | Export a chat to the Android share sheet — Drive, mail, Keep, another app. |
| **Long chats** | Trim the request to fit the model's window; **your stored transcript is never truncated.** |

**Desktop is unchanged.** Every difference is behind a platform switch, and the
Windows test suite has passed at every step — 266 tests at the start of this
work, 332 now, with **zero failures ever attributable to new code.**

---

## What still needs your phone (CP3 + CP1, one session)

The runbook is `docs/ops/cp3-founder-checkpoint.md`. It is ordered so the
gate-critical part comes first — if you run out of time or patience, run out
of it on Part B.

**Part A — the one that matters.** Sign in, force-stop the app, reopen it. You
should still be signed in. That is the whole point of the credential work, and
it is also the first real exercise of one piece of plumbing we have reasoned
through but never run. Then: sign out and confirm the encrypted files are gone,
and try an offline sign-in in airplane mode.

**Part B — measurements still open from CP1.** Confirm the CPU math kernels are
active (if not, ignore the speed numbers entirely), rough first-word latency
early vs. deep in a conversation, a tampered model file being refused, and the
four behaviour probes.

**One thing to know going in:** the app prints a line for every credential
read and write. Seeing those lines is part of the test — silence would be
indistinguishable from the code never running.

---

## What Phase 4 needs from you

Phase 4 (in-app updates without Google Play) is the last engineering block
before the P1 gate, and **it is the first work that touches your signing key.**

**One session with you, and it does two jobs at once:**

1. **Signing wiring.** The build reads the keystore path and passphrase from a
   local file that is **never in the repo** — the key stays where you put it,
   and you hold the passphrase. Nothing about the ceremony changes; this only
   teaches the build where to look.
2. **The first signed release build**, which is the earliest moment several
   safety checks can run *at all*.

**Why they are the same session.** A release build strips code it thinks is
unused — and it cannot see that our Android code is called from Rust. If it
strips the wrong thing, the symptom is **"signed out on every launch"**, which
looks like a bug in something else entirely. The debug build you are testing
has that stripping turned off, so a green result from your CP3 session says
nothing about it. The checklist for this was written in advance, cold, and it
now runs as **one command** rather than a list of things to remember during a
key ceremony.

**Still outstanding from earlier, unchanged:**

- Google Play developer account ($25) + developer-verification registration
  (Thailand is in the pilot for apps distributed outside Play — our market).
  Worth starting early; it is a waiting game, not a work item.
- **Two more test devices** for the 3-device install/update gate: one 8 GB
  mid-ranger, one Huawei/Honor without Google services. The A22 is device #1.

---

## Honest limits

- **Timing numbers from the current build are indicative.** The math kernels
  are compiled optimised even in a debug build, so they are closer than they
  sound — but the definitive figures come from a signed release build.
- **Release behaviour is unverified**, by construction: no signed build exists
  yet. Every item that can only be checked there is written down rather than
  assumed.
- **Backup exclusion is verified as correct in form, not yet in effect.** We
  found that the rules protecting your conversation database from Google Drive
  backup named the wrong directories, and fixed it. Nothing ever leaked — a
  separate setting was doing the work — but the second layer was not doing what
  it claimed. Proving the fix works needs a device test, and it is on the
  checklist.
