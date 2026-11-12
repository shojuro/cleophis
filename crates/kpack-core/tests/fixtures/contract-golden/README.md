# Contract golden: the full grounded system message

These fixtures pin, byte for byte, the grounded system message that
`kpack_core::contract::assemble_system` builds. The triage repo vendors this
directory; its JS probe runner and its Python row builder must reproduce every
`assembled-*.txt` file exactly from `chunks-k3.json` and the contract TOML.

## Files

| File | Role |
|---|---|
| `chunks-k3.json` | Input. Hand-written. The k=3 chunks, the expected doc-context line, and the list of outputs. |
| `assembled-triage-with-doc-context.txt` | Emitted. Triage contract, doc-context line included. |
| `assembled-triage-no-doc-context.txt` | Emitted. Triage contract, doc-context line omitted. |
| `assembled-tutor-with-doc-context.txt` | Emitted. Tutor contract, doc-context line included. |

The `.txt` files are UTF-8, LF only, with **no trailing newline**. The repo's
`.gitattributes` (`* text=auto eol=lf`) keeps them LF on every checkout.

Re-emit only after a reviewed contract or assembly change:

```
cargo test -p kpack-core -- --ignored emit_golden
```

The normal `cargo test -p kpack-core` run fails if any emitted file drifts.

## `chunks-k3.json` format (`"format": "cleophis-contract-golden/1"`)

- `k`: the number of chunks.
- `chunks[]`: in rank order, each `{source_title, section_path, locator, text}`.
  Chunk 1 has all three source parts. Chunk 2 is a root chunk with an empty
  `section_path`. Chunk 3's text contains literal `{n}`, `{text}` and `{source}`.
- `doc_context`: the doc-context line derived from the chunks (rule 3 below).
  A port must derive it itself and check it equals this value.
- `outputs[]`: `{file, contract, contract_id, with_doc_context}`. `contract` is
  the repo-relative path of the TOML whose fields are used.

## The assembly rule

Inputs: a contract's `system_contract`, `citation_line` and
`citation_source_sep` strings (TOML-parsed, so the multi-line
`system_contract` ends in one `"\n"`), an optional doc-context line, and the
chunks.

1. **Refuse k = 0.** Zero chunks is an error. A grounded prompt is never
   emitted without sources; the caller takes the NO_EVIDENCE path instead.
2. **Sources block.** For chunk i (1-based), `source` is the non-empty parts of
   `[source_title, section_path, locator]`, untrimmed, joined by
   `citation_source_sep` (`", "`). The line is `citation_line`
   (`"[{n}] ({source}): {text}"`) with `{n}` = the decimal i, `{source}` and
   `{text}` substituted in ONE left-to-right pass. Substituted values are never
   re-scanned, so a chunk text containing `{n}` stays literal. An unrecognised
   `{...}` in the template is copied literally. Lines are joined by a single
   `"\n"`, with no trailing newline.
3. **Doc-context line.** Take each chunk's `source_title` in chunk order.
   Trim surrounding whitespace, drop empty titles, and drop a title equal to
   one already kept, keeping the first. If none remain, there is no line.
   Otherwise the line is
   `"These sources are excerpts from: " + kept.join("; ") + "."`.
4. **Message.** `S = system_contract` with all trailing whitespace removed
   (Rust `trim_end`, Python `rstrip()`, JS `trimEnd()`; leading bytes kept).
   - With a doc-context line `D`: `S + "\n\n" + D + "\n\n" + sources_block`.
   - Without one (absent or the empty string): `S + "\n\n" + sources_block`.

   The message has no trailing newline, and never contains three consecutive
   newlines unless a chunk's own text does.

## Python sketch (reference only; the fixture bytes are the authority)

```python
import json, tomllib

def render_line(tpl, n, source, text):
    out, i = [], 0
    subs = {"{n}": n, "{source}": source, "{text}": text}
    while i < len(tpl):
        for ph, val in subs.items():
            if tpl.startswith(ph, i):
                out.append(val); i += len(ph); break
        else:
            out.append(tpl[i]); i += 1
    return "".join(out)

def assemble_system(c, doc_context, chunks):
    if not chunks:
        raise ValueError("no chunks")
    lines = []
    for i, ch in enumerate(chunks, 1):
        parts = [p for p in (ch["source_title"], ch["section_path"], ch["locator"]) if p]
        lines.append(render_line(c["citation_line"], str(i),
                                 c["citation_source_sep"].join(parts), ch["text"]))
    s = c["system_contract"].rstrip()
    body = "\n".join(lines)
    return f"{s}\n\n{doc_context}\n\n{body}" if doc_context else f"{s}\n\n{body}"
```
