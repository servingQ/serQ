#!/usr/bin/env python3
"""Fit the testbed's constants from the lone-request probe: transfer time
x0 + n/Bw (least squares on nixl_xfer_time per request) and TTFT of a lone
request h + cP + aP*n + bP*n^2/2 + x0 + n/Bw + (one decode step)."""
import json, sys
import numpy as np
rs = [json.loads(l) for l in open(sys.argv[1])]
rs = [r for r in rs if r["xfer_count"] >= 1 and r["ttft"]]
n = np.array([r["n"] for r in rs], float); xf = np.array([r["xfer_sum"] / r["xfer_count"] for r in rs]); tt = np.array([r["ttft"] for r in rs])
A = np.c_[np.ones_like(n), n]; (x0, inv_bw), *_ = np.linalg.lstsq(A, xf, rcond=None)
print(f"transfer: x0 = {x0*1e3:.1f} ms, Bw = {1/inv_bw:.0f} tokens/s  (residual rms {np.sqrt(np.mean((A@[x0,inv_bw]-xf)**2))*1e3:.1f} ms)")
# TTFT minus transfer = h + cP*steps + aP*n + bP*n^2/2 ; steps = ceil(n/8192) prefill steps + 1 decode step
steps = np.ceil(n / 8192) + 1
B = np.c_[np.ones_like(n), steps, n, n**2 / 2]
(h, cstep, aP, bP), *_ = np.linalg.lstsq(B, tt - xf, rcond=None)
print(f"ttft - xfer: h = {h*1e3:.1f} ms, per step = {cstep*1e3:.1f} ms, aP = {aP*1e6:.1f} us/token, bP = {bP*1e9:.2f} ns/token^2 (rms {np.sqrt(np.mean((B@[h,cstep,aP,bP]-(tt-xf))**2))*1e3:.1f} ms)")
for r in rs: print(r["n"], "ttft {:.3f} xfer {:.3f}".format(r["ttft"], r["xfer_sum"]/r["xfer_count"]))
