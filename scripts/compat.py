#!/usr/bin/env python3
"""Executable compatibility matrix: runs bskyban (Go) and rsbsky (Rust) against a scripted
fake PDS with isolated state and compares stdout, stderr, exit status, request trace and
the files left behind.

  compat.py capture --go-bin PATH        record Go behaviour into tests/compat/golden/
  compat.py check   --bin PATH           compare a binary against the golden files
  compat.py diff    --go-bin G --bin R   run both live and compare (no golden files)

Cases live in compat_cases.py. A case may be `defer`red to a later phase: it is run and
reported but cannot fail the check; if it starts passing it is reported so it can be enabled.
"""
import argparse, json, os, pty, re, shutil, struct, subprocess, sys, tempfile, threading, time, fcntl, termios
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

HERE = os.path.dirname(os.path.abspath(__file__))
GOLDEN = os.path.join(HERE, "..", "tests", "compat", "golden")
sys.path.insert(0, HERE)
import compat_cases  # noqa: E402

AGO = re.compile(r"\{AGO:(\d+)([dhm])\}")


def expand(v):
    """Replace {AGO:400d}-style placeholders so ages are stable across runs."""
    if isinstance(v, str):
        def sub(m):
            n, u = int(m.group(1)), m.group(2)
            d = {"d": timedelta(days=n), "h": timedelta(hours=n), "m": timedelta(minutes=n)}[u]
            return (datetime.now(timezone.utc) - d).strftime("%Y-%m-%dT%H:%M:%S.000Z")
        return AGO.sub(sub, v)
    if isinstance(v, list):
        return [expand(x) for x in v]
    if isinstance(v, dict):
        return {k: expand(x) for k, x in v.items()}
    return v


DEFAULT_LOGIN = {"method": "POST", "path": "/xrpc/com.atproto.server.createSession",
                 "body": {"accessJwt": "TOKEN", "did": "did:me", "handle": "me.test"}}


class QuietServer(ThreadingHTTPServer):
    daemon_threads = True

    def handle_error(self, request, client_address):
        pass  # clients that abort mid-request are expected (the TUI cancels tasks on quit)


class FakePDS:
    def __init__(self, routes):
        self.routes = ([] if any(r.get("method") == "POST" and r["path"].endswith("createSession") for r in routes)
                       else [DEFAULT_LOGIN]) + list(routes)
        self.trace = []
        pds = self

        class H(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *a):
                pass

            def handle_any(self):
                u = urlparse(self.path)
                q = parse_qs(u.query, keep_blank_values=True)
                n = int(self.headers.get("Content-Length") or 0)
                raw = self.rfile.read(n) if n else b""
                try:
                    body = json.loads(raw) if raw else None
                except ValueError:
                    body = raw.decode("utf-8", "replace")
                pds.trace.append({"method": self.command, "path": u.path, "query": {k: q[k] for k in sorted(q)},
                                  "auth": self.headers.get("Authorization"),
                                  "ctype": self.headers.get("Content-Type"), "body": body})
                for r in pds.routes:
                    if r.get("method", "GET") != self.command or r["path"] != u.path:
                        continue
                    jb = body if isinstance(body, dict) else {}
                    if not all(jb.get(k) == v for k, v in r.get("jbody", {}).items()):
                        continue
                    if all((k not in q) if want is None else (q.get(k) == [want]) for k, want in r.get("q", {}).items()):
                        if r.get("delay"):
                            time.sleep(r["delay"])
                        status, payload = r.get("status", 200), expand(r.get("body", {}))
                        data = payload.encode() if isinstance(payload, str) else json.dumps(payload).encode()
                        self.send_response(status)
                        self.send_header("Content-Type", "text/html" if isinstance(payload, str) else "application/json")
                        self.send_header("Content-Length", str(len(data)))
                        self.end_headers()
                        self.wfile.write(data)
                        return
                data = b'{"error":"NoRoute","message":"no scripted response"}'
                self.send_response(404)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            do_GET = do_POST = handle_any

        self.srv = QuietServer(("127.0.0.1", 0), H)
        self.url = "http://127.0.0.1:%d" % self.srv.server_address[1]
        threading.Thread(target=lambda: self.srv.serve_forever(poll_interval=0.01), daemon=True).start()

    def close(self):
        self.srv.shutdown()
        self.srv.server_close()


def run_case(case, binary, go_syntax=False):
    tmp = tempfile.mkdtemp(prefix="compat-")
    pds = FakePDS(case.get("routes", []))
    try:
        os.makedirs(os.path.join(tmp, "home"))
        for rel, content in case.get("files", {}).items():
            path = os.path.join(tmp, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w") as f:
                f.write(content if isinstance(content, str) else json.dumps(content))
        env = {"PATH": "/usr/bin:/bin", "HOME": os.path.join(tmp, "home"), "BSKY_HANDLE": "me.test",
               "BSKY_APP_PASSWORD": "pw", "BSKY_PDS": pds.url, "BSKYBAN_STATE": os.path.join(tmp, "state.json"),
               "BSKYBAN_AUDIT": os.path.join(tmp, "audit.jsonl"), "TERM": "xterm",
               "TZ": "UTC"}  # `log` prints local times: pin the zone so goldens do not depend on the machine
        for k, v in case.get("env", {}).items():
            if v is None:
                env.pop(k, None)
            else:
                env[k] = v.replace("{TMP}", tmp).replace("{PDS}", pds.url)
        args = case["go_args"] if go_syntax and "go_args" in case else case["args"]
        args = [a.replace("{TMP}", tmp) for a in args]
        stdin = case.get("stdin")
        if case.get("tty"):
            master, slave = pty.openpty()
            cols = case["tty"]
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, cols, 0, 0))
            p = subprocess.Popen([binary] + args, env=env, cwd=tmp, stdin=subprocess.DEVNULL, stdout=slave,
                                 stderr=subprocess.PIPE)
            os.close(slave)
            chunks = []
            while True:
                try:
                    b = os.read(master, 65536)
                except OSError:
                    break
                if not b:
                    break
                chunks.append(b)
            os.close(master)
            err = p.stderr.read()
            p.wait(timeout=60)
            out = b"".join(chunks).replace(b"\r\n", b"\n")
        else:
            p = subprocess.run([binary] + args, env=env, cwd=tmp, input=(stdin or "").encode(),
                               capture_output=True, timeout=60, start_new_session=True)
            out, err = p.stdout, p.stderr
        files, modes = {}, {}
        for root, dirs, names in os.walk(tmp):
            for d in dirs:
                modes[os.path.relpath(os.path.join(root, d), tmp) + "/"] = oct(os.stat(os.path.join(root, d)).st_mode & 0o777)
            for n in names:
                full = os.path.join(root, n)
                rel = os.path.relpath(full, tmp)
                modes[rel] = oct(os.stat(full).st_mode & 0o777)
                with open(full, "rb") as f:
                    files[rel] = f.read().decode("utf-8", "replace").replace(tmp, "{TMP}")
        norm = lambda b: b.decode("utf-8", "replace").replace(pds.url, "{PDS}").replace(tmp, "{TMP}")
        return {"status": p.returncode, "stdout": norm(out), "stderr": norm(err), "trace": pds.trace,
                "files": files, "modes": modes}
    finally:
        pds.close()
        shutil.rmtree(tmp, ignore_errors=True)


TS_KEYS = {"ts", "at", "createdAt"}
RFC3339 = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$")


def fresh_ts(s):
    """True for a timestamp written by the program just now (as opposed to a fixture value)."""
    if not (isinstance(s, str) and RFC3339.match(s)):
        return False
    try:
        t = datetime.fromisoformat(s.replace("Z", "+00:00"))
    except ValueError:
        return False
    return abs((datetime.now(timezone.utc) - t).total_seconds()) < 120


def norm_json(v):
    """Replace just-written timestamps by <NOW>; everything else must match exactly."""
    if isinstance(v, dict):
        return {k: ("<NOW>" if k in TS_KEYS and fresh_ts(x) else norm_json(x)) for k, x in v.items()}
    if isinstance(v, list):
        return [norm_json(x) for x in v]
    return v


def norm_file(name, text):
    """state.json / audit.jsonl compare as parsed values (key order, indent, escaping are free)."""
    try:
        if name.endswith(".jsonl"):
            return [norm_json(json.loads(l)) for l in text.splitlines() if l.strip()]
        if name.endswith(".json"):
            return norm_json(json.loads(text))
    except ValueError:
        pass
    return text


AGO_TS = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.000Z")


def scrub(v):
    """Fixture timestamps ({AGO:...}) embed the current second; their *age* is what matters."""
    if isinstance(v, str):
        return AGO_TS.sub("<AGO>", v)
    if isinstance(v, list):
        return [scrub(x) for x in v]
    if isinstance(v, dict):
        return {k: scrub(x) for k, x in v.items()}
    return v


def freeze(r):
    """Golden files keep no wall-clock values: timestamps written 'just now' become <NOW>, so a
    golden stays valid however long after it was captured it is compared."""
    out = dict(r)
    out["trace"] = [dict(x, body=norm_json(x["body"])) for x in r["trace"]]
    files = {}
    for name, text in r["files"].items():
        v = norm_file(name, text)
        files[name] = ("\n".join(json.dumps(e) for e in v) + "\n") if name.endswith(".jsonl") and isinstance(v, list) else (json.dumps(v) if name.endswith(".json") and not isinstance(v, str) else text)
    out["files"] = files
    return out


def comparable(case, r):
    return scrub(_comparable(case, r))


def _comparable(case, r):
    """The parts of a result that must match, per the case's comparison mode."""
    mode = case.get("cmp", "exact")
    if mode == "status":  # message wording differs by design (clap vs Go flag package)
        return {"status": r["status"]}
    out = {"status": r["status"], "stdout": r["stdout"], "stderr": r["stderr"],
           "trace": [dict(x, body=norm_json(x["body"])) for x in r["trace"]],
           "files": {k: norm_file(k, v) for k, v in r["files"].items()}, "modes": r.get("modes", {})}
    if mode == "no_stderr":  # OS error wording differs (Go: "open x: is a directory", Rust: "Is a directory (os error 21)")
        out.pop("stderr")
        for name, v in out["files"].items():  # ...and so does the `error` recorded in the audit log
            if name.endswith(".jsonl"):
                out["files"][name] = [{k: x for k, x in e.items() if k != "error"} for e in v]
    if mode == "stderr_first_line":  # error text embeds Go/serde JSON wording
        out["stderr"] = r["stderr"].splitlines()[0].split(":")[0] if r["stderr"] else ""
    # stdout JSON Lines: compare parsed objects (key order/escaping are not part of the contract)
    if case.get("jsonl"):
        out["stdout"] = [json.loads(l) for l in r["stdout"].splitlines()]
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["capture", "check", "diff"])
    ap.add_argument("--go-bin")
    ap.add_argument("--bin")
    ap.add_argument("-k", help="only cases whose name contains this")
    a = ap.parse_args()
    for attr in ("bin", "go_bin"):
        if getattr(a, attr):
            setattr(a, attr, os.path.abspath(getattr(a, attr)))
    cases = [c for c in compat_cases.CASES if not a.k or a.k in c["name"]]
    names = [c["name"] for c in cases]
    assert len(set(names)) == len(names), "duplicate case names"
    os.makedirs(GOLDEN, exist_ok=True)
    failed, deferred, now_pass, ok = [], [], [], 0
    for c in cases:
        gpath = os.path.join(GOLDEN, c["name"] + ".json")
        if a.mode == "capture":
            r = run_case(c, a.go_bin, go_syntax=True)
            with open(gpath, "w") as f:
                json.dump(freeze(r), f, indent=1, sort_keys=True)
                f.write("\n")
            ok += 1
            continue
        if a.mode == "diff":
            want = run_case(c, a.go_bin, go_syntax=True)
        else:
            with open(gpath) as f:
                want = json.load(f)
        got = run_case(c, a.bin)
        same = comparable(c, got) == comparable(c, want)
        if c.get("defer"):
            (now_pass if same else deferred).append((c["name"], c["defer"]))
        elif same:
            ok += 1
        else:
            failed.append(c["name"])
            print("FAIL", c["name"])
            g, w = comparable(c, got), comparable(c, want)
            for k in w:
                if g.get(k) != w[k]:
                    print("  %s:\n    rust: %s\n    go:   %s" % (k, json.dumps(g.get(k))[:600], json.dumps(w[k])[:600]))
    print("%s: %d passed, %d failed, %d deferred" % (a.mode, ok, len(failed), len(deferred)))
    for n, ph in deferred:
        print("  deferred (%s): %s" % (ph, n))
    for n, ph in now_pass:
        print("  NOTE: deferred case now passes, enable it: %s (%s)" % (n, ph))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
