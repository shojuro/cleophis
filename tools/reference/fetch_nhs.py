#!/usr/bin/env python3
"""fetch_nhs.py — Phase 1h Task M4a: the NHS reference corpus (Medicines A-Z + Health A-Z).

Fetches the NHS website's Medicines A-Z and Health A-Z indexes and every
entry page (plus that entry's own sub-pages, e.g. a condition's "Symptoms" /
"Treatment" pages), extracts the main article text as markdown, and writes:

    tools/reference/corpus/<section>/<slug>.md   one file per index entry
    tools/reference/corpus/index.json            the manifest

`<section>` is `medicines` (Medicines A-Z) or `conditions` (Health A-Z; this
includes the index's entries that live outside `/conditions/`, e.g. the
`/mental-health/conditions/...` pages the Health A-Z links to).

LICENCE. NHS website content is released under the Open Government Licence
v3.0 by the NHS website terms and conditions §3.3-3.4 (checked 2026-09-27;
see docs/superpowers/mobile-tools/build-reference-pack.md for the citation
and the attribution conditions of §3.6). Images are never fetched or kept
(many are stock-licensed and excluded from the OGL grant, §3.5), and no
third-party embed survives extraction. Every file carries the URL, the UTC
retrieval date and the OGL attribution in its YAML front matter.

POLITENESS. robots.txt is fetched first and every URL is checked against it
(case-sensitively, as RFC 9309 requires: the live file disallows
`/Conditions/` in capitals, which is NOT the lowercase `/conditions/` Health
A-Z). One connection, at least `--delay` seconds (default 1.0) between
network requests, exponential back-off on 429/5xx honouring Retry-After,
and every response (including 404s) is cached under `--cache` so a re-run
re-renders from disk without touching the network (`--offline` enforces
that). Only https://www.nhs.uk/ is ever requested.

DETERMINISM. Output depends only on the cached HTML: stable ordering (index
order for entries, first-link order for sub-pages), no timestamps in the
body (the retrieval date lives in the front matter and comes from the
cache, not the clock), JSON written with sorted keys. Re-running
`--offline` over the same cache is byte-identical.

Stdlib only (html.parser, urllib) — no new dependency for the pipeline.

Usage:
    python3 tools/reference/fetch_nhs.py                 # full fetch (network, ~1h at 1 req/s)
    python3 tools/reference/fetch_nhs.py --offline       # re-render from the cache only
    python3 tools/reference/fetch_nhs.py --limit 5       # smoke: first 5 entries per section, no prune
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime as _dt
import hashlib
import json
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import urllib.robotparser
from html.parser import HTMLParser
from pathlib import Path

REFERENCE_ROOT = Path(__file__).resolve().parent
DEFAULT_OUT = REFERENCE_ROOT / "corpus"
DEFAULT_CACHE = REFERENCE_ROOT / ".cache"

HOST = "www.nhs.uk"
BASE = f"https://{HOST}"
USER_AGENT = "cleophis-reference-fetch/1.0 (offline reference corpus build; OGL v3 reuse; 1 req/s)"
SECTIONS = {
    "medicines": f"{BASE}/medicines/",
    "conditions": f"{BASE}/conditions/",
}
LICENCE = "OGL v3"
ATTRIBUTION = "Information from the NHS website, licensed under the Open Government Licence v3.0"
MANIFEST_SCHEMA = "cleophis/reference-corpus-index/v1"
MAX_SUBPAGES = 60
# Sites inside www.nhs.uk that the NHS website terms do NOT license under the
# OGL ("Content not covered by our standard terms and conditions",
# https://www.nhs.uk/our-policies/terms-and-conditions/content-not-licensed-for-re-use/,
# page last reviewed 17 April 2024, read 2026-09-27). Never fetched.
EXCLUDED_PREFIXES = ("/change4life/", "/best-start-in-life/", "/be-clear-on-cancer/", "/oneyou/", "/smokefree/", "/quit/")

# ---------------------------------------------------------------------------
# A tiny, lenient DOM over html.parser.
# ---------------------------------------------------------------------------

VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"}


@dataclasses.dataclass
class Node:
    tag: str
    attrs: dict
    children: list = dataclasses.field(default_factory=list)
    parent: "Node | None" = None

    def cls(self) -> str:
        return self.attrs.get("class", "") or ""

    def iter(self):
        yield self
        for c in self.children:
            if isinstance(c, Node):
                yield from c.iter()

    def find(self, tag: str) -> "Node | None":
        for n in self.iter():
            if n.tag == tag:
                return n
        return None

    def text(self) -> str:
        out = []
        for c in self.children:
            out.append(c if isinstance(c, str) else c.text())
        return "".join(out)


class _TreeBuilder(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.root = Node("#root", {})
        self.stack = [self.root]

    def handle_starttag(self, tag, attrs):
        node = Node(tag, {k: (v or "") for k, v in attrs}, parent=self.stack[-1])
        self.stack[-1].children.append(node)
        if tag not in VOID:
            self.stack.append(node)

    def handle_startendtag(self, tag, attrs):
        node = Node(tag, {k: (v or "") for k, v in attrs}, parent=self.stack[-1])
        self.stack[-1].children.append(node)

    def handle_endtag(self, tag):
        if tag in VOID:
            return
        for i in range(len(self.stack) - 1, 0, -1):
            if self.stack[i].tag == tag:
                del self.stack[i:]
                return
        # stray end tag: ignore

    def handle_data(self, data):
        self.stack[-1].children.append(data)


def parse_html(html: str) -> Node:
    b = _TreeBuilder()
    b.feed(html)
    b.close()
    return b.root


def main_node(root: Node) -> Node:
    return root.find("main") or root.find("body") or root


# ---------------------------------------------------------------------------
# URLs.
# ---------------------------------------------------------------------------


def normalise_url(href: str, base: str) -> str | None:
    """Absolute https://www.nhs.uk URL with no query/fragment and a trailing
    slash on directory-style paths; None for anything off-site or non-http."""
    href = (href or "").strip()
    if not href or href.startswith(("mailto:", "tel:", "javascript:", "sms:")):
        return None
    absu = urllib.parse.urljoin(base, href)
    p = urllib.parse.urlsplit(absu)
    if p.scheme not in ("http", "https") or p.netloc.lower() not in (HOST, "nhs.uk"):
        return None
    path = p.path or "/"
    last = path.rsplit("/", 1)[-1]
    if not path.endswith("/") and "." not in last:
        path += "/"
    return f"{BASE}{path}"


def url_path(url: str) -> str:
    return urllib.parse.urlsplit(url).path


def assign_slugs(urls: list[str]) -> dict[str, str]:
    """Slug = the URL's last path segment; where two URLs share one, both
    use their full path joined with `--` instead (deterministic, no
    first-come-first-served)."""
    last = {u: [s for s in url_path(u).split("/") if s][-1] for u in urls}
    counts: dict[str, int] = {}
    for s in last.values():
        counts[s] = counts.get(s, 0) + 1
    out = {}
    for u in urls:
        if counts[last[u]] == 1:
            out[u] = last[u]
        else:
            out[u] = "--".join(s for s in url_path(u).split("/") if s)
    return out


# ---------------------------------------------------------------------------
# Index pages.
# ---------------------------------------------------------------------------


@dataclasses.dataclass
class IndexEntry:
    url: str
    index_title: str
    aliases: list = dataclasses.field(default_factory=list)


_SEE = re.compile(r"^(.*?),\s+see\s+(.+)$", re.I)


def _squash(s: str) -> str:
    return re.sub(r"\s+", " ", s).strip()


def parse_index(html: str, index_url: str) -> list[IndexEntry]:
    """Entries of an A-Z index page, in page order, de-duplicated by URL.

    Only links inside the `<main>` element's lists count (the A-Z letter
    navigation is a `<nav>` and its links are `#x` fragments). A link text
    of the form "Acid reflux, see Heartburn and acid reflux" is an ALIAS of
    the target entry, not an entry of its own."""
    root = main_node(parse_html(html))
    order: list[str] = []
    by_url: dict[str, IndexEntry] = {}
    index_path = url_path(index_url)
    for n in root.iter():
        if n.tag != "a" or _inside(n, lambda a: a.tag == "nav"):
            continue
        if not _inside(n, lambda a: a.tag == "li"):
            continue
        raw = n.attrs.get("href", "")
        if "#" in raw and urllib.parse.urlsplit(urllib.parse.urljoin(index_url, raw)).path == index_path:
            continue
        url = normalise_url(raw, index_url)
        if url is None or url_path(url) == index_path:
            continue
        text = _squash(n.text())
        m = _SEE.match(text)
        alias, title = (m.group(1).strip(), m.group(2).strip()) if m else (None, text)
        if url not in by_url:
            by_url[url] = IndexEntry(url=url, index_title=title if not alias else "", aliases=[])
            order.append(url)
        e = by_url[url]
        if alias:
            if alias not in e.aliases:
                e.aliases.append(alias)
            if not e.index_title:
                e.index_title = title
        elif not e.index_title or e.index_title == title:
            e.index_title = title
        elif title not in e.aliases and title != e.index_title:
            e.aliases.append(title)
    return [by_url[u] for u in order]


def _inside(n: Node, pred) -> bool:
    p = n.parent
    while p is not None:
        if pred(p):
            return True
        p = p.parent
    return False


# ---------------------------------------------------------------------------
# Article extraction.
# ---------------------------------------------------------------------------

DROP_TAGS = {
    "script", "style", "svg", "img", "picture", "figure", "iframe", "video", "audio", "noscript", "form",
    "button", "input", "select", "textarea", "nav", "aside", "object", "embed", "canvas", "template",
    "header", "footer", "link", "meta",
}
DROP_CLASS_PARTS = (
    "nhsuk-breadcrumb", "nhsuk-back-link", "nhsuk-contents-list", "nhsuk-pagination", "hub-key-links",
    "related-links", "sibling-nav", "nhsuk-action-link", "nhsuk-review-date", "nhsuk-video", "nhsuk-image",
    "nhsuk-card--clickable", "nhsuk-caption-xl", "nhsuk-skip-link", "app-feedback", "nhsuk-promo",
)
DROP_IDS = {"sibling-nav", "nhsuk-feedback", "feedback"}
BLOCK_TAGS = {
    "p", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "table", "details", "blockquote", "hr", "dl", "pre",
    "div", "section", "article", "main", "summary", "li", "dt", "dd", "figcaption", "#root", "body", "html",
}
_REVIEWED = re.compile(r"Page last reviewed:\s*(.+?)\s*(?:Next review due:\s*(.+?))?\s*$", re.S)
_BRANDS = re.compile(r"brands?(?:\s+names?)?\s*:\s*(.+)$", re.I)


def _dropped(n: Node) -> bool:
    if n.tag in DROP_TAGS:
        return True
    c = n.cls()
    if c and any(part in c for part in DROP_CLASS_PARTS):
        return True
    return n.attrs.get("id", "") in DROP_IDS


@dataclasses.dataclass
class PageDoc:
    url: str
    title: str
    body: str  # markdown, WITHOUT the page title heading
    brands: list = dataclasses.field(default_factory=list)
    last_reviewed: str = ""
    next_review: str = ""
    heading: str = ""  # the page's own H1 text: == title, except on old-format pages ("Symptoms")


def _inline(n, in_heading: bool = False) -> str:
    """Inline text of a node: links become their text, <b>/<strong> become
    **bold** (never inside a heading), <br> a newline, everything else its
    text. Whitespace is squashed by the caller."""
    if isinstance(n, str):
        return n
    if _dropped(n):
        return ""
    if n.tag == "br":
        return "\n"
    inner = "".join(_inline(c, in_heading) for c in n.children)
    if n.tag in ("b", "strong") and not in_heading:
        core = inner.strip()
        if not core:
            return inner
        lead = " " if inner[:1].isspace() else ""
        trail = " " if inner[-1:].isspace() else ""
        return f"{lead}**{core}**{trail}"
    return inner


def _clean_inline(s: str) -> str:
    lines = [re.sub(r"[ \t\r\f\v ]+", " ", ln).strip() for ln in s.split("\n")]
    return "\n".join(ln for ln in lines if ln)


def _is_block(n) -> bool:
    return isinstance(n, Node) and (n.tag in BLOCK_TAGS)


class _Renderer:
    def __init__(self):
        self.reviewed = ("", "")
        self.skip_first_h1 = True

    def blocks(self, n: Node, depth: int = 0) -> list[str]:
        """Markdown blocks for a container node's children."""
        out: list[str] = []
        run: list = []

        def flush():
            if run:
                t = _clean_inline("".join(_inline(x) for x in run))
                label_only = all(
                    (isinstance(x, str) and not x.strip())
                    or (isinstance(x, Node) and "nhsuk-u-visually-hidden" in x.cls())
                    for x in run
                )
                if t and not label_only:
                    out.append(t)
                run.clear()

        for c in n.children:
            if isinstance(c, Node) and _dropped(c):
                continue
            if _is_block(c):
                flush()
                out.extend(self.block(c, depth))
            else:
                run.append(c)
        flush()
        return out

    def block(self, n: Node, depth: int) -> list[str]:
        t = n.tag
        if t == "h1" and self.skip_first_h1:
            self.skip_first_h1 = False
            return []
        if t in ("h1", "h2", "h3", "h4", "h5", "h6"):
            level = int(t[1])
            txt = _clean_inline("".join(_inline(c, in_heading=True) for c in n.children)).replace("\n", " ")
            return [f"{'#' * level} {txt}"] if txt else []
        if t == "p":
            raw = _clean_inline("".join(_inline(c) for c in n.children))
            m = _REVIEWED.match(raw.replace("\n", " "))
            if m:
                self.reviewed = (m.group(1).strip(), (m.group(2) or "").strip())
                return []
            return [raw] if raw else []
        if t in ("ul", "ol"):
            txt = self.list_block(n, depth=0)
            return [txt] if txt else []
        if t == "table":
            txt = self.table(n)
            return [txt] if txt else []
        if t == "details":
            out = []
            for c in n.children:
                if isinstance(c, Node) and c.tag == "summary":
                    s = _clean_inline("".join(_inline(x, in_heading=True) for x in c.children)).replace("\n", " ")
                    if s:
                        out.append(f"**{s}**")
            rest = Node("div", {}, [c for c in n.children if not (isinstance(c, Node) and c.tag == "summary")])
            out.extend(self.blocks(rest, depth))
            return out
        if t == "summary":
            s = _clean_inline("".join(_inline(x, in_heading=True) for x in n.children)).replace("\n", " ")
            return [f"**{s}**"] if s else []
        if t == "hr":
            return []
        if t == "dl":
            out = []
            for c in n.children:
                if isinstance(c, Node) and c.tag == "dt":
                    s = _clean_inline(_inline(c, in_heading=True)).replace("\n", " ")
                    if s:
                        out.append(f"**{s}**")
                elif isinstance(c, Node) and c.tag == "dd":
                    out.extend(self.blocks(c, depth))
            return out
        if t == "pre":
            s = n.text().strip("\n")
            return [s] if s.strip() else []
        if t == "blockquote":
            inner = self.blocks(n, depth)
            return ["\n".join("> " + ln if ln else ">" for ln in b.split("\n")) for b in inner]
        # generic container (div, section, article, li outside a list, ...)
        return self.blocks(n, depth)

    def list_block(self, n: Node, depth: int) -> str:
        ordered = n.tag == "ol"
        lines: list[str] = []
        idx = 0
        for li in n.children:
            if not (isinstance(li, Node) and li.tag == "li") or _dropped(li):
                continue
            idx += 1
            marker = f"{idx}. " if ordered else "- "
            pad = " " * len(marker)
            text_parts: list[str] = []
            nested: list[str] = []
            run: list = []

            def flush():
                if run:
                    t = _clean_inline("".join(_inline(x) for x in run)).replace("\n", " ")
                    if t:
                        text_parts.append(t)
                    run.clear()

            for c in li.children:
                if isinstance(c, Node) and _dropped(c):
                    continue
                if isinstance(c, Node) and c.tag in ("ul", "ol"):
                    flush()
                    sub = self.list_block(c, depth + 1)
                    if sub:
                        nested.append(sub)
                elif _is_block(c):
                    flush()
                    for b in self.blocks(Node("div", {}, [c]), depth):
                        text_parts.append(b.replace("\n", " "))
                else:
                    run.append(c)
            flush()
            head = " ".join(text_parts).strip()
            if not head and not nested:
                idx -= 1
                continue
            lines.append(f"{marker}{head}".rstrip())
            for sub in nested:
                lines.extend((pad + ln) if ln else ln for ln in sub.split("\n"))
        return "\n".join(lines)

    def table(self, n: Node) -> str:
        caption = ""
        rows: list[list[str]] = []
        header_idx = None
        for el in n.iter():
            if el.tag == "caption" and not caption:
                caption = _clean_inline(_inline(el, in_heading=True)).replace("\n", " ")
            if el.tag == "tr":
                cells = [c for c in el.children if isinstance(c, Node) and c.tag in ("td", "th")]
                if not cells:
                    continue
                vals = [
                    _clean_inline("".join(_inline(x) for x in c.children)).replace("\n", " ").replace("|", "\\|")
                    for c in cells
                ]
                if header_idx is None and (_inside(el, lambda a: a.tag == "thead") or all(c.tag == "th" for c in cells)):
                    header_idx = len(rows)
                rows.append(vals)
        if not rows:
            return ""
        width = max(len(r) for r in rows)
        rows = [r + [""] * (width - len(r)) for r in rows]
        h = header_idx if header_idx is not None else 0
        header, body = rows[h], rows[:h] + rows[h + 1 :]
        lines = []
        if caption:
            lines += [f"**{caption}**", ""]
        lines.append("| " + " | ".join(header) + " |")
        lines.append("| " + " | ".join("---" for _ in header) + " |")
        lines += ["| " + " | ".join(r) + " |" for r in body]
        return "\n".join(ln.replace("|  |", "| |") for ln in lines)


def _title_and_caption(main: Node, root: Node) -> tuple[str, str, bool]:
    """(h1 text without its caption, caption text, caption-is-bottom).

    New-format pages: the H1 is the entity ("Ibuprofen for adults (Nurofen)")
    and an optional caption lists brands. Old-format multi-page conditions:
    the H1 is the SECTION ("Overview", "Symptoms") and a `nhsuk-caption--bottom`
    caption carries the entity name ("Alzheimer's disease")."""
    h1 = main.find("h1")
    if h1 is None:
        t = root.find("title")
        title = _squash(t.text()) if t else ""
        return re.sub(r"\s+-\s+NHS\s*$", "", title), "", False
    title = _squash(_inline(h1, in_heading=True))  # the caption is a DROP class, so it is excluded here
    caption, bottom = "", False
    for n in h1.iter():
        if "nhsuk-caption-xl" in n.cls():
            bottom = "nhsuk-caption--bottom" in n.cls()
            caption = _squash(
                "".join(
                    (c if isinstance(c, str) else ("" if "nhsuk-u-visually-hidden" in c.cls() else c.text()))
                    for c in n.children
                )
            )
            break
    return title, caption, bottom


def extract_page(html: str, url: str) -> PageDoc:
    root = parse_html(html)
    main = main_node(root)
    heading, caption, bottom = _title_and_caption(main, root)
    title = caption if (bottom and caption) else heading
    r = _Renderer()
    blocks = r.blocks(main)
    if caption and not bottom:
        blocks.insert(0, caption)
    brands: list[str] = []
    m = _BRANDS.search(caption) if (caption and not bottom) else None
    if m:
        brands = [b.strip() for b in re.split(r",|\band\b", m.group(1)) if b.strip()]
    body = "\n\n".join(b for b in blocks if b.strip())
    body = "\n".join(ln.rstrip() for ln in body.split("\n"))
    body = re.sub(r"\n{3,}", "\n\n", body).strip("\n")
    body = body + "\n" if body else ""
    return PageDoc(url=url, title=title, body=body, brands=brands, last_reviewed=r.reviewed[0],
                   next_review=r.reviewed[1], heading=heading)


# ---------------------------------------------------------------------------
# Sub-pages.
# ---------------------------------------------------------------------------


def discover_subpages(html: str, entry_url: str, other_entries: set[str] | frozenset = frozenset()) -> list[str]:
    """Links in <main> (navigation included — that is where hub pages list
    their sub-pages) that sit strictly UNDER the entry's path, in first-seen
    order, excluding URLs that are index entries in their own right."""
    main = main_node(parse_html(html))
    prefix = url_path(entry_url)
    seen: list[str] = []
    for n in main.iter():
        if n.tag != "a":
            continue
        u = normalise_url(n.attrs.get("href", ""), entry_url)
        if u is None or u == entry_url or u in other_entries or u in seen:
            continue
        if url_path(u).startswith(prefix):
            seen.append(u)
    return seen


# ---------------------------------------------------------------------------
# Output.
# ---------------------------------------------------------------------------


@dataclasses.dataclass
class EntryDoc:
    section: str
    slug: str
    url: str
    index_title: str
    aliases: list
    pages: list  # [(PageDoc, retrieved-date)], hub first


_HEADING_LINE = re.compile(r"^(#{1,5}) ", re.M)


def _demote(body: str) -> str:
    return _HEADING_LINE.sub(lambda m: "#" + m.group(1) + " ", body)


def _yaml_value(v) -> str:
    # JSON is a subset of YAML 1.2: every value is emitted as JSON so the
    # front matter is valid YAML AND trivially machine-parseable.
    return json.dumps(v, ensure_ascii=False, sort_keys=True)


def as_at(iso_date: str) -> str:
    """ISO date -> DDMMYY, the form terms §3.6(a) asks for ("as at DDMMYY")."""
    y, m, d = iso_date.split("-")
    return f"{d}{m}{y[2:]}"


def entry_front_matter(e: EntryDoc) -> dict:
    hub, hub_date = e.pages[0]
    title = hub.title or e.index_title
    aliases = list(e.aliases)
    if e.index_title and e.index_title != title and e.index_title not in aliases:
        aliases.append(e.index_title)
    return {
        "title": title,
        "url": e.url,
        "retrieved": hub_date,
        "as_at": as_at(hub_date),
        "licence": LICENCE,
        "attribution": ATTRIBUTION,
        "section": e.section,
        "slug": e.slug,
        "aliases": aliases,
        "brands": hub.brands,
        "sources": [
            {"url": p.url, "title": p.heading or p.title, "retrieved": d, "last_reviewed": p.last_reviewed}
            for p, d in e.pages
        ],
    }


FRONT_MATTER_ORDER = ["title", "url", "retrieved", "as_at", "licence", "attribution", "section", "slug", "aliases", "brands", "sources"]


def entry_body(e: EntryDoc) -> str:
    fm = entry_front_matter(e)
    parts = [f"# {fm['title']}\n"]
    hub = e.pages[0][0]
    if hub.body:
        parts.append(hub.body)
    for p, _d in e.pages[1:]:
        chunk = f"## {p.heading or p.title}\n"
        if p.body:
            chunk += "\n" + _demote(p.body)
        parts.append(chunk)
    return "\n".join(parts)


def render_entry(e: EntryDoc) -> str:
    fm = entry_front_matter(e)
    head = "---\n" + "".join(f"{k}: {_yaml_value(fm[k])}\n" for k in FRONT_MATTER_ORDER) + "---\n"
    return head + entry_body(e)


def split_front_matter(text: str) -> tuple[dict, str]:
    if not text.startswith("---\n"):
        raise ValueError("no front matter")
    end = text.index("\n---\n", 4)
    fm = {}
    for line in text[4:end].split("\n"):
        k, _, v = line.partition(": ")
        fm[k] = json.loads(v)
    return fm, text[end + 5 :]


def write_corpus(root: Path, entries: list[EntryDoc], prune: bool = False) -> dict:
    root.mkdir(parents=True, exist_ok=True)
    manifest_entries = {}
    written: dict[str, set[str]] = {}
    for e in entries:
        text = render_entry(e)
        path = root / e.section / f"{e.slug}.md"
        path.parent.mkdir(parents=True, exist_ok=True)
        data = text.encode("utf-8")
        if not path.exists() or path.read_bytes() != data:
            path.write_bytes(data)
        written.setdefault(e.section, set()).add(path.name)
        fm, body = split_front_matter(text)
        manifest_entries[f"{e.section}/{e.slug}"] = {
            "slug": e.slug,
            "title": fm["title"],
            "url": e.url,
            "section": e.section,
            "sha256": hashlib.sha256(body.encode("utf-8")).hexdigest(),
            "bytes": len(data),
            "pages": len(e.pages),
        }
    if prune:
        for section, names in written.items():
            for p in sorted((root / section).glob("*.md")):
                if p.name not in names:
                    p.unlink()
    counts = {}
    for rec in manifest_entries.values():
        c = counts.setdefault(rec["section"], {"entries": 0, "pages": 0, "bytes": 0})
        c["entries"] += 1
        c["pages"] += rec["pages"]
        c["bytes"] += rec["bytes"]
    manifest = {
        "schema": MANIFEST_SCHEMA,
        "source": "NHS website (www.nhs.uk) Medicines A-Z and Health A-Z",
        "licence": LICENCE,
        "attribution": ATTRIBUTION,
        "sha256_of": "the UTF-8 body of each file (everything after the closing front-matter fence)",
        "counts": counts,
        "entries": manifest_entries,
    }
    data = (json.dumps(manifest, indent=1, sort_keys=True, ensure_ascii=False) + "\n").encode("utf-8")
    mp = root / "index.json"
    if not mp.exists() or mp.read_bytes() != data:
        mp.write_bytes(data)
    return manifest


# ---------------------------------------------------------------------------
# Polite fetching.
# ---------------------------------------------------------------------------


class FetchRefused(RuntimeError):
    pass


class RobotsRules:
    def __init__(self, robots_txt: str):
        self._rp = urllib.robotparser.RobotFileParser()
        self._rp.parse(robots_txt.splitlines())

    def allowed(self, url: str) -> bool:
        return self._rp.can_fetch(USER_AGENT, url)


@dataclasses.dataclass
class Fetched:
    url: str
    final_url: str
    status: int
    html: str
    retrieved: str


def _utc_date() -> str:
    return _dt.datetime.now(_dt.timezone.utc).date().isoformat()


class Fetcher:
    def __init__(self, cache_dir: Path, robots: RobotsRules, delay: float = 1.0, offline: bool = False,
                 max_retries: int = 4, log=None):
        self.cache = cache_dir
        self.robots = robots
        self.delay = delay
        self.offline = offline
        self.max_retries = max_retries
        self._last = 0.0
        self.network_requests = 0
        self.log = log or (lambda msg: None)

    def _key(self, url: str) -> Path:
        return self.cache / hashlib.sha256(url.encode("utf-8")).hexdigest()

    def store(self, url: str, final_url: str, html: str, retrieved: str, status: int = 200) -> None:
        self.cache.mkdir(parents=True, exist_ok=True)
        k = self._key(url)
        k.with_suffix(".html").write_text(html, encoding="utf-8")
        meta = {"url": url, "final_url": final_url, "status": status, "retrieved": retrieved}
        k.with_suffix(".json").write_text(json.dumps(meta, sort_keys=True) + "\n", encoding="utf-8")

    def _cached(self, url: str) -> Fetched | None:
        k = self._key(url)
        mj, mh = k.with_suffix(".json"), k.with_suffix(".html")
        if not (mj.is_file() and mh.is_file()):
            return None
        meta = json.loads(mj.read_text(encoding="utf-8"))
        return Fetched(url, meta["final_url"], meta["status"], mh.read_text(encoding="utf-8"), meta["retrieved"])

    def check(self, url: str) -> None:
        p = urllib.parse.urlsplit(url)
        if p.scheme != "https" or p.netloc != HOST:
            raise FetchRefused(f"refusing off-site URL {url}")
        path = p.path if p.path.endswith("/") else p.path + "/"
        if path.lower().startswith(EXCLUDED_PREFIXES):
            raise FetchRefused(f"{url} is on a site excluded from the NHS website's OGL terms")
        if not self.robots.allowed(url):
            raise FetchRefused(f"robots.txt disallows {url}")

    def get(self, url: str) -> Fetched:
        self.check(url)
        hit = self._cached(url)
        if hit is not None:
            return hit
        if self.offline:
            raise FetchRefused(f"offline and not cached: {url}")
        attempt = 0
        while True:
            wait = self.delay - (time.monotonic() - self._last)
            if wait > 0:
                time.sleep(wait)
            self._last = time.monotonic()
            self.network_requests += 1
            req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Accept": "text/html"})
            try:
                with urllib.request.urlopen(req, timeout=30) as resp:
                    final = resp.geturl()
                    html = resp.read().decode(resp.headers.get_content_charset() or "utf-8", "replace")
                    status = resp.status
            except urllib.error.HTTPError as exc:
                if exc.code in (429, 500, 502, 503, 504) and attempt < self.max_retries:
                    ra = exc.headers.get("Retry-After", "") if exc.headers else ""
                    backoff = float(ra) if ra.isdigit() else 5.0 * (2 ** attempt)
                    self.log(f"[fetch] {exc.code} on {url}; backing off {backoff:.0f}s")
                    time.sleep(backoff)
                    attempt += 1
                    continue
                if exc.code in (404, 410):
                    self.store(url, url, "", _utc_date(), status=exc.code)
                    return self._cached(url)
                raise
            except (urllib.error.URLError, TimeoutError) as exc:
                if attempt < self.max_retries:
                    backoff = 5.0 * (2 ** attempt)
                    self.log(f"[fetch] {exc} on {url}; backing off {backoff:.0f}s")
                    time.sleep(backoff)
                    attempt += 1
                    continue
                raise
            final_n = normalise_url(final, url) or final
            if final_n != url:
                # a redirect must land on-site and robots-allowed too
                self.check(final_n)
            self.store(url, final_n, html, _utc_date(), status=status)
            return self._cached(url)


# ---------------------------------------------------------------------------
# The crawl.
# ---------------------------------------------------------------------------


def crawl(fetcher: Fetcher, sections: list[str], limit: int | None, log) -> tuple[list[EntryDoc], dict]:
    stats = {}
    indexes: dict[str, list[IndexEntry]] = {}
    for s in sections:
        idx = fetcher.get(SECTIONS[s])
        if idx.status != 200:
            raise RuntimeError(f"index {SECTIONS[s]} returned {idx.status}")
        indexes[s] = parse_index(idx.html, SECTIONS[s])
    all_entry_urls = {e.url for es in indexes.values() for e in es}

    docs: list[EntryDoc] = []
    for s in sections:
        entries = indexes[s][:limit] if limit else indexes[s]
        # Resolve redirects first so two index entries that land on one page
        # become ONE entry (the second's title becomes an alias).
        resolved: list[tuple[IndexEntry, Fetched]] = []
        by_final: dict[str, int] = {}
        skipped = []
        for i, e in enumerate(entries):
            log(f"[{s}] {i + 1}/{len(entries)} {e.url}")
            try:
                f = fetcher.get(e.url)
            except FetchRefused as exc:
                skipped.append({"url": e.url, "why": str(exc)})
                continue
            if f.status != 200 or not f.html:
                skipped.append({"url": e.url, "why": f"HTTP {f.status}"})
                continue
            if "no-longer-available" in url_path(f.final_url):
                # a retired entry redirected to a generic tombstone page: not content
                skipped.append({"url": e.url, "why": f"redirects to {f.final_url} (page no longer available)"})
                continue
            if f.final_url in by_final:
                prev = resolved[by_final[f.final_url]][0]
                for a in [e.index_title, *e.aliases]:
                    if a and a != prev.index_title and a not in prev.aliases:
                        prev.aliases.append(a)
                continue
            by_final[f.final_url] = len(resolved)
            resolved.append((IndexEntry(f.final_url, e.index_title, list(e.aliases)), f))
        slugs = assign_slugs([e.url for e, _ in resolved])
        finals = {e.url for e, _ in resolved} | all_entry_urls
        n_pages = 0
        for e, hubf in resolved:
            hub = extract_page(hubf.html, e.url)
            pages = [(hub, hubf.retrieved)]
            queue = discover_subpages(hubf.html, e.url, finals - {e.url})
            seen = {e.url, *queue}
            got: set[str] = set()
            while queue and len(pages) <= MAX_SUBPAGES:
                u = queue.pop(0)
                try:
                    sf = fetcher.get(u)
                except FetchRefused as exc:
                    skipped.append({"url": u, "why": str(exc)})
                    continue
                if sf.status != 200 or not sf.html:
                    skipped.append({"url": u, "why": f"HTTP {sf.status}"})
                    continue
                fu = sf.final_url
                if fu == e.url or fu in got or not url_path(fu).startswith(url_path(e.url)):
                    continue  # redirected back to the hub, a duplicate, or out of this entry
                got.add(fu)
                pages.append((extract_page(sf.html, fu), sf.retrieved))
                for more in discover_subpages(sf.html, e.url, finals - {e.url}):
                    if more not in seen:
                        seen.add(more)
                        queue.append(more)
            n_pages += len(pages)
            docs.append(EntryDoc(s, slugs[e.url], e.url, e.index_title, list(e.aliases), pages))
        stats[s] = {"index_links": len(indexes[s]), "entries": len(resolved), "pages": n_pages, "skipped": skipped}
    return docs, stats


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Fetch the NHS Medicines A-Z and Health A-Z into tools/reference/corpus/.")
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT)
    ap.add_argument("--cache", type=Path, default=DEFAULT_CACHE)
    ap.add_argument("--delay", type=float, default=1.0, help="minimum seconds between network requests (default 1.0; floor 1.0)")
    ap.add_argument("--offline", action="store_true", help="never touch the network; render from the cache only")
    ap.add_argument("--section", choices=["medicines", "conditions", "all"], default="all")
    ap.add_argument("--limit", type=int, default=None, help="first N entries per section (smoke test; disables pruning)")
    args = ap.parse_args(argv)
    delay = max(args.delay, 1.0)

    def log(msg: str) -> None:
        print(msg, file=sys.stderr, flush=True)

    robots_cache = args.cache / "robots.txt"
    if args.offline:
        if not robots_cache.is_file():
            print("error: --offline but no cached robots.txt", file=sys.stderr)
            return 1
        robots_txt = robots_cache.read_text(encoding="utf-8")
    else:
        req = urllib.request.Request(f"{BASE}/robots.txt", headers={"User-Agent": USER_AGENT})
        with urllib.request.urlopen(req, timeout=30) as resp:
            robots_txt = resp.read().decode("utf-8", "replace")
        args.cache.mkdir(parents=True, exist_ok=True)
        robots_cache.write_text(robots_txt, encoding="utf-8")
        time.sleep(delay)
    fetcher = Fetcher(args.cache, RobotsRules(robots_txt), delay=delay, offline=args.offline, log=log)
    sections = ["medicines", "conditions"] if args.section == "all" else [args.section]
    docs, stats = crawl(fetcher, sections, args.limit, log)
    manifest = write_corpus(args.out, docs, prune=(args.limit is None and args.section == "all"))
    report = {"network_requests": fetcher.network_requests, "sections": stats, "counts": manifest["counts"]}
    print(json.dumps(report, indent=1, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
