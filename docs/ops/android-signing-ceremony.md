# Android APK Signing Key — Ceremony Record

**Crown jewel #2** (spec v1.1 §5.1). Loss means every installed user of the direct-APK channel can never update again. This record contains ONLY public facts — the keystore file and its passphrase exist nowhere in any repository, log, or chat transcript.

| field | value |
|---|---|
| Ceremony date | 2026-07-26 |
| Performed by | Founder (key generated at an interactive prompt; passphrase typed only there, stored only in the founder's password manager) |
| Keystore | `~/cleophis-keys/cleophis-release.keystore` on the founder machine (WSL-native fs; dir `700`, file `600`; outside every repository) |
| Type / alg | PKCS12, RSA-4096, validity 10,950 days (~30 years) |
| Alias | `cleophis` |
| DN | `CN=Cleophis, O=Cleophis, C=TH` |
| **Certificate SHA-256** | `EE:C5:F0:8E:14:47:84:40:89:D1:05:B1:E4:38:41:90:91:87:5C:4F:2C:7F:86:7D:E1:BD:AB:88:A0:32:46:98` |

## Backup attestations (spec: two offline backups, each verified restorable)

| # | medium / label | verified (`keytool -list` against the copy) | date |
|---|---|---|---|
| 1+ | Multiple offline copies, locations known to founder | ☑ founder-attested: "saved in multiple places, all paths verified" | 2026-07-26 |

**CEREMONY COMPLETE** (founder attestation 2026-07-26). Optional hardening: record the physical labels/locations here privately if ever useful for recovery drills.

## Rules of handling (carry-forward from the curator-key regime)

- The keystore and passphrase never enter a repository, a chat, a log, an environment variable committed anywhere, or any cloud storage.
- Release signing happens only on the founder machine. Build wiring (Phase 4/5) reads the keystore path from an untracked, `600`-permission local properties file; the passphrase is entered at build time or read from that same local file — never from the repo.
- The direct-APK channel key is never enrolled in Play App Signing; if a Play listing ever happens, Play gets its own key and this one continues to govern the direct channel (spec §5.1).
- Any suspicion of compromise: stop publishing, rotate via a forced-update path while the old key still works — see spec §5.1's loss/rotation notes.

## Verification snippet (public)

Anyone can verify an official Cleophis APK's signer:
`apksigner verify --print-certs cleophis.apk` → the SHA-256 digest above must match.
