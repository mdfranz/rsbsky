#!/usr/bin/env python3
"""Sends a few real spans to Logfire using the LOGFIRE_TOKEN from the environment.

Bluesky is a local fake PDS (no real account is touched); only telemetry leaves the machine.
The data is tagged LOGFIRE_ENVIRONMENT=rsbsky-dev-check. The token is passed through the
environment and never printed.

  telemetry_live.py --bin target/debug/rsbsky
"""
import argparse, os, shutil, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from compat import FakePDS  # noqa: E402
from compat_cases import TIMELINE, P_NEW, create, delete, BLOCK  # noqa: E402
from telemetry_check import run, tui_run  # noqa: E402
from tui_check import timeline as tui_timeline, detail_routes  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("--bin", required=True)
binp = os.path.abspath(ap.parse_args().bin)
token = os.environ.get("LOGFIRE_TOKEN", "")
if not token.strip():
    sys.exit("LOGFIRE_TOKEN is not set")

pds = FakePDS(list(TIMELINE) + tui_timeline() + detail_routes() + [P_NEW, create(BLOCK, "at://did:me/app.bsky.graph.block/new1"), delete(BLOCK)])
env = {"LOGFIRE_TOKEN": token, "LOGFIRE_ENVIRONMENT": "rsbsky-dev-check"}
tmps = [tempfile.mkdtemp(prefix="telemetry-live-") for _ in range(4)]
try:
    code, out, err, secs = run(binp, ["feed", "--list", "--pages", "2"], pds, tmps[0], env)
    print("feed --list        exit %d, %d lines, %.1fs, stderr %r" % (code, len(out.splitlines()), secs, err))
    code, out, err, secs = run(binp, ["block", "newbie"], pds, tmps[1], env)
    print("block newbie       exit %d, %.1fs, stderr %r" % (code, secs, err))
    code, out, err, secs = run(binp, ["block", "ghost"], pds, tmps[2], env)
    print("block ghost (fail) exit %d, %.1fs" % (code, secs))
    r = tui_run(binp, pds, env, tmps[3])
    print("TUI session        exit %d, quit took %.1fs" % (r["code"], r["elapsed"]))
finally:
    pds.close()
    for d in tmps:
        shutil.rmtree(d, ignore_errors=True)
print("done: look for service.name=rsbsky, environment=rsbsky-dev-check in Logfire")
