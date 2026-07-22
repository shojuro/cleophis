#!/usr/bin/env python3
"""Multi-turn cascade-recovery examples for contract-v2.

v1's E2E failure was a REFUSAL CASCADE: an early (often correct) refusal in the
conversation history poisons the following answerable grounded turns, so
"summarize" refuses mid-chat even though it answers in isolation. These examples
teach the model to judge EACH turn on its own merits against the current
sources: after a refusal, an answerable follow-up (incl. "summarize") is ANSWERED
and cited. Built on the SAME contract as v1 (byte-faithful system via
gen_contract_v1.assemble_system). Output: chat JSONL, one conversation per line.
"""
import argparse
import json
import os
import random
import re
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from gen_contract_v1 import (  # noqa: E402
    load_contract, assemble_system, deepseek, envval, DOMAINS, ENV,
)

GEN_SYS = (
    "You are a data engineer building MULTI-TURN fine-tuning conversations that teach a "
    "retrieval-grounded model to judge EACH turn on its own merits against the provided sources. "
    "Output ONLY a single JSON object, no prose. Be realistic and domain-accurate; the `text` "
    "fields are excerpt bodies (2-5 sentences), NOT including their titles."
)


def instruction(domain):
    return (
        f"Domain: {domain}. Produce JSON with keys: "
        '"sources" (array of 3-5 objects {"title","section","locator","text"} on ONE coherent topic), '
        '"uncovered_question" (a question RELATED to the topic but genuinely NOT answerable from the '
        'sources — a real hard negative), '
        '"covered_question" (a specific question that IS fully answerable from the sources), '
        '"covered_answer" (the ideal reply to covered_question, drawn ONLY from the sources, citing '
        'each supporting source inline as [1],[2],... with real indices), '
        '"summary_answer" (a concise synthesis of the sources into the key points, citing [n] for each '
        "point). Use ONLY the sources in both answers; do not use outside knowledge."
    )


SUMMARIZE_Q = [
    "summarize", "summarize the sources", "summarize the source",
    "give me the key points", "can you summarize this?", "bullet points please",
]


def build(c, g, domain):
    chunks = g.get("sources") or []
    if len(chunks) < 2:
        raise ValueError("need >=2 sources")
    sysmsg = assemble_system(c, chunks, no_evidence=False)
    refusal = c["refusal_with_offer"].strip()
    unc = g["uncovered_question"].strip()
    out = []
    # (1) prior REFUSAL -> covered follow-up is ANSWERED (cited)
    out.append({"messages": [
        {"role": "system", "content": sysmsg},
        {"role": "user", "content": unc},
        {"role": "assistant", "content": refusal},
        {"role": "user", "content": g["covered_question"].strip()},
        {"role": "assistant", "content": g["covered_answer"].strip()},
    ], "source": "synthetic_deepseek", "category": f"contract_mt_recover_cite_{domain}"})
    # (2) prior REFUSAL -> "summarize" is ANSWERED (the exact cascade trigger)
    out.append({"messages": [
        {"role": "system", "content": sysmsg},
        {"role": "user", "content": unc},
        {"role": "assistant", "content": refusal},
        {"role": "user", "content": random.choice(SUMMARIZE_Q)},
        {"role": "assistant", "content": g["summary_answer"].strip()},
    ], "source": "synthetic_deepseek", "category": f"contract_mt_recover_summarize_{domain}"})
    return out


def validate(ex):
    msgs = ex["messages"]
    roles = [m["role"] for m in msgs]
    assert roles == ["system", "user", "assistant", "user", "assistant"], roles
    assert msgs[2]["content"].strip().startswith("I don't have material"), "turn-1 must be the refusal"
    last = msgs[-1]["content"]
    assert re.search(r"\[\d+\]", last), "recovery answer must cite [n]"
    low = last.lower()
    assert "don't have material" not in low and "add a source and rebuild" not in low, \
        "recovery turn must NOT be a refusal"
    return ex


def gen_one(key, c, domain):
    g = deepseek(key, GEN_SYS, instruction(domain), temperature=0.9)
    return [validate(e) for e in build(c, g, domain)]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--per-domain", type=int, default=100, help="deepseek calls per domain (yields 2x examples)")
    ap.add_argument("--workers", type=int, default=12)
    ap.add_argument("--out", default="work/contract-v2/multiturn.jsonl")
    args = ap.parse_args()

    key = envval("DEEPSEEK_API", "DEEPSEEK_API_KEY") or sys.exit(f"no DEEPSEEK_API in {ENV}")
    c = load_contract()
    tasks = [d for d in DOMAINS for _ in range(args.per_domain)]
    os.makedirs(os.path.dirname(os.path.abspath(args.out)) or ".", exist_ok=True)

    examples, fails = [], 0
    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = {ex.submit(gen_one, key, c, d): d for d in tasks}
        for i, fut in enumerate(as_completed(futs), 1):
            try:
                examples.extend(fut.result())
            except Exception as e:  # noqa: BLE001
                fails += 1
            if i % 25 == 0:
                print(f"  {i}/{len(tasks)} calls done, {len(examples)} examples, {fails} fails")

    with open(args.out, "w", encoding="utf-8") as f:
        for e in examples:
            f.write(json.dumps(e, ensure_ascii=False) + "\n")
    print(f"[done] wrote {len(examples)} multi-turn examples to {args.out} ({fails} failed calls)")


if __name__ == "__main__":
    main()
