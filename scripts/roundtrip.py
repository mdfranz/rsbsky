#!/usr/bin/env python3
"""Sequential Go -> Rust -> Go round trips over shared state.json / audit.jsonl.

Each step runs one of the two binaries against the same fake PDS and the same files, so
anything one tool writes must be read, preserved and updated correctly by the other.

  roundtrip.py --go-bin PATH --bin PATH
"""
import argparse, json, os, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from compat import FakePDS  # noqa: E402
from compat_cases import pr, create, delete, BLOCK, FOLLOW, prof, FOLLOWING_VIEW, BLOCKED_VIEW  # noqa: E402

FAILS = []


def check(name, ok, detail=""):
    print(("ok   " if ok else "FAIL ") + name + ("" if ok else "  -> " + str(detail)[:500]))
    if not ok:
        FAILS.append(name)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--go-bin", required=True)
    ap.add_argument("--bin", required=True)
    a = ap.parse_args()
    tools = {"go": os.path.abspath(a.go_bin), "rs": os.path.abspath(a.bin)}

    tmp = tempfile.mkdtemp(prefix="roundtrip-")
    routes = [
        pr("newbie"), pr("friend", viewer=FOLLOWING_VIEW), pr("bad", viewer=BLOCKED_VIEW), pr("kay"), pr("zed"), pr("dee"),
        create(BLOCK, "at://did:me/app.bsky.graph.block/new1"), create(FOLLOW, "at://did:me/app.bsky.graph.follow/new1"),
        delete(BLOCK), delete(FOLLOW),
        {"path": "/xrpc/app.bsky.graph.getFollowers", "body": {"followers": [
            prof("did:newbie", "newbie.test"), prof("did:kay", "kay.test"), prof("did:zed", "zed.test"), prof("did:dee", "dee.test")]}},
        {"path": "/xrpc/app.bsky.actor.getProfiles", "body": {"profiles": [
            {"did": "did:newbie", "handle": "newbie.test"}, {"did": "did:kay", "handle": "kay.test"},
            {"did": "did:zed", "handle": "zed.test"}, {"did": "did:dee", "handle": "dee.test"}]}},
        {"path": "/xrpc/app.bsky.feed.getAuthorFeed", "body": {"feed": []}},
    ]
    pds = FakePDS(routes)
    state, audit = os.path.join(tmp, "state.json"), os.path.join(tmp, "audit.jsonl")
    env = {"PATH": "/usr/bin:/bin", "HOME": tmp, "BSKY_HANDLE": "me.test", "BSKY_APP_PASSWORD": "pw", "BSKY_PDS": pds.url,
           "BSKYBAN_STATE": state, "BSKYBAN_AUDIT": audit, "TZ": "UTC"}

    def run(tool, *args, expect=0):
        p = subprocess.run([tools[tool], *args], env=env, capture_output=True, text=True, cwd=tmp, start_new_session=True)
        if p.returncode != expect:
            check("%s %s exits %d" % (tool, " ".join(args), expect), False, (p.returncode, p.stdout, p.stderr))
        return p

    def entries():
        return json.load(open(state))["entries"] if os.path.exists(state) else {}

    def events():
        return [json.loads(l) for l in open(audit).read().splitlines()] if os.path.exists(audit) else []

    def listing(tool, *extra):
        out = run(tool, "list", "--all", "--json", *extra).stdout
        return {json.loads(l)["did"]: json.loads(l)["status"] for l in out.splitlines()}

    try:
        # 1. Go writes, Rust reads and writes
        run("go", "block", "newbie")
        go_entry = dict(entries()["did:newbie"])
        check("Go block recorded the decision", go_entry["decision"] == "blocked")
        check("Rust `list` reads Go's state", listing("rs")["did:newbie"] == "blocked")
        run("rs", "keep", "kay")
        check("Rust keeps Go's entry byte-for-value intact when rewriting the file", entries()["did:newbie"] == go_entry, entries())
        check("Rust added its own entry", entries()["did:kay"]["decision"] == "kept")

        # 2. Rust writes, Go reads and updates
        rs_entry = dict(entries()["did:kay"])
        check("Go `list` reads Rust's state", listing("go") == {"did:newbie": "blocked", "did:kay": "kept", "did:zed": "new", "did:dee": "new"}, listing("go"))
        run("go", "keep", "zed")
        check("Go preserves Rust's entry (handle, decision, timestamp)", entries()["did:kay"] == rs_entry, entries())
        check("three decisions from two tools", sorted(entries()) == ["did:kay", "did:newbie", "did:zed"], entries())

        # 3. follow / unfollow across tools: the decision written by one is cleared by the other
        run("rs", "follow", "dee")
        check("Rust follow recorded", entries()["did:dee"]["decision"] == "followed")
        pds.routes.insert(0, pr("dee", viewer={"following": "at://did:me/app.bsky.graph.follow/new1"}))
        run("go", "unfollow", "dee")
        check("Go unfollow clears the decision Rust wrote", "did:dee" not in entries(), entries())
        run("rs", "unblock", "newbie")
        check("Rust unblock clears the decision Go wrote", "did:newbie" not in entries(), entries())

        # 4. baseline by one tool, read by the other with --backlog
        run("rs", "baseline")
        after = entries()
        check("Rust baseline marked undecided followers as skipped and left decided ones", after["did:newbie"]["decision"] == "skipped" and after["did:dee"]["decision"] == "skipped" and after["did:kay"]["decision"] == "kept" and after["did:zed"]["decision"] == "kept", after)
        out = run("go", "list", "--json").stdout
        check("Go's default list hides skipped followers", out.strip() == "", out)
        out = run("go", "list", "--backlog", "--json").stdout
        check("Go's `--backlog` shows them", sorted(json.loads(l)["did"] for l in out.splitlines()) == ["did:dee", "did:newbie"], out)
        out = run("rs", "list", "--backlog", "--json").stdout
        check("Rust's `--backlog` agrees", sorted(json.loads(l)["did"] for l in out.splitlines()) == ["did:dee", "did:newbie"], out)
        run("go", "baseline")
        check("Go baseline leaves Rust's skipped entries alone (they are decided)", entries()["did:newbie"]["decision"] == "skipped")

        # 5. one audit log, two writers, two readers
        ev = events()
        check("both tools' events are in one log", {e["action"] for e in ev} >= {"block", "keep", "follow", "unfollow", "unblock", "baseline"}, [e["action"] for e in ev])
        check("every event has ts, action, source cli", all(e.get("ts") and e.get("action") and e.get("source") == "cli" for e in ev))
        rs_log = run("rs", "log", "--json").stdout
        go_log = run("go", "log", "--json").stdout
        same = [json.loads(l) for l in rs_log.splitlines()] == [json.loads(l) for l in go_log.splitlines()]
        check("Rust and Go read the same audit log identically (--json)", same and len(rs_log.splitlines()) == len(ev), (rs_log[:300], go_log[:300]))
        rs_txt = run("rs", "log").stdout
        go_txt = run("go", "log").stdout
        check("...and render the same text", rs_txt == go_txt and rs_txt.count("\n") == len(ev), (rs_txt[:300], go_txt[:300]))
        for flt in (["--action", "keep"], ["--who", "zed"], ["-n", "3"], ["--since", "1h"]):
            check("log %s agrees" % " ".join(flt), run("rs", "log", *flt).stdout == run("go", "log", *flt).stdout)

        # 6. files written by Rust keep Go's permissions
        mode = lambda p: oct(os.stat(p).st_mode & 0o777)
        check("state.json is 0600 and audit.jsonl is 0600", mode(state) == "0o600" and mode(audit) == "0o600", (mode(state), mode(audit)))
    finally:
        pds.close()
        shutil.rmtree(tmp, ignore_errors=True)

    print("\n%s" % ("all round trips passed" if not FAILS else "FAILED: " + ", ".join(FAILS)))
    sys.exit(1 if FAILS else 0)


if __name__ == "__main__":
    main()
