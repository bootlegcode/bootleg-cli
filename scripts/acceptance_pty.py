"""PTY lifecycle helpers for interactive acceptance tests."""
import errno
import os
import time


def read_pty(fd):
    """Linux reports slave closure as EIO; other platforms return EOF."""
    try:
        return os.read(fd, 65536)
    except OSError as error:
        if error.errno != errno.EIO:
            raise
        return b""


def finish_process(proc, deadline):
    """PTY EOF is not process exit. Reap within the original interaction budget."""
    return proc.communicate(timeout=max(0, deadline - time.monotonic()))
