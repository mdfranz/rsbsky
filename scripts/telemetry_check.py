#!/usr/bin/env python3
"""Logfire telemetry checks against a local fake OTLP collector.

Verifies that with LOGFIRE_TOKEN set rsbsky exports spans, and, as importantly, that it
  * is completely inert without a token (no connections, identical behaviour),
  * never leaks credentials, handles, DIDs or post text,
  * never changes stdout / stderr / exit status of any command,
  * does not hang or print anything when the collector is unreachable,
  * does not draw on the terminal while the TUI is running.

  telemetry_check.py --bin target/debug/rsbsky
"""
import argparse, gzip, os, pty, re, select, shutil, struct, subprocess, sys, tempfile, threading, time, fcntl, termios
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from compat import FakePDS  # noqa: E402
from compat_cases import entry, tl, prof, pr, create, delete, BLOCK, P_NEW, R1, A, B, TIMELINE, STATE  # noqa: E402
from tui_check import timeline as tui_timeline, detail_routes  # noqa: E402

FAILS = []
TOKEN = "pylf_v1_us_TELEMETRYTESTSECRET0123456789"
# Everything that identifies the user, their account, or the people they act on.
SECRETS = [b"pw", b"Bearer", b"TOKEN", b"JWT", b"me.test", b"did:me", b"did:a", b"did:newbie", b"newbie",
           b"a.test", b"first post by A", b"Alice", b"app.bsky.graph.block/", b"password", b"identifier",
           # the PDS host, and the dependencies' connection-level debug events that mention it
           b"127.0.0.1", b"localhost", b"bsky.social", b"connecting to", b"idle connection", b"Http::connect",
           b"CA root certificates", b"hyper", b"rustls"]


def check(name, ok, detail=""):
    print(("ok   " if ok else "FAIL ") + name + ("" if ok else "  -> " + str(detail)[:400]))
    if not ok:
        FAILS.append(name)


class Collector:
    """Accepts OTLP/HTTP exports and keeps what it received."""

    def __init__(self):
        self.requests = []
        col = self

        class H(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *a):
                pass

            def do_POST(self):
                n = int(self.headers.get("Content-Length") or 0)
                body = self.rfile.read(n)
                if self.headers.get("Content-Encoding") == "gzip":
                    try:
                        body = gzip.decompress(body)
                    except OSError:
                        pass
                col.requests.append({"path": self.path, "auth": self.headers.get("Authorization"), "body": body})
                self.send_response(200)
                self.send_header("Content-Type", "application/x-protobuf")
                self.send_header("Content-Length", "0")
                self.end_headers()

        class S(ThreadingHTTPServer):
            daemon_threads = True

            def handle_error(self, *a):
                pass

        self.srv = S(("127.0.0.1", 0), H)
        self.url = "http://127.0.0.1:%d" % self.srv.server_address[1]
        threading.Thread(target=lambda: self.srv.serve_forever(poll_interval=0.02), daemon=True).start()

    def traces(self):
        return b"".join(r["body"] for r in self.requests if r["path"].endswith("/v1/traces"))

    def everything(self):
        return b"".join(r["body"] for r in self.requests)

    def close(self):
        self.srv.shutdown()
        self.srv.server_close()


def run(binp, args, pds, tmp, extra_env=None, stdin="", timeout=60):
    env = {"PATH": "/usr/bin:/bin", "HOME": tmp, "TZ": "UTC", "BSKY_HANDLE": "me.test", "BSKY_APP_PASSWORD": "pw",
           "BSKY_PDS": pds.url, "BSKYBAN_STATE": tmp + "/state.json", "BSKYBAN_AUDIT": tmp + "/audit.jsonl"}
    env.update(extra_env or {})
    t0 = time.time()
    p = subprocess.run([binp] + args, env=env, cwd=tmp, input=stdin.encode(), capture_output=True, timeout=timeout,
                       start_new_session=True)
    return p.returncode, p.stdout.decode(), p.stderr.decode(), time.time() - t0


def fresh_tmp():
    d = tempfile.mkdtemp(prefix="telemetry-")
    return d


def normalize(s, tmp):
    return s.replace(tmp, "{TMP}")


def span_names_in(payload):
    return {m.decode() for m in re.findall(rb"(xrpc|command|load timeline page|refresh check|moderation action|feed session|"
                                           rb"fetch profile|fetch recent posts|load follows page)", payload)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--disabled", action="store_true", help="binary built with --no-default-features: must never export")
    args = ap.parse_args()
    binp = os.path.abspath(args.bin)
    routes = list(TIMELINE) + tui_timeline() + detail_routes() + [P_NEW, create(BLOCK, "at://did:me/app.bsky.graph.block/new1"), delete(BLOCK)]
    pds = FakePDS(routes)
    col = Collector()
    tel = {"LOGFIRE_TOKEN": TOKEN, "LOGFIRE_BASE_URL": col.url}
    tmps = []

    def tmp():
        d = fresh_tmp()
        tmps.append(d)
        return d

    try:
        # ---- 1. without a token nothing is sent and nothing changes
        t = tmp()
        base = run(binp, ["feed", "--list", "--pages", "2"], pds, t)
        check("baseline `feed --list` works", base[0] == 0 and "a.test" in base[1], base)
        for env, label in [({}, "no token"), ({"LOGFIRE_TOKEN": ""}, "empty token"), ({"LOGFIRE_TOKEN": "   "}, "blank token"),
                           (dict(tel, LOGFIRE_SEND_TO_LOGFIRE="no"), "LOGFIRE_SEND_TO_LOGFIRE=no")]:
            e = dict(env, LOGFIRE_BASE_URL=col.url)
            if label == "no token":
                e = {"LOGFIRE_BASE_URL": col.url}
            r = run(binp, ["feed", "--list", "--pages", "2"], pds, tmp(), e)
            check("%s: same output and exit status" % label, r[:3] == base[:3], (r, base))
        time.sleep(0.5)
        check("nothing was sent without a usable token", not col.requests, [r["path"] for r in col.requests])

        if args.disabled:
            r = run(binp, ["feed", "--list", "--pages", "2"], pds, tmp(), tel)
            check("built without the logfire feature: output unchanged even with a token", r[:3] == base[:3], (r[:3], base[:3]))
            time.sleep(1.0)
            check("...and nothing is ever sent", not col.requests, [x["path"] for x in col.requests])
            screen = tui_run(binp, pds, tel, tmp())
            check("...the TUI works too", screen["code"] == 0)
            time.sleep(0.5)
            check("...and sends nothing", not col.requests)
            return finish(pds, col, tmps)

        # ---- 2. with a token: spans are exported, authenticated, and output is unchanged
        r = run(binp, ["feed", "--list", "--pages", "2"], pds, tmp(), tel)
        check("with LOGFIRE_TOKEN stdout, stderr and exit status are unchanged", r[:3] == base[:3], (r, base))
        time.sleep(0.5)
        check("spans reached the collector", col.traces() != b"", [x["path"] for x in col.requests])
        check("exports go to /v1/traces", any(x["path"].endswith("/v1/traces") for x in col.requests))
        check("the write token authenticates the export", all(x["auth"] == "Bearer " + TOKEN for x in col.requests if x["path"].endswith("/v1/traces")), [x["auth"] for x in col.requests])
        names = span_names_in(col.traces())
        check("command and request spans are present", {"command", "xrpc"} <= names, names)
        tr = col.traces()
        check("service.name defaults to rsbsky", b"service.name" in tr and b"rsbsky" in tr)
        check("service.version is the crate version", b"service.version" in tr and b"0.1.0" in tr)
        check("operation names are recorded", b"app.bsky.feed.getTimeline" in tr and b"com.atproto.server.createSession" in tr)
        check("the command is recorded (not its arguments)", b"feed" in tr and b"--pages" not in tr)
        check("the HTTP status and outcome are recorded", b"http.response.status_code" in tr and b"outcome" in tr)

        # ---- 3. privacy: nothing identifying leaves the machine
        leaked = [s.decode() for s in SECRETS if s in col.everything()]
        check("no credentials, handles, DIDs or post text in any export", not leaked, leaked)
        check("the token itself is not in the payload", TOKEN.encode() not in col.everything())

        # ---- 4. mutations: actions are traced without identities
        col.requests.clear()
        t = tmp()
        plain = run(binp, ["block", "newbie"], pds, t)
        col.requests.clear()
        t2 = tmp()
        traced = run(binp, ["block", "newbie"], pds, t2, tel)
        check("`block` behaves identically with telemetry", normalize(traced[1], t2) == normalize(plain[1], t) and traced[0] == plain[0] == 0 and traced[2] == plain[2], (plain, traced))
        time.sleep(0.5)
        check("`block` is exported as a command span", b"block" in col.traces() and b"createRecord" in col.traces(), span_names_in(col.traces()))
        leaked = [s.decode() for s in SECRETS if s in col.everything()]
        check("...with no identities (not the account blocked, not its DID)", not leaked, leaked)
        failing = run(binp, ["block", "ghost"], pds, tmp(), tel)
        check("a failing command keeps its exit status and message", failing[0] == 1 and "ghost" in failing[2] and "ghost" not in col.everything().decode("latin1"), failing)

        # ---- 5. a dead collector must not change behaviour or slow the CLI down
        dead = {"LOGFIRE_TOKEN": TOKEN, "LOGFIRE_BASE_URL": "http://127.0.0.1:1"}
        r = run(binp, ["feed", "--list", "--pages", "2"], pds, tmp(), dead)
        check("unreachable collector: stdout, stderr and exit status unchanged", r[:3] == base[:3], (r[:3], base[:3]))
        check("...and exit is prompt (under 8s)", r[3] < 8, r[3])
        r = run(binp, ["log"], pds, tmp(), dead)
        check("unreachable collector: credential-free commands still work", r[0] == 0, r)
        r = run(binp, ["themes"], pds, tmp(), dead, timeout=30)
        check("...including `themes`", r[0] == 0 and "dracula" in r[1], r[:2])

        # ---- 6. bad configuration warns once and carries on
        r = run(binp, ["feed", "--list", "--pages", "2"], pds, tmp(), dict(tel, LOGFIRE_SEND_TO_LOGFIRE="sometimes"))
        check("an invalid LOGFIRE_SEND_TO_LOGFIRE warns on stderr but the command still works", r[0] == 0 and r[1] == base[1] and r[2].count("warning: logfire disabled") == 1, r)

        # ---- 7. the TUI: no drawing on the screen, spans for background work, prompt exit
        col.requests.clear()
        screen = tui_run(binp, pds, tel, tmp())
        check("TUI exits 0 with telemetry on", screen["code"] == 0, screen["code"])
        check("TUI output has no logfire / opentelemetry text", not re.search(rb"logfire|opentelemetry|otel|WARN|INFO ", screen["bytes"], re.I), screen["bytes"][:300])
        time.sleep(0.5)
        names = span_names_in(col.traces())
        check("TUI exports session and background-work spans", {"feed session", "load timeline page", "xrpc", "refresh check", "moderation action", "fetch profile", "fetch recent posts"} <= names, names)
        leaked = [s.decode() for s in SECRETS if s in col.everything()]
        check("TUI exports contain no identities", not leaked, leaked)
        dead_screen = tui_run(binp, pds, dead, tmp())
        check("TUI with an unreachable collector still quits promptly", dead_screen["code"] == 0 and dead_screen["elapsed"] < 8, (dead_screen["code"], dead_screen["elapsed"]))
        check("...and prints nothing extra on screen", not re.search(rb"logfire|opentelemetry|\botel\b", dead_screen["bytes"], re.I), dead_screen["bytes"][:300])
    finally:
        pass
    finish(pds, col, tmps)


def finish(pds, col, tmps):
    pds.close()
    col.close()
    for d in tmps:
        shutil.rmtree(d, ignore_errors=True)
    print("\n%s" % ("all telemetry checks passed" if not FAILS else "FAILED: " + ", ".join(FAILS)))
    sys.exit(1 if FAILS else 0)


def tui_run(binp, pds, extra_env, tmp):
    """Runs the TUI on a pty for a moment, presses Down then q, and returns the raw output."""
    env = {"PATH": "/usr/bin:/bin", "HOME": tmp, "TZ": "UTC", "TERM": "xterm-256color", "BSKY_HANDLE": "me.test",
           "BSKY_APP_PASSWORD": "pw", "BSKY_PDS": pds.url, "BSKYBAN_STATE": tmp + "/state.json",
           "BSKYBAN_AUDIT": tmp + "/audit.jsonl", "BROWSER": "/bin/true"}
    env.update(extra_env)
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
    p = subprocess.Popen([binp, "--refresh", "10s"], env=env, stdin=slave, stdout=slave, stderr=subprocess.PIPE, close_fds=True, preexec_fn=os.setsid)
    os.close(slave)
    buf = b""

    def read(sec):
        nonlocal buf
        end = time.time() + sec
        while time.time() < end:
            r, _, _ = select.select([master], [], [], 0.05)
            if r:
                try:
                    chunk = os.read(master, 65536)
                except OSError:
                    return
                if not chunk:
                    return
                buf += chunk

    read(2.0)
    os.write(master, b"\x1b[B")  # Down
    read(0.4)
    os.write(master, b"\x1b[C")  # Right: detail view, prefetches profiles
    read(1.0)
    os.write(master, b"\x1b")  # Esc
    read(0.3)
    os.write(master, b"k")  # keep: a local decision, traced as a moderation action
    read(0.6)
    os.write(master, b"R")  # manual refresh
    read(1.0)
    t0 = time.time()
    os.write(master, b"q")
    try:
        code = p.wait(timeout=15)
    except subprocess.TimeoutExpired:
        p.kill()
        code = -9
    elapsed = time.time() - t0
    read(0.3)
    err = p.stderr.read()
    os.close(master)
    return {"code": code, "bytes": buf + err, "elapsed": elapsed}


if __name__ == "__main__":
    main()
