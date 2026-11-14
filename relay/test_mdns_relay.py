"""Tests of the mDNS alarm relay (dcontroller #88). Run: python3 -m unittest discover -s relay"""
import json
import os
import sys
import tempfile
import textwrap
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mdns_relay as r  # noqa: E402


def entry(at, source="10.0.0.7", ok=False, devices="mixer, this server", before="under 50/s", after="80.5/s"):
    return {"at": at, "kind": "status", "what": "mdns", "rx_label": source, "rx_device": devices,
            "ok": ok, "name_before": before, "name_after": after, "user": "mDNS meter"}


class Pick(unittest.TestCase):
    def test_only_mdns_alarms_after_the_cursor_oldest_first(self):
        es = [entry(30), {"at": 40, "kind": "status", "what": "rx"},
              {"at": 41, "kind": "rename", "what": "mdns"}, entry(20), entry(10)]
        got = r.pick(es, {"at": 15, "sent": []})
        self.assertEqual([e["at"] for e in got], [20, 30])

    def test_an_entry_in_the_cursor_s_second_goes_once(self):
        a, b = entry(50, "10.0.0.7"), entry(50, "10.0.0.9")
        c = r.advance({"at": 40, "sent": []}, a)
        self.assertEqual(r.pick([a, b], c), [b])
        c = r.advance(c, b)
        self.assertEqual(r.pick([a, b], c), [])
        self.assertEqual(r.advance(c, entry(60))["sent"], [r.key(entry(60))])


class Message(unittest.TestCase):
    def test_a_start_names_source_rate_and_devices(self):
        m = r.message(entry(1_700_000_000))
        self.assertIn("mDNS alarm (dcontroller", m)
        self.assertIn("10.0.0.7 is over the cap of 50/s since ", m)
        self.assertIn("80.5/s averaged over a whole minute", m)
        self.assertIn("Dante devices at that address: mixer, this server", m)

    def test_an_end_says_now_and_during(self):
        m = r.message(entry(1, ok=True, before="80.5/s", after="quiet"))
        self.assertIn("mDNS alarm ended", m)
        self.assertIn("10.0.0.7 is back under the cap", m)
        self.assertIn("now quiet, during the alarm 80.5/s", m)

    def test_a_source_with_no_device_says_so(self):
        m = r.message(entry(1, source="10.0.0.9", devices="10.0.0.9"))
        self.assertIn("Dante devices at that address: none listed", m)


class Mcp(unittest.TestCase):
    """send() against a fake peer channel speaking the same MCP lines."""

    def fake(self, body):
        d = tempfile.mkdtemp()
        p = os.path.join(d, "fake_peer.py")
        with open(p, "w") as f:
            f.write(textwrap.dedent(body))
        self.log = os.path.join(d, "in.jsonl")
        return [sys.executable, "-I", p, self.log]

    def test_peer_send_goes_after_the_handshake_and_its_answer_counts(self):
        cmd = self.fake("""
            import json, sys
            seen = open(sys.argv[1], "w", buffering=1)
            for line in sys.stdin:
                m = json.loads(line); seen.write(line)
                if m.get("method") == "initialize":
                    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": {}}))
                elif m.get("method") == "tools/call":
                    a = m["params"]["arguments"]
                    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": {
                        "content": [{"type": "text", "text": "delivered to 1 %s session(s)" % a["to"]}],
                        "isError": False}}))
        """)
        ok, said = r.send("hello", cmd)
        self.assertTrue(ok)
        self.assertEqual(said, "delivered to 1 dstack session(s)")
        with open(self.log) as f:
            sent = [json.loads(line) for line in f]
        self.assertEqual([m.get("method") for m in sent],
                         ["initialize", "notifications/initialized", "tools/call"])
        self.assertEqual(sent[2]["params"], {"name": "peer_send",
                                             "arguments": {"to": "dstack", "text": "hello"}})

    def test_no_live_session_is_not_delivered(self):
        cmd = self.fake("""
            import json, sys
            for line in sys.stdin:
                m = json.loads(line)
                if m.get("method") == "tools/call":
                    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": {
                        "content": [{"type": "text", "text": "dstack has no live session"}],
                        "isError": True}}))
        """)
        self.assertEqual(r.send("x", cmd), (False, "dstack has no live session"))

    def test_a_channel_that_dies_is_not_delivered(self):
        cmd = self.fake("import sys; sys.stderr.write('boom'); sys.exit(3)")
        ok, said = r.send("x", cmd)
        self.assertFalse(ok)
        self.assertIn("exit 3: boom", said)


class Run(unittest.TestCase):
    def setUp(self):
        self.state = os.path.join(tempfile.mkdtemp(), "s", "cursor.json")
        self.sent = []

    def sender(self, results):
        def go(text, _cmd):
            self.sent.append(text)
            return results.pop(0)
        return go

    def test_the_first_run_replays_no_history(self):
        es = [entry(90), entry(110)]
        self.assertEqual(r.run("u", self.state, [], 100, get=lambda _: es,
                               sender=self.sender([(True, "ok")])), 0)
        self.assertEqual(len(self.sent), 1)
        self.assertIn(r.stamp(110), self.sent[0])
        self.assertEqual(r.load(self.state)["at"], 110)

    def test_each_alarm_goes_once_and_an_undelivered_one_waits_in_order(self):
        r.save(self.state, {"at": 100, "sent": []})
        es = [entry(110), entry(120, ok=True, after="3/s")]
        r.run("u", self.state, [], 200, get=lambda _: es,
              sender=self.sender([(False, "dstack has no live session")]))
        self.assertEqual(r.load(self.state)["at"], 100)
        r.run("u", self.state, [], 200, get=lambda _: es,
              sender=self.sender([(True, "ok"), (True, "ok")]))
        self.assertEqual(len(self.sent), 3)
        self.assertIn("over the cap", self.sent[1])
        self.assertIn("back under the cap", self.sent[2])
        r.run("u", self.state, [], 200, get=lambda _: es, sender=self.sender([]))
        self.assertEqual(len(self.sent), 3)

    def test_an_unreadable_server_is_an_error(self):
        r.save(self.state, {"at": 100, "sent": []})

        def down(_):
            raise OSError("connection refused")
        self.assertEqual(r.run("u", self.state, [], 200, get=down, sender=self.sender([])), 1)


if __name__ == "__main__":
    unittest.main()
