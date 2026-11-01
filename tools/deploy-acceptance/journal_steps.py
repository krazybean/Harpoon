#!/usr/bin/env python3
"""Read-only: print one deployment's step timings from a COPY of the deploy journal (SQLite).

usage: journal_steps.py JOURNAL_COPY.sqlite3 DEPLOYMENT_ID

The latest `snap` record of the deployment holds six step groups after the release field, in the
engine's order (source, build, preliminary_check, cutover, final_check, recovery), four fields
each: status, started_ms, finished_ms, note. Output is JSON: per step status, started_ms,
finished_ms and duration_ms. Opens the file read-only; never use the live journal file.
"""
import json
import sqlite3
import sys

STEPS = ["source", "build", "preliminary_check", "cutover", "final_check", "recovery"]
STATUS = {"pending", "running", "succeeded", "failed", "skipped", "cancelled", "interrupted"}


def num(x):
    return x == "-" or x.isdigit()


def groups(f):
    for i in range(len(f) - 24 + 1):
        if all(
            f[i + 4 * k] in STATUS and num(f[i + 4 * k + 1]) and num(f[i + 4 * k + 2])
            for k in range(6)
        ):
            return [f[i + 4 * k : i + 4 * k + 3] for k in range(6)]
    return None


def main():
    path, dep = sys.argv[1], sys.argv[2]
    db = sqlite3.connect("file:%s?mode=ro" % path, uri=True)
    last = None
    for (body,) in db.execute("select body from records where kind='snapshot' order by seq"):
        f = body.split("\t")
        if dep in f[:8]:
            last = f
    if last is None:
        sys.exit("deployment %s not found" % dep)
    g = groups(last)
    if g is None:
        sys.exit("no step groups found in the snapshot")
    out = {}
    for name, (st, a, b) in zip(STEPS, g):
        a = None if a == "-" else int(a)
        b = None if b == "-" else int(b)
        out[name] = {
            "status": st,
            "started_ms": a,
            "finished_ms": b,
            "duration_ms": (b - a) if a is not None and b is not None else None,
        }
    print(json.dumps(out, indent=1))


main()
