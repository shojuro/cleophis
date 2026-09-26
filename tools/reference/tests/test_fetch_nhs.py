#!/usr/bin/env python3
"""Phase 1h Task M4a — fetch_nhs.py: index parsing, article extraction, output shape.

No network: every test runs on small synthetic HTML modelled on the live
nhs.uk markup (inspected 2026-09-27). Run:

    python3 -m pytest -q tools/reference/tests/test_fetch_nhs.py
"""

from __future__ import annotations

import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

REFERENCE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REFERENCE))

import fetch_nhs as fn  # noqa: E402

INDEX_HTML = """<html><body><header><a href="/conditions/header-link/">Header</a></header>
<main id="maincontent">
<nav><ol><li><a href="/conditions/#a">A</a></li><li><span>X</span></li></ol></nav>
<div class="nhsuk-card"><h2 id="a">A</h2><ul class="nhsuk-list">
<li><a href="/conditions/acne/">
   Acne
</a></li>
<li><a href="/conditions/heartburn-and-acid-reflux/">Acid reflux, see Heartburn and acid reflux</a></li>
<li><a href="/conditions/heartburn-and-acid-reflux/">Heartburn and acid reflux</a></li>
<li><a href="/mental-health/conditions/depression-in-adults/">Depression in adults</a></li>
<li><a href="https://www.nhs.uk/conditions/acne/">Acne</a></li>
<li><a href="https://example.org/elsewhere/">Off-site</a></li>
</ul></div></main><footer><a href="/conditions/footer/">F</a></footer></body></html>"""

ARTICLE_HTML = """<html><body><header>SITE HEADER</header>
<nav class="nhsuk-breadcrumb"><a href="/">Home</a></nav>
<main class="nhsuk-main-wrapper" id="maincontent">
  <h1><span role="text">Ibuprofen for adults (Nurofen) <span class="nhsuk-caption-xl"><span class="nhsuk-u-visually-hidden"> - </span>Other common brands: Brufen, Calprofen</span></span></h1>
  <p class="nhsuk-lede-text">Ibuprofen is a   painkiller.</p>
  <article>
  <section><h2 id="what">What ibuprofen is for</h2>
    <p>It helps with <a href="/conditions/headaches/">headaches</a> and <b>period pain</b>.</p>
    <ul><li>toothache</li><li>joint pain<ul><li>knees</li></ul></li></ul>
    <img src="/x.png" alt="a picture">
    <figure><img src="/y.png"><figcaption>Credit: stock library</figcaption></figure>
    <iframe src="https://www.youtube.com/embed/abc"></iframe>
  </section>
  <div class="nhsuk-inset-text"><span class="nhsuk-u-visually-hidden">Information: </span><p>Separate page for children.</p></div>
  <section><h2>Dose</h2>
    <details class="nhsuk-details"><summary class="nhsuk-details__summary"><span class="nhsuk-details__summary-text">200mg tablets</span></summary>
      <div class="nhsuk-details__text"><ol><li>swallow 1 or 2 tablets</li><li>wait 4 hours</li></ol></div></details>
    <div class="nhsuk-card nhsuk-card--care nhsuk-card--care--urgent"><div class="nhsuk-card__heading-container">
      <h4 class="nhsuk-card__heading"><span role="text"><span class="nhsuk-u-visually-hidden">Urgent advice: </span>Get help from NHS 111 if:</span></h4></div>
      <div class="nhsuk-card__content"><ul><li>you took too much</li></ul>
      <a class="nhsuk-action-link" href="https://www.nhs.uk/service-search/pharmacy/find-a-pharmacy"><span class="nhsuk-action-link__text">Find a pharmacy</span></a></div></div>
    <table class="nhsuk-table"><caption>Doses by age</caption><thead><tr><th>Age</th><th>Dose</th></tr></thead>
      <tbody><tr><td>3 to 6 months</td><td>2.5ml | once</td></tr></tbody></table>
    <script>var x = 1;</script><style>p{}</style>
  </section>
  <ul class="nhsuk-hub-key-links"><li><a href="/medicines/ibuprofen-for-adults/about/">About</a></li></ul>
  <div class="beta-hub-related-links-title"><h2>Related information</h2></div>
  <ul class="beta-hub-related-links"><li><a href="/conditions/other/">Unrelated page</a></li></ul>
  </article>
  <aside><div id="sibling-nav"><h2>More in <a href="/x/">X</a></h2></div></aside>
  <div><p class="nhsuk-body-s">Page last reviewed: 27 August 2025<br>Next review due: 27 August 2028</p></div>
</main><footer>SITE FOOTER</footer></body></html>"""

HUB_HTML = """<html><body><main id="maincontent"><h1>Type 2 diabetes</h1>
<p class="nhsuk-lede-text">Lede.</p>
<ul class="nhsuk-hub-key-links">
<li><a href="https://www.nhs.uk/conditions/type-2-diabetes/symptoms/">Symptoms</a></li>
<li><a href="/conditions/type-2-diabetes/treatment/">Treatment</a></li>
<li><a href="https://www.nhs.uk/conditions/type-2-diabetes/symptoms/#x">Symptoms again</a></li>
<li><a href="/conditions/type-2-diabetes/">Self</a></li>
<li><a href="/conditions/type-2-diabetes-in-children/">A different entry</a></li>
<li><a href="/conditions/type-1-diabetes/">Sibling</a></li>
<li><a href="/conditions/type-2-diabetes/food/">Also an index entry</a></li>
</ul></main></body></html>"""


class IndexParsing(unittest.TestCase):
    def test_entries_aliases_and_dedupe(self):
        entries = fn.parse_index(INDEX_HTML, "https://www.nhs.uk/conditions/")
        urls = [e.url for e in entries]
        self.assertEqual(
            urls,
            [
                "https://www.nhs.uk/conditions/acne/",
                "https://www.nhs.uk/conditions/heartburn-and-acid-reflux/",
                "https://www.nhs.uk/mental-health/conditions/depression-in-adults/",
            ],
        )
        heartburn = entries[1]
        self.assertEqual(heartburn.index_title, "Heartburn and acid reflux")
        self.assertEqual(heartburn.aliases, ["Acid reflux"])
        self.assertEqual(entries[0].aliases, [])  # duplicate "Acne" link adds nothing

    def test_letter_anchors_header_footer_and_offsite_are_ignored(self):
        entries = fn.parse_index(INDEX_HTML, "https://www.nhs.uk/conditions/")
        for e in entries:
            self.assertNotIn("#", e.url)
            self.assertTrue(e.url.startswith("https://www.nhs.uk/"))
            self.assertNotIn("header-link", e.url)
            self.assertNotIn("footer", e.url)


class ArticleExtraction(unittest.TestCase):
    def setUp(self):
        self.page = fn.extract_page(ARTICLE_HTML, "https://www.nhs.uk/medicines/ibuprofen-for-adults/")

    def test_title_excludes_caption_and_brands_are_parsed(self):
        self.assertEqual(self.page.title, "Ibuprofen for adults (Nurofen)")
        self.assertEqual(self.page.brands, ["Brufen", "Calprofen"])

    def test_review_dates_extracted_and_not_in_body(self):
        self.assertEqual(self.page.last_reviewed, "27 August 2025")
        self.assertEqual(self.page.next_review, "27 August 2028")
        self.assertNotIn("Page last reviewed", self.page.body)
        self.assertNotIn("2025", self.page.body)

    def test_markdown_structure(self):
        body = self.page.body
        self.assertIn("Other common brands: Brufen, Calprofen", body)
        self.assertIn("Ibuprofen is a painkiller.", body)
        self.assertIn("## What ibuprofen is for", body)
        self.assertIn("It helps with headaches and **period pain**.", body)
        self.assertIn("- toothache", body)
        self.assertIn("- joint pain\n  - knees", body)
        self.assertIn("**200mg tablets**", body)
        self.assertIn("1. swallow 1 or 2 tablets\n2. wait 4 hours", body)
        self.assertIn("#### Urgent advice: Get help from NHS 111 if:", body)
        self.assertIn("- you took too much", body)
        self.assertIn("**Doses by age**", body)
        # a screen-reader-only label standing alone is dropped; one inside a heading stays
        self.assertIn("\n\nSeparate page for children.\n\n", body)
        self.assertNotIn("Information:", body)
        self.assertIn("| Age | Dose |\n| --- | --- |\n| 3 to 6 months | 2.5ml \\| once |", body)

    def test_boilerplate_images_embeds_scripts_dropped(self):
        body = self.page.body
        for gone in (
            "SITE HEADER", "SITE FOOTER", "Home", "a picture", "Credit: stock library", "youtube",
            "var x", "p{}", "Find a pharmacy", "About", "Related information", "Unrelated page", "More in",
            "http", "Ibuprofen for adults (Nurofen)",  # title lives in the entry heading, not twice
        ):
            self.assertNotIn(gone, body, gone)

    def test_body_whitespace_is_normalised(self):
        body = self.page.body
        self.assertFalse(body.startswith("\n"))
        self.assertTrue(body.endswith("\n") and not body.endswith("\n\n"))
        self.assertNotIn("\n\n\n", body)
        for line in body.splitlines():
            self.assertEqual(line, line.rstrip(), repr(line))


class OldFormatCaption(unittest.TestCase):
    HTML = """<main><h1><span role="text">Symptoms
    <span class="nhsuk-caption-xl nhsuk-caption--bottom"><span class="nhsuk-u-visually-hidden"> - </span>
      Alzheimer&#x27;s disease</span></span></h1><p>Memory problems.</p></main>"""

    def test_bottom_caption_is_the_entity_and_h1_the_section(self):
        p = fn.extract_page(self.HTML, "https://www.nhs.uk/conditions/alzheimers-disease/symptoms/")
        self.assertEqual(p.title, "Alzheimer's disease")
        self.assertEqual(p.heading, "Symptoms")
        self.assertEqual(p.brands, [])
        self.assertEqual(p.body, "Memory problems.\n")

    def test_new_format_heading_is_the_title(self):
        p = fn.extract_page(ARTICLE_HTML, "https://www.nhs.uk/medicines/ibuprofen-for-adults/")
        self.assertEqual(p.heading, "Ibuprofen for adults (Nurofen)")

    def test_subpage_section_uses_the_heading(self):
        hub = fn.extract_page(self.HTML.replace("Symptoms", "Overview"), "https://www.nhs.uk/conditions/alzheimers-disease/")
        sub = fn.extract_page(self.HTML, "https://www.nhs.uk/conditions/alzheimers-disease/symptoms/")
        e = fn.EntryDoc("conditions", "alzheimers-disease", hub.url, "Alzheimer's disease", [],
                        [(hub, "2026-09-27"), (sub, "2026-09-27")])
        fm, body = fn.split_front_matter(fn.render_entry(e))
        self.assertEqual(fm["title"], "Alzheimer's disease")
        self.assertEqual(fm["aliases"], [])
        self.assertEqual([s["title"] for s in fm["sources"]], ["Overview", "Symptoms"])
        self.assertTrue(body.startswith("# Alzheimer's disease\n\nMemory problems.\n\n## Symptoms\n"))


class SubpageDiscovery(unittest.TestCase):
    def test_prefix_bounded_ordered_and_excludes_other_entries(self):
        subs = fn.discover_subpages(
            HUB_HTML,
            "https://www.nhs.uk/conditions/type-2-diabetes/",
            other_entries={"https://www.nhs.uk/conditions/type-2-diabetes/food/"},
        )
        self.assertEqual(
            subs,
            [
                "https://www.nhs.uk/conditions/type-2-diabetes/symptoms/",
                "https://www.nhs.uk/conditions/type-2-diabetes/treatment/",
            ],
        )


class OutputShape(unittest.TestCase):
    def _entry(self):
        hub = fn.extract_page(ARTICLE_HTML, "https://www.nhs.uk/medicines/ibuprofen-for-adults/")
        sub = fn.extract_page(
            "<main><h1>Side effects</h1><h2>Common</h2><p>Nausea.</p></main>",
            "https://www.nhs.uk/medicines/ibuprofen-for-adults/side-effects/",
        )
        return fn.EntryDoc(
            section="medicines",
            slug="ibuprofen-for-adults",
            url="https://www.nhs.uk/medicines/ibuprofen-for-adults/",
            index_title="Ibuprofen for adults (Nurofen)",
            aliases=[],
            pages=[(hub, "2026-09-27"), (sub, "2026-09-28")],
        )

    def test_front_matter_and_body(self):
        text = fn.render_entry(self._entry())
        fm, body = fn.split_front_matter(text)
        self.assertEqual(fm["title"], "Ibuprofen for adults (Nurofen)")
        self.assertEqual(fm["url"], "https://www.nhs.uk/medicines/ibuprofen-for-adults/")
        self.assertEqual(fm["retrieved"], "2026-09-27")
        self.assertEqual(fm["licence"], "OGL v3")
        self.assertEqual(
            fm["attribution"], "Information from the NHS website, licensed under the Open Government Licence v3.0"
        )
        self.assertEqual(fm["section"], "medicines")
        # terms §3.6(a): a copy not refreshed every 7 days is attributed "as at DDMMYY"
        self.assertEqual(fm["as_at"], "270926")
        self.assertEqual(fm["brands"], ["Brufen", "Calprofen"])
        self.assertEqual([s["url"] for s in fm["sources"]], [
            "https://www.nhs.uk/medicines/ibuprofen-for-adults/",
            "https://www.nhs.uk/medicines/ibuprofen-for-adults/side-effects/",
        ])
        self.assertEqual(fm["sources"][1]["retrieved"], "2026-09-28")
        self.assertTrue(body.startswith("# Ibuprofen for adults (Nurofen)\n"))
        # the subpage becomes an H2 and its own headings are demoted one level
        self.assertIn("\n## Side effects\n\n### Common\n\nNausea.\n", body)
        self.assertNotIn("2026", body)

    def test_render_is_deterministic_and_manifest_hashes_body(self):
        a = fn.render_entry(self._entry())
        b = fn.render_entry(self._entry())
        self.assertEqual(a, b)
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            fn.write_corpus(root, [self._entry()])
            md = (root / "medicines" / "ibuprofen-for-adults.md").read_text(encoding="utf-8")
            self.assertEqual(md, a)
            manifest = json.loads((root / "index.json").read_text(encoding="utf-8"))
            rec = manifest["entries"]["medicines/ibuprofen-for-adults"]
            _fm, body = fn.split_front_matter(md)
            self.assertEqual(rec["sha256"], hashlib.sha256(body.encode("utf-8")).hexdigest())
            self.assertEqual(rec["slug"], "ibuprofen-for-adults")
            self.assertEqual(rec["title"], "Ibuprofen for adults (Nurofen)")
            self.assertEqual(rec["section"], "medicines")
            self.assertEqual(rec["url"], "https://www.nhs.uk/medicines/ibuprofen-for-adults/")
            # a rewrite is byte-identical
            before = (root / "index.json").read_bytes()
            fn.write_corpus(root, [self._entry()])
            self.assertEqual((root / "index.json").read_bytes(), before)


class Crawl(unittest.TestCase):
    def test_redirects_to_a_tombstone_page_are_skipped_and_duplicates_merge(self):
        idx = """<main><ul class="nhsuk-list">
<li><a href="/medicines/aciclovir/">Aciclovir</a></li>
<li><a href="/medicines/budesonide-nasal-spray/">Budesonide nasal spray</a></li>
<li><a href="/medicines/zovirax/">Zovirax</a></li>
</ul></main>"""
        with tempfile.TemporaryDirectory() as d:
            f = fn.Fetcher(Path(d), robots=fn.RobotsRules(""), delay=0.0, offline=True)
            f.store(fn.SECTIONS["medicines"], fn.SECTIONS["medicines"], idx, "2026-09-27")
            f.store("https://www.nhs.uk/medicines/aciclovir/", "https://www.nhs.uk/medicines/aciclovir/",
                    "<main><h1>Aciclovir</h1><p>Antiviral.</p></main>", "2026-09-27")
            f.store("https://www.nhs.uk/medicines/budesonide-nasal-spray/",
                    "https://www.nhs.uk/medicine-page-no-longer-available/",
                    "<main><h1>Medicine page no longer available</h1></main>", "2026-09-27")
            f.store("https://www.nhs.uk/medicines/zovirax/", "https://www.nhs.uk/medicines/aciclovir/",
                    "<main><h1>Aciclovir</h1><p>Antiviral.</p></main>", "2026-09-27")
            docs, stats = fn.crawl(f, ["medicines"], None, lambda m: None)
        self.assertEqual([x.slug for x in docs], ["aciclovir"])
        self.assertEqual(docs[0].aliases, ["Zovirax"])
        self.assertEqual(len(stats["medicines"]["skipped"]), 1)
        self.assertIn("no longer available", stats["medicines"]["skipped"][0]["why"])


class Slugs(unittest.TestCase):
    def test_slug_is_last_segment_and_collisions_are_disambiguated(self):
        urls = [
            "https://www.nhs.uk/conditions/acne/",
            "https://www.nhs.uk/mental-health/conditions/depression/",
            "https://www.nhs.uk/conditions/depression/",
        ]
        self.assertEqual(
            fn.assign_slugs(urls),
            {
                urls[0]: "acne",
                urls[1]: "mental-health--conditions--depression",
                urls[2]: "conditions--depression",
            },
        )


class Politeness(unittest.TestCase):
    ROBOTS = "User-agent: *\nDisallow: /Conditions/\nDisallow: /service-search/x\n"

    def test_robots_is_case_sensitive_and_enforced(self):
        rules = fn.RobotsRules(self.ROBOTS)
        self.assertTrue(rules.allowed("https://www.nhs.uk/conditions/acne/"))
        self.assertFalse(rules.allowed("https://www.nhs.uk/Conditions/acne/"))
        self.assertFalse(rules.allowed("https://www.nhs.uk/service-search/x/y"))

    def test_fetcher_refuses_disallowed_and_offsite_urls_without_network(self):
        with tempfile.TemporaryDirectory() as d:
            f = fn.Fetcher(Path(d), robots=fn.RobotsRules(self.ROBOTS), delay=0.0, offline=True)
            with self.assertRaises(fn.FetchRefused):
                f.get("https://www.nhs.uk/Conditions/acne/")
            with self.assertRaises(fn.FetchRefused):
                f.get("https://example.org/")

    def test_sites_excluded_from_the_standard_terms_are_refused(self):
        # nhs.uk/our-policies/terms-and-conditions/content-not-licensed-for-re-use/ (reviewed 17 April 2024)
        f = fn.Fetcher(Path("/nonexistent"), robots=fn.RobotsRules(""), delay=0.0, offline=True)
        for u in (
            "https://www.nhs.uk/change4life/", "https://www.nhs.uk/best-start-in-life/x/",
            "https://www.nhs.uk/be-clear-on-cancer/", "https://www.nhs.uk/oneyou/a/",
            "https://www.nhs.uk/smokefree/", "https://www.nhs.uk/quit/",
        ):
            with self.assertRaises(fn.FetchRefused, msg=u):
                f.check(u)
        f.check("https://www.nhs.uk/conditions/quitting-smoking/")  # a prefix match is by path segment

    def test_redirect_hops_are_checked_before_they_are_requested(self):
        import urllib.request
        f = fn.Fetcher(Path("/nonexistent"), robots=fn.RobotsRules(self.ROBOTS), delay=0.0)
        handler = fn.CheckedRedirectHandler(f)
        req = urllib.request.Request("https://www.nhs.uk/conditions/acne/")
        for bad in ("https://example.org/x", "https://www.nhs.uk/Conditions/acne/", "https://www.nhs.uk/smokefree/"):
            with self.assertRaises(fn.FetchRefused, msg=bad):
                handler.redirect_request(req, None, 301, "Moved", {}, bad)
        ok = handler.redirect_request(req, None, 301, "Moved", {}, "https://www.nhs.uk/conditions/acne-new/")
        self.assertEqual(ok.full_url, "https://www.nhs.uk/conditions/acne-new/")

    def test_offline_fetch_uses_cache_only(self):
        with tempfile.TemporaryDirectory() as d:
            f = fn.Fetcher(Path(d), robots=fn.RobotsRules(""), delay=0.0, offline=True)
            with self.assertRaises(fn.FetchRefused):
                f.get("https://www.nhs.uk/conditions/acne/")  # not cached, offline
            f.store("https://www.nhs.uk/conditions/acne/", "https://www.nhs.uk/conditions/acne/", "<main>x</main>", "2026-09-27")
            got = f.get("https://www.nhs.uk/conditions/acne/")
            self.assertEqual(got.html, "<main>x</main>")
            self.assertEqual(got.retrieved, "2026-09-27")


if __name__ == "__main__":
    unittest.main()
