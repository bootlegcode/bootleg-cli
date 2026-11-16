#!/usr/bin/env python3
"""Offline regression tests for PTY closure racing with child exit."""
import errno
import os
import pty
import select
import subprocess
import sys
import time
import unittest
from unittest.mock import Mock, patch

from acceptance_pty import finish_process, read_pty


class PtyLifecycleTests(unittest.TestCase):
    def test_slave_closure_before_process_exit(self):
        master, slave = pty.openpty()
        # The stdin pipe is a barrier: the child cannot exit until after we
        # observe PTY closure. No timing-dependent sleep is needed.
        child = subprocess.Popen(
            [sys.executable, "-c",
             "import os, sys; os.close(2); sys.stdin.buffer.read(1); print('done')"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=slave,
        )
        os.close(slave)
        try:
            self.assertTrue(select.select([master], [], [], 5)[0])
            self.assertEqual(read_pty(master), b"")
            self.assertIsNone(child.poll())  # The old assertion would fail here.
            child.stdin.write(b"x")
            child.stdin.flush()
            stdout, _ = finish_process(child, time.monotonic() + 5)
            self.assertEqual(stdout, b"done\n")
            self.assertEqual(child.returncode, 0)
        finally:
            os.close(master)
            if child.poll() is None:
                child.kill()
            child.communicate()

    def test_unexpected_read_error_is_not_eof(self):
        with patch("acceptance_pty.os.read", side_effect=OSError(errno.EBADF, "bad fd")):
            with self.assertRaises(OSError) as caught:
                read_pty(123)
        self.assertEqual(caught.exception.errno, errno.EBADF)

    def test_wait_preserves_deadline(self):
        proc = Mock()
        with patch("acceptance_pty.time.monotonic", return_value=10):
            finish_process(proc, 12)
            proc.communicate.assert_called_once_with(timeout=2)
        proc.reset_mock()
        with patch("acceptance_pty.time.monotonic", return_value=13):
            finish_process(proc, 12)
            proc.communicate.assert_called_once_with(timeout=0)


if __name__ == "__main__":
    unittest.main()
