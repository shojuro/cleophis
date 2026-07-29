import json, re, sys

W = "/mnt/c/Users/JM505 Computers/dev/cleophis/.claude/worktrees/hyperframe-marketing-sandbox/videos/houseofel-recut/transcript.json"
words = json.load(open(W))
tw = [x["text"] for x in words]
low = [t.lower().strip(".,?!\"'") for t in tw]

# (anchor phrase in speech, on-screen emphasis words)
CAND = [
    ("rapidly losing the ability", "LOSING THE ABILITY TO READ"),
    ("what the research says", "WHAT THE RESEARCH SAYS"),
    ("outsourcing our thinking", "OUTSOURCING OUR THINKING"),
    ("weakest neural connectivity", "WEAKEST NEURAL CONNECTIVITY"),
    ("strongest, most distributed", "STRONGEST NEURAL NETWORKS"),
    ("could not quote a single", "83% COULDN'T QUOTE A LINE"),
    ("total cognitive amnesia", "TOTAL COGNITIVE AMNESIA"),
    ("screen saver mode", "SCREENSAVER MODE"),
    ("sample size is 54", "SAMPLE SIZE: 54"),
    ("did not return to normal", "DID NOT RETURN TO NORMAL"),
    ("cognitive debt persisted", "THE COGNITIVE DEBT PERSISTED"),
    ("319 knowledge workers", "319 KNOWLEDGE WORKERS"),
    ("less critical thinking they applied", "LESS CRITICAL THINKING"),
    ("let go of the wheel", "LET GO OF THE WHEEL"),
    ("diminished skill for independent", "DIMINISHED INDEPENDENT SKILL"),
    ("strong negative correlation", "STRONG NEGATIVE CORRELATION"),
    ("mediating factor was cognitive", "COGNITIVE OFFLOADING"),
    ("17% worse on conceptual", "17% WORSE ON CONCEPTS"),
    ("output without necessarily", "OUTPUT WITHOUT UNDERSTANDING"),
    ("was retracted last month", "RETRACTED"),
    ("students who critically engaged", "CRITICAL ENGAGEMENT WINS"),
    ("the tool is not the problem", "THE TOOL IS NOT THE PROBLEM"),
    ("use it or lose it", "USE IT OR LOSE IT"),
    ("thinking requires energy", "THINKING REQUIRES ENERGY"),
    ("it is profoundly human", "PROFOUNDLY HUMAN"),
    ("didn't destroy critical thinking", "AI DIDN'T DESTROY IT"),
    ("academically adrift", "ACADEMICALLY ADRIFT"),
    ("argument for acting faster", "AN ARGUMENT FOR ACTING FASTER"),
    ("46% of her students", "46% CONFIDENT IN SEPTEMBER"),
    ("that number was 95%", "95% BY FEBRUARY"),
    ("removing the tool is what rebuilds", "REMOVING THE TOOL REBUILDS IT"),
    ("may never build it at all", "THEY MAY NEVER BUILD IT"),
    ("replaces the development", "REPLACES THE DEVELOPMENT"),
    ("kind of sounds the same", "EVERYONE SOUNDS THE SAME"),
    ("reasoning becomes the student's", "THE MODEL'S REASONING BECOMES THEIRS"),
    ("biases become their defaults", "ITS BIASES BECOME DEFAULTS"),
    ("reduced available cognitive", "REDUCED COGNITIVE CAPACITY"),
    ("social media captured attention", "SOCIAL MEDIA CAPTURED ATTENTION"),
    ("captures thinking itself", "AI CAPTURES THINKING ITSELF"),
    ("70% of students themselves", "70% OF STUDENTS ARE WORRIED"),
    ("that is data", "THAT IS DATA"),
    ("one tool, every context", "ONE TOOL. EVERY CONTEXT."),
    ("misusing the infrastructure", "MISUSING THE INFRASTRUCTURE"),
    ("refusing to just hand over", "REFUSE TO HAND OVER THE ANSWER"),
    ("think harder, not less", "THINK HARDER, NOT LESS"),
    ("missing is the priority", "WHAT'S MISSING IS PRIORITY"),
    ("research is urgently needed", "RESEARCH IS URGENTLY NEEDED"),
    ("make people better thinkers", "CAN MAKE BETTER THINKERS"),
    ("design, deployment, and intention", "DESIGN. DEPLOYMENT. INTENTION."),
    ("smarter rather than lazier", "SMARTER, NOT LAZIER"),
    ("potential without intentionality", "POTENTIAL WITHOUT INTENTIONALITY"),
    ("seeing the pattern this time", "WE CAN SEE THE PATTERN"),
    ("whether we act on it", "DO WE ACT, OR WAIT?"),
]

found, missing = [], []
for anchor, emph in CAND:
    a = [w.lower().strip(".,?!\"'") for w in anchor.split()]
    hit = None
    for i in range(len(low) - len(a)):
        if low[i : i + len(a)] == a:
            hit = i
            break
    if hit is None:
        missing.append(anchor)
        continue
    found.append({"t": round(words[hit]["start"], 2), "words": emph})

found.sort(key=lambda x: x["t"])
# enforce a minimum gap so panels never crowd
MIN_GAP = 8.0
spaced = []
for b in found:
    if not spaced or b["t"] - spaced[-1]["t"] >= MIN_GAP:
        spaced.append(b)

json.dump(spaced, open("/home/penguinzyue/.claude/jobs/a5b95366/tmp/beats.json", "w"), indent=1)
print(f"matched {len(found)} / {len(CAND)}; after {MIN_GAP}s spacing: {len(spaced)}")
if missing:
    print("NOT FOUND:", missing)
print()
for b in spaced:
    m, s = int(b["t"]) // 60, int(b["t"]) % 60
    print(f"  {m}:{s:02d}  {b['words']}")
