#!/usr/bin/env python3
"""Replay seQ's trace CSV (session,turn,new,out,think,forced) against an
OpenAI-compatible endpoint (a vLLM instance or a P/D proxy) with token-id
prompts of exactly the trace's lengths.

Session i is sent at i * spacing seconds; turn k+1 is sent `think` seconds
after turn k completes, and its prompt is turn k's prompt + turn k's output
+ fresh tokens up to the trace's `new` (a forced turn gets a fresh nonce at
its head, so nothing matches). Every request records send time, first-token
time, end time, the server's cached_tokens and the tokens generated.

    python3 pd_replay.py --csv short_base.csv --base http://127.0.0.1:8000 \
        --model Qwen/Qwen3-8B --spacing 3.0 --sessions 96 --out rounds.jsonl
"""
import argparse, asyncio, csv, json, random, time
import aiohttp

VOCAB_LO, VOCAB_HI = 1000, 100000   # plain token ids, no specials


def load(path, n):
    sess = {}
    for r in csv.DictReader(open(path)):
        s = int(r["session"])
        sess.setdefault(s, []).append(r)
    out = []
    for s in sorted(sess)[:n]:
        turns = sorted(sess[s], key=lambda r: int(r["turn"]))
        out.append([(int(t["new"]), int(t["out"]), float(t["think"]), int(t["forced"])) for t in turns])
    return out


async def one_request(http, base, model, prompt, out_tokens, rec):
    body = {"model": model, "prompt": prompt, "max_tokens": max(1, out_tokens),
            "temperature": 0.0, "ignore_eos": True, "stream": True,
            "stream_options": {"include_usage": True}, "return_token_ids": True}
    rec["sent"] = time.time()
    first = None
    gen = []
    usage = None
    async with http.post(f"{base}/v1/completions", json=body) as resp:
        rec["status"] = resp.status
        if resp.status != 200:
            rec["error"] = (await resp.text())[:300]
            return gen
        async for raw in resp.content:
            line = raw.decode().strip()
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
                break
            j = json.loads(data)
            if j.get("choices"):
                c = j["choices"][0]
                ids = c.get("token_ids") or []
                if (c.get("text") or ids) and first is None:
                    first = time.time()
                gen.extend(ids)
            if j.get("usage"):
                usage = j["usage"]
    rec["first"] = first
    rec["done"] = time.time()
    rec["generated"] = len(gen)
    if usage:
        rec["prompt_tokens"] = usage.get("prompt_tokens")
        det = usage.get("prompt_tokens_details") or {}
        rec["cached_tokens"] = det.get("cached_tokens", 0)
    return gen


async def session(i, turns, args, http, out, t0):
    await asyncio.sleep(i * args.spacing)
    rng = random.Random(1000 + i)
    prompt = []
    for k, (new, o, think, forced) in enumerate(turns):
        if forced:
            prompt = [rng.randint(VOCAB_LO, VOCAB_HI) for _ in range(new)]
        else:
            need = new - len(prompt)
            if need < 1:
                prompt = prompt[: max(1, new - 1)]
                need = new - len(prompt)
            prompt = prompt + [rng.randint(VOCAB_LO, VOCAB_HI) for _ in range(need)]
        rec = {"session": i, "turn": k + 1, "new": new, "out": o, "forced": forced, "prompt_len": len(prompt)}
        gen = await one_request(http, args.base, args.model, prompt, o, rec)
        for key in ("sent", "first", "done"):
            if rec.get(key) is not None:
                rec[key] -= t0
        out.write(json.dumps(rec) + "\n"); out.flush()
        if "error" in rec:
            return
        # the generated tokens are part of the next prompt (vLLM returned ids; else pad)
        prompt = prompt + (gen if len(gen) == o else [rng.randint(VOCAB_LO, VOCAB_HI) for _ in range(o)])
        if k + 1 < len(turns):
            await asyncio.sleep(think)


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--csv", required=True)
    ap.add_argument("--base", required=True)
    ap.add_argument("--model", required=True)
    ap.add_argument("--spacing", type=float, default=3.0)
    ap.add_argument("--sessions", type=int, default=96)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    sessions = load(args.csv, args.sessions)
    t0 = time.time()
    conn = aiohttp.TCPConnector(limit=0)
    timeout = aiohttp.ClientTimeout(total=None, sock_read=None)
    with open(args.out, "w") as out:
        async with aiohttp.ClientSession(connector=conn, timeout=timeout) as http:
            await asyncio.gather(*(session(i, t, args, http, out, t0) for i, t in enumerate(sessions)))
    print("done", len(sessions), "sessions", round(time.time() - t0, 1), "s")


if __name__ == "__main__":
    asyncio.run(main())
