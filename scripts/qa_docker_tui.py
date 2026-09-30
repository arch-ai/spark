#!/usr/bin/env python3
"""Bounded Linux PTY smoke test; never invokes the real Docker or PM2 CLIs."""
import codecs
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unicodedata


class Screen:
    """Minimal VT screen for the cursor-positioned output emitted by Ratatui."""
    def __init__(self, width=100, height=24):
        self.width, self.height = width, height
        self.cells = [[' '] * width for _ in range(height)]
        self.x = self.y = 0
        self.decoder = codecs.getincrementaldecoder('utf-8')('replace')
        self.pending = ''

    def feed(self, data):
        self.pending += self.decoder.decode(data)
        while self.pending:
            if self.pending.startswith('\x1b['):
                match = re.match(r'\x1b\[([0-?]*)([ -/]*)([@-~])', self.pending)
                if not match:
                    return
                args, _, code = match.groups()
                values = [int(value) if value else 1 for value in args.split(';')] if not args.startswith('?') else []
                if code in ('H', 'f'):
                    self.y = (values[0] if values else 1) - 1
                    self.x = (values[1] if len(values) > 1 else 1) - 1
                elif code == 'J' and args == '2':
                    self.cells = [[' '] * self.width for _ in range(self.height)]
                self.pending = self.pending[match.end():]
                continue
            ch, self.pending = self.pending[0], self.pending[1:]
            if ch == '\r':
                self.x = 0
            elif ch == '\n':
                self.y += 1
            elif ch >= ' ' and not unicodedata.combining(ch):
                if 0 <= self.x < self.width and 0 <= self.y < self.height:
                    self.cells[self.y][self.x] = ch
                self.x += 2 if unicodedata.east_asian_width(ch) in ('W', 'F') else 1

    def text(self):
        return '\n'.join(''.join(row) for row in self.cells)


def main():
    binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/spark').resolve())
    with tempfile.TemporaryDirectory(prefix='spark-tui-') as directory:
        fixture = Path(directory)
        docker = fixture / 'docker'
        docker.write_text('''#!/usr/bin/python3
import datetime, json, os, pathlib, sys, time
root = pathlib.Path(os.environ['SPARK_QA_DIR'])
args = sys.argv[1:]
large_volume = 'abcdef0123456789' * 4
if (root / 'offline').exists():
    print('QA daemon unavailable', file=sys.stderr)
    sys.exit(1)
if args[:2] == ['image', 'ls']:
    time.sleep(2)
    print('demo:latest|sha256:abcdef|2MB')
elif args[:2] == ['volume', 'ls']:
    if not (root / 'qa-volume-deleted').exists():
        print('qa-volume')
    if not (root / 'deleted').exists():
        print(large_volume)
elif args and args[0] == 'rm':
    assert args[:3] == ['rm', '-f', '--'], args
    assert args[-1] == '0123456789abcdef', args
    with (root / 'container-deletions').open('a') as log:
        log.write(json.dumps(args) + '\\n')
    if (root / 'deny-container').exists():
        time.sleep(4.5)
        print('QA container removal denied', file=sys.stderr)
        sys.exit(1)
    (root / 'container-deleted').touch()
    print(args[-1])
elif args[:2] == ['volume', 'rm']:
    assert args == ['volume', 'rm', '--', args[-1]], args
    with (root / 'deletions').open('a') as log:
        log.write(json.dumps(args) + '\\n')
    assert args[-1] in ('qa-volume', large_volume), args
    if args[-1] == 'qa-volume':
        assert (root / 'container-deleted').exists(), 'container must be removed first'
        (root / 'qa-volume-deleted').touch()
    else:
        time.sleep(2)
        (root / 'deleted').touch()
    print(args[-1])
elif args[:2] == ['volume', 'inspect']:
    print(json.dumps([{'Name':args[-1], 'CreatedAt':'2020-01-01T00:00:00Z', 'Driver':'local', 'Mountpoint':'/var/lib/docker/volumes/qa-volume/_data', 'Scope':'local'}]))
elif args[:3] == ['system', 'df', '-v']:
    print('Local Volumes space usage:')
    print('NAME        LINKS      SIZE')
    print('qa-volume   1          950 GB')
    if not (root / 'deleted').exists():
        print(large_volume + ' 0 800 GB')
    print('Build cache space usage:')
elif args and args[0] == 'ps' and '-q' in args:
    if not (root / 'container-deleted').exists():
        print('0123456789abcdef')
elif args[:3] == ['inspect', '--type', 'container']:
    finished = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=2)).isoformat()
    print(json.dumps({'Id':'0123456789abcdef', 'Name':'/qa-api', 'Status':'exited', 'FinishedAt':finished, 'Mounts':[{'Type':'volume', 'Name':'qa-volume'}], 'Image':'demo:latest', 'Project':'qa-project'}))
elif args[:2] == ['system', 'df']:
    print('Images|1|1|2MB|0B (0%)')
    print('Containers|1|1|0B|0B (0%)')
    print('Local Volumes|0|0|0B|0B (0%)')
    print('Build Cache|0|0|0B|0B (0%)')
elif args and args[0] == 'ps':
    print('0123456789abcdef|qa-api|demo:latest|0.0.0.0:8080->80/tcp|Up 2 hours (healthy)|com.docker.compose.project=qa-project')
else:
    print('Unexpected fixture command: ' + repr(args), file=sys.stderr)
    sys.exit(2)
''')
        docker.chmod(0o755)
        pm2 = fixture / 'pm2'
        pm2.write_text('#!/bin/sh\nexit 1\n')
        pm2.chmod(0o755)
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 100, 0, 0))
        env = dict(os.environ, PATH=f'{fixture}:' + os.environ['PATH'], TERM='xterm-256color', SPARK_QA_DIR=directory)
        proc = subprocess.Popen([binary], stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True)
        screen = Screen()

        def expect(text, timeout=8, absent=False):
            if (text not in screen.text()) if absent else (text in screen.text()):
                return
            received = bytearray()
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], max(0, deadline - time.monotonic()))
                if not ready:
                    break
                chunk = os.read(master, 65536)
                received.extend(chunk)
                screen.feed(chunk)
                if (text not in screen.text()) if absent else (text in screen.text()):
                    return
            raise AssertionError(f'Did not render {text!r}:\n{screen.text()}')

        def click_text(label, button=0, near_right=False):
            if label not in screen.text():
                expect(label)
            for y, row in enumerate(screen.text().splitlines()):
                if label in row:
                    x = 90 if near_right else row.index(label) + 2
                    os.write(master, f'\x1b[<{button};{x + 1};{y + 1}M'.encode())
                    return
            raise AssertionError(f'Cannot click {label!r}:\n{screen.text()}')

        try:
            expect('PROCESS VIEW')
            os.write(master, b'd')
            expect('qa-api')
            (fixture / 'offline').touch()
            os.write(master, b'\x1b[15~')  # F5
            expect('QA daemon unavailable')
            (fixture / 'offline').unlink()
            os.write(master, b'\x1b[15~')
            expect('Updated')
            os.write(master, b'v')
            expect('Stop 2d ago')
            expect('Containers: qa-api')
            expect('Images: demo:latest')
            assert '950 GB' in screen.text(), screen.text()
            assert '800 GB' in screen.text(), screen.text()
            assert 'abcdef0123456789' in screen.text() and 'Unknown' in screen.text(), screen.text()
            os.write(master, b'\x1b[B')
            expect('Containers: None')
            expect('Images: None')
            os.write(master, b'\x1b[A')
            expect('Containers: qa-api')
            expect('Images: demo:latest')
            os.write(master, b'\r')
            expect('Size: 950 GB')
            assert 'Volume details:' in screen.text(), screen.text()
            expect('Last file access: Unknown')
            os.write(master, b'\x1b')
            expect('Volume details:', absent=True)
            expect('Docker Volumes')
            (fixture / 'deny-container').touch()
            os.write(master, b'\x1b[3~')  # Attached-container removal will fail
            expect('Confirm Delete')
            click_text('[ Yes ]')
            expect('Deleting 0:00')
            os.write(master, b'\x1b[3~y')  # Duplicate must not start another CLI
            expect('Deleting 0:03')  # Progress survives the old 3-second toast lifetime
            expect('Delete failed')
            expect('QA container removal denied')
            assert not (fixture / 'deletions').exists()
            assert len((fixture / 'container-deletions').read_text().splitlines()) == 1
            os.write(master, b'\x1b')
            expect('Delete failed', absent=True)
            expect('Docker Volumes')
            assert 'qa-volume' in screen.text(), screen.text()
            click_text('abcdef0123456789', button=2, near_right=True)
            expect('Delete Volume')
            click_text('Delete Volume')  # Rendered menu shifted left for the full name
            expect('Confirm Delete')
            click_text('[ Yes ]')
            expect('Deleting 0:00')
            os.write(master, b'\x1b')  # Browse while the slow delete runs
            expect('Docker Volumes', absent=True)
            expect('DOCKER')
            os.write(master, b'd')
            expect('PROCESS VIEW')
            expect('Deletion complete')
            os.write(master, b'\x1b')
            expect('Deletion complete', absent=True)
            expect('PROCESS VIEW')
            os.write(master, b'dv')
            expect('Stop 2d ago')
            assert 'qa-volume' in screen.text(), screen.text()
            assert 'abcdef0123456789' not in screen.text(), screen.text()
            assert len((fixture / 'deletions').read_text().splitlines()) == 1
            (fixture / 'deny-container').unlink()
            os.write(master, b'\x1b[3~')
            expect('Running containers stop.')
            os.write(master, b'y')
            expect('Deletion complete')
            expect('Removed container qa-api')
            expect('Removed volume qa-volume')
            assert len((fixture / 'container-deletions').read_text().splitlines()) == 2
            assert len((fixture / 'deletions').read_text().splitlines()) == 2
            os.write(master, b'\x1b')
            expect('Deletion complete', absent=True)
            expect('Docker Volumes')
            os.write(master, b'\x1b')
            expect('Docker Volumes', absent=True)
            expect('DOCKER')
            os.write(master, b'i')
            expect('Loading')
            started = time.monotonic()
            os.write(master, b'\x1b')
            expect('Docker Images', absent=True)
            expect('DOCKER')
            os.write(master, b'd')
            expect('PROCESS VIEW')
            os.write(master, b'q')
            proc.wait(timeout=2)
            elapsed = time.monotonic() - started
            assert proc.returncode == 0, f'exit code {proc.returncode}'
            restored = termios.tcgetattr(slave)
            assert restored == original, 'terminal attributes were not restored'
            assert elapsed < 1.5, f'slow Docker list blocked navigation/quit for {elapsed:.2f}s'
            print(f'PASS: volume details, long-name menu click, slow deletion, duplicate prevention, container removal failure/success, successful removal, daemon failure/recovery, slow-list cancellation, navigation, clean exit, terminal restoration ({elapsed:.2f}s).')
        finally:
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=2)
            os.close(master)
            os.close(slave)


if __name__ == '__main__':
    main()
