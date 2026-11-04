#!/usr/bin/env python3
"""Tests for fetch_data.py against a local server that misbehaves on purpose."""

from __future__ import annotations

import http.server
import sys
import tempfile
import threading
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import fetch_data  # noqa: E402

PINNED_SIZES = dict(fetch_data.SIZES)


class Misbehaving(http.server.BaseHTTPRequestHandler):
    """Each path is one way for a download to go wrong."""

    protocol_version = "HTTP/1.1"
    body = b"DAF/SPK " + b"x" * 5_000

    def log_message(self, *args):
        pass

    def do_GET(self):
        if self.path == "/whole":
            self.send_response(200)
            self.send_header("Content-Length", str(len(self.body)))
            self.end_headers()
            self.wfile.write(self.body)
        elif self.path == "/short":
            self.send_response(200)
            self.send_header("Content-Length", str(len(self.body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(self.body[:1000])
            self.close_connection = True
        elif self.path == "/chunked-cut":
            self.send_response(200)
            self.send_header("Transfer-Encoding", "chunked")
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(b"400\r\n" + self.body[:1024] + b"\r\n400\r\n" + self.body[:100])
            self.close_connection = True
        elif self.path == "/html":
            page = b"<html><body>Please sign in to this network</body></html>" * 10
            self.send_response(200)
            self.send_header("Content-Length", str(len(page)))
            self.end_headers()
            self.wfile.write(page)
        else:
            self.send_error(404)


class Fetch(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Misbehaving)
        cls.base = f"http://127.0.0.1:{cls.server.server_address[1]}"
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.data = Path(self.dir.name)
        self.dest = self.data / "spice" / "kernel.bsp"
        sizes = {"spice/kernel.bsp": len(Misbehaving.body)}
        self.addCleanup(setattr, fetch_data, "SIZES", fetch_data.SIZES)
        fetch_data.SIZES = sizes

    def get(self, path, rel="spice/kernel.bsp", expect=fetch_data.KERNEL):
        return fetch_data.download(self.base + path, self.dest, rel, expect)

    def leftovers(self):
        return [p.name for p in self.dest.parent.iterdir() if p.name.endswith(".part")]

    def test_a_whole_file_lands(self):
        ok, detail = self.get("/whole")
        self.assertTrue(ok, detail)
        self.assertEqual(self.dest.read_bytes(), Misbehaving.body)
        self.assertEqual(self.leftovers(), [])

    def test_a_short_file_is_a_failure_and_not_promoted(self):
        ok, detail = self.get("/short", rel="unpinned.bsp")
        self.assertFalse(ok)
        self.assertIn("1,000 of 5,008 bytes", detail)
        self.assertFalse(self.dest.exists())
        self.assertEqual(self.leftovers(), [])

    def test_a_chunked_reply_cut_off_is_a_failure_and_not_a_crash(self):
        ok, detail = self.get("/chunked-cut", rel="unpinned.bsp")
        self.assertFalse(ok)
        self.assertIn("IncompleteRead", detail)
        self.assertFalse(self.dest.exists())
        self.assertEqual(self.leftovers(), [])

    def test_an_html_page_is_not_saved_as_a_map(self):
        ok, detail = self.get("/html", rel="textures/map.jpg", expect=fetch_data.JPEG)
        self.assertFalse(ok)
        self.assertIn("marker", detail)
        self.assertFalse(self.dest.exists())

    def test_a_404_is_a_failure(self):
        ok, _detail = self.get("/nope")
        self.assertFalse(ok)
        self.assertFalse(self.dest.exists())

    def test_the_wrong_size_is_not_promoted(self):
        fetch_data.SIZES = {"spice/kernel.bsp": len(Misbehaving.body) + 1}
        ok, detail = self.get("/whole")
        self.assertFalse(ok)
        self.assertIn("expected", detail)
        self.assertFalse(self.dest.exists())

    def test_a_truncated_file_is_found_and_repaired(self):
        self.dest.parent.mkdir(parents=True)
        self.dest.write_bytes(Misbehaving.body[:1000])
        rel = "spice/kernel.bsp"
        self.assertIn("1,000 bytes", fetch_data.defect(self.dest, rel, fetch_data.KERNEL))
        ok, detail = self.get("/whole")
        self.assertTrue(ok, detail)
        self.assertIsNone(fetch_data.defect(self.dest, rel, fetch_data.KERNEL))

    def test_a_failed_repair_keeps_what_was_there(self):
        self.dest.parent.mkdir(parents=True)
        self.dest.write_bytes(Misbehaving.body[:1000])
        ok, _detail = self.get("/short")
        self.assertFalse(ok)
        self.assertEqual(self.dest.read_bytes(), Misbehaving.body[:1000])

    def test_a_horizons_table_without_its_end_is_a_defect(self):
        path = self.data / "track.txt"
        path.write_bytes(b"$$SOE\n1,2,3\n")
        self.assertIsNotNone(fetch_data.defect(path, "spacecraft/x.txt", b"$$EOE"))
        path.write_bytes(b"$$SOE\n1,2,3\n$$EOE\n")
        self.assertIsNone(fetch_data.defect(path, "spacecraft/x.txt", b"$$EOE"))

    def test_a_missing_file_is_a_defect(self):
        self.assertEqual(fetch_data.defect(self.data / "nope", "nope", None), "missing")

    def test_every_pinned_size_is_a_file_in_the_manifest(self):
        rels = {item[1] for item in fetch_data.manifest()}
        self.assertLessEqual(set(PINNED_SIZES), rels)


if __name__ == "__main__":
    unittest.main()
