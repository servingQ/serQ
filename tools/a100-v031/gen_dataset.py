#!/usr/bin/env python3
"""The workload examples/workloads/single_turn.sq run against
examples/multi-turn/vllm.sq (examples/single-turn/vllm_single_turn.sq at
collection time) as a vllm bench custom dataset: prompt
n ~ ceil(uniform(1000, 3000)) tokens, output o = floor(exp(mean 200)) + 1
tokens. The (n, o) draws depend only on the seed, so both models get the same
lengths.

The prompt is random words of the model's own vocabulary, each a token that
decodes to " <letters>" and re-encodes to itself, so a sequence of them
tokenizes back to the same length. The length is corrected at most 8 times;
`tokens` records what the prompt encodes to (equal to `n` unless noted).

usage: gen_dataset.py MODEL N SEED OUT.jsonl
"""
import json
import math
import random
import sys

from transformers import AutoTokenizer

model, N, seed, out = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
tok = AutoTokenizer.from_pretrained(model)
words = []
for i in range(tok.vocab_size):
    s = tok.decode([i])
    if len(s) >= 3 and s[0] == " " and s[1:].isascii() and s[1:].isalpha() and s[1:].islower():
        if tok(s, add_special_tokens=False).input_ids == [i]:
            words.append(s)
assert len(words) > 1000, len(words)


def length(p):
    return len(tok(p, add_special_tokens=False).input_ids)


lens = random.Random(seed)
text = random.Random(seed + 1_000_003)
off = 0
with open(out, "w") as f:
    for i in range(N):
        n = math.ceil(lens.uniform(1000, 3000))
        o = math.floor(lens.expovariate(1 / 200)) + 1
        w = [text.choice(words) for _ in range(n)]
        p = "".join(w)
        m = length(p)
        for _ in range(8):
            if m == n:
                break
            if m > n:
                w = w[: max(1, len(w) - (m - n))]
            else:
                w += [text.choice(words) for _ in range(n - m)]
            p = "".join(w)
            m = length(p)
        off += m != n
        f.write(json.dumps({"prompt": p, "output_tokens": o, "n": n, "tokens": m, "i": i}) + "\n")
print(f"{N} requests -> {out}; {len(words)} words; {off} prompts not exactly n")
