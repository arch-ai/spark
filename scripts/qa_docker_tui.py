#!/usr/bin/env python3
"""Bounded Linux PTY smoke test; never invokes the real Docker or PM2 CLIs."""
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shutil
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
        self.clipboards = []

    def feed(self, data):
        self.pending += self.decoder.decode(data)
        while self.pending:
            if self.pending == '\x1b':
                return
            if self.pending.startswith('\x1b]'):
                end = self.pending.find('\x07')
                if end < 0:
                    return
                payload = self.pending[2:end]
                if payload.startswith('52;c;'):
                    import base64
                    self.clipboards.append(base64.b64decode(payload[5:]).decode('utf-8'))
                self.pending = self.pending[end + 1:]
                continue
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
import datetime, json, os, pathlib, signal, sys, time
root = pathlib.Path(os.environ['SPARK_QA_DIR'])
args = sys.argv[1:]
large_volume = 'abcdef0123456789' * 4
if (root / 'offline').exists():
    print('QA daemon unavailable', file=sys.stderr)
    sys.exit(1)
if args and args[0] == 'stats':
    signal.pause()
elif args[:2] == ['image', 'ls']:
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
    print(json.dumps({'Id':'0123456789abcdef', 'Name':'/qa-api', 'Status':'exited', 'FinishedAt':finished, 'Mounts':[{'Type':'volume', 'Name':'qa-volume'}], 'Image':'demo:latest', 'Project':'qa-project','WorkingDir':'/tmp/qa-project'}))
elif args[:2] == ['builder', 'prune']:
    time.sleep(1.5)
    print('QA prune permission denied: café', file=sys.stderr)
    sys.exit(1)
elif args[:2] == ['system', 'df']:
    print('Images|1|1|2MB|0B (0%)')
    print('Containers|1|1|0B|0B (0%)')
    print('Local Volumes|0|0|0B|0B (0%)')
    print('Build Cache|0|0|0B|0B (0%)')
elif args and args[0] == 'ps':
    print('0123456789abcdef|qa-api|demo:latest|0.0.0.0:8080->80/tcp|Up 2 hours (healthy)|com.docker.compose.project=qa-project')
    print('deadbeef00000001|qa-other|demo:latest|0.0.0.0:9090->90/tcp|Up 2 hours|com.docker.compose.project=qa-project')
else:
    print('Unexpected fixture command: ' + repr(args), file=sys.stderr)
    sys.exit(2)
''')
        docker.chmod(0o755)
        pm2 = fixture / 'pm2'
        pm2.write_text('''#!/usr/bin/python3
import json, os, pathlib, sys, time
root = pathlib.Path(os.environ['SPARK_QA_DIR'])
args = sys.argv[1:]
if args and args[0] == 'jlist':
    if not (root / 'pm2-first-query').exists():
        (root / 'pm2-first-query').touch()
        time.sleep(3)
    if (root / 'pm2-offline').exists():
        print('QA PM2 list unavailable', file=sys.stderr)
        sys.exit(1)
    print(json.dumps([{'pm_id':7,'name':'qa-worker','pid':111,'monit':{'cpu':1.5,'memory':47185920},'pm2_env':{'status':'online','exec_mode':'fork_mode','pm_exec_path':'/tmp/qa.js','pm_cwd':'/tmp'}}, {'pm_id':8,'name':'qa-worker-2','pid':112,'monit':{'cpu':2.5,'memory':94371840},'pm2_env':{'status':'online','exec_mode':'fork_mode','pm_exec_path':'/tmp/qa2.js','pm_cwd':'/tmp'}}]))
elif args == ['restart', '7']:
    with (root / 'pm2-actions').open('a') as log:
        print(json.dumps(args), file=log)
    time.sleep(2)
    print('QA PM2 restart denied', file=sys.stderr)
    sys.exit(1)
else:
    print('Unexpected fake PM2 command: ' + repr(args), file=sys.stderr)
    sys.exit(2)
''')
        pm2.chmod(0o755)
        terminal_ready = fixture / 'terminal-ready'
        os.mkfifo(terminal_ready)
        terminal_fd = os.open(terminal_ready, os.O_RDWR | os.O_NONBLOCK)
        terminal = fixture / 'terminator'
        terminal.write_text('''#!/usr/bin/python3
import json, os, pathlib, sys
print('QA TERMINAL STDOUT LEAK', flush=True)
print('ConfigBase::load: QA TERMINAL STDERR LEAK', file=sys.stderr, flush=True)
with (pathlib.Path(os.environ['SPARK_QA_DIR']) / 'terminal-ready').open('w') as ready:
    ready.write(json.dumps(sys.argv[1:]))
''')
        terminal.chmod(0o755)
        (fixture / 'gnome-terminal').symlink_to(terminal.name)
        (fixture / 'x-terminal-emulator').write_bytes(terminal.read_bytes())
        (fixture / 'x-terminal-emulator').chmod(0o755)
        # An owned fixture process with a Node executable path and script argument.
        # It blocks on stdin, so native snapshots can be checked during a slow PM2 query.
        native_script = fixture / 'qa-native.js'
        native_script.write_text('')
        native_bin = fixture / 'node'
        native_bin.write_bytes(Path(shutil.which('cat')).read_bytes())
        native_bin.chmod(0o755)
        native_proc = subprocess.Popen([str(native_bin), str(native_script), '-'], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        native_pid = str(native_proc.pid)
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 100, 0, 0))
        # Restrict PATH so fallback tests cannot launch a real desktop terminal.
        env = dict(os.environ, PATH=str(fixture), TERM='xterm-256color', TERMINAL=str(terminal), SPARK_QA_DIR=directory, SPARK_CONFIG_DIR=str(fixture / 'config'))
        proc = subprocess.Popen([binary], stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True)
        screen = Screen()
        ui_output = bytearray()

        def expect(text, timeout=8, absent=False, predicate=None):
            matches = lambda: predicate(screen.text()) if predicate else ((text not in screen.text()) if absent else (text in screen.text()))
            if matches():
                return
            received = bytearray()
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], max(0, deadline - time.monotonic()))
                if not ready:
                    break
                chunk = os.read(master, 65536)
                ui_output.extend(chunk)
                received.extend(chunk)
                screen.feed(chunk)
                if matches():
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

        def click_header(label, row_marker):
            expect(f'column header {label}', predicate=lambda text: any(label in row and row_marker in row for row in text.splitlines()))
            for y, row in enumerate(screen.text().splitlines()):
                if label in row and row_marker in row:
                    x = row.index(label) + len(label) // 2
                    os.write(master, f'\x1b[<0;{x + 1};{y + 1}M'.encode())
                    return
            raise AssertionError(f'Cannot click header {label!r}:\n{screen.text()}')

        def search_via_mouse(value):
            click_text('/ to filter...')
            os.write(master, value.encode() + bytes([13]))
            expect('Filter: ' + value)

        def click_tab(label):
            expect('Node tab bar', predicate=lambda text: any('Node.js Processes' in row and 'PM2' in row for row in text.splitlines()))
            for y, row in enumerate(screen.text().splitlines()):
                if 'Node.js Processes' in row and 'PM2' in row:
                    x = row.index(label) + len(label) // 2
                    os.write(master, f'\x1b[<0;{x + 1};{y + 1}M'.encode())
                    return
            raise AssertionError(f'Cannot click tab {label!r}:\n{screen.text()}')

        def check_terminal_launch(action, expected_command, execute_option):
            if action == 'Shell':
                click_text('qa-api')
                os.write(master, b'\r')
            else:
                click_text('qa-api')
                os.write(master, b'\x1b[21~')  # F10
                expect(action)
                click_text(action)
            ready, _, _ = select.select([terminal_fd], [], [], 8)
            assert ready, f'Terminal launcher did not acknowledge {action}'
            arguments = json.loads(os.read(terminal_fd, 65536))
            assert arguments == [execute_option, 'bash', '-lc', expected_command], arguments
            # A fresh render after the launch receipt consumes any leaked output.
            os.write(master, b'?')
            expect('KEYBOARD HELP')
            assert b'QA TERMINAL STDOUT LEAK' not in ui_output, 'Terminal stdout leaked into Spark'
            assert b'QA TERMINAL STDERR LEAK' not in ui_output, 'Terminal stderr leaked into Spark'
            os.write(master, b'\x1b')
            expect('KEYBOARD HELP', absent=True)

        try:
            expect('PROCESS VIEW')
            search_via_mouse('qa-search')
            os.write(master, b'x')
            expect('/ to filter...')
            logo = [row[1:19] for row in screen.cells[1:8]]
            expect('animated logo', timeout=1, predicate=lambda text: [row[1:19] for row in screen.cells[1:8]] != logo)
            os.write(master, b' ')
            expect('Logo animation paused')
            os.write(master, b' ')
            expect('Logo animation resumed')
            os.write(master, b'?')
            expect('KEYBOARD HELP')
            expect('SORTING')
            os.write(master, bytes([27]))
            expect('KEYBOARD HELP', absent=True)
            os.write(master, b's' + bytes([27]) + b'[F' + bytes([13]))
            expect('Sort: PID asc')
            os.write(master, b'r')
            expect('Sort: PID desc')
            click_header('CPU', 'TREE')
            expect('Sort: CPU desc')
            click_header('CPU', 'TREE')
            expect('Sort: CPU asc')
            click_header('RAM', 'TREE')
            expect('Sort: RAM desc')
            click_header('TREE', 'PID')
            expect('Sort: TREE RAM desc')
            expect('TREE ▼')
            os.write(master, b'd')
            expect('qa-api')
            expect('qa-other')
            logs_command = 'docker logs -f --tail 200 0123456789abcdef; exec bash'
            shell_command = 'docker exec -it 0123456789abcdef bash 2>/dev/null || docker exec -it 0123456789abcdef sh; exec bash'
            check_terminal_launch('Logs - New Window', logs_command, '-x')
            # Keep a GNOME fixture after the configured Terminator disappears.
            (fixture / 'gnome-terminal').unlink()
            (fixture / 'gnome-terminal').write_bytes(terminal.read_bytes())
            (fixture / 'gnome-terminal').chmod(0o755)
            terminal.unlink()
            check_terminal_launch('Logs - New Window', logs_command, '--')
            check_terminal_launch('Shell', shell_command, '--')
            (fixture / 'gnome-terminal').unlink()
            check_terminal_launch('Logs - New Window', logs_command, '-e')
            check_terminal_launch('Shell', shell_command, '-e')
            search_via_mouse('qa')
            os.write(master, b'x')
            expect('/ to filter...')
            os.write(master, b's' + bytes([27]) + b'[H' + bytes([13]))
            expect('Name asc')
            os.write(master, b'r')
            expect('Name desc')
            expect('reversed Docker rows', predicate=lambda text: text.index('qa-other') < text.index('qa-api'))
            click_header('STATUS', 'NAME / PROJECT')
            expect('Status asc')
            click_header('STATUS', 'NAME / PROJECT')
            expect('Status desc')
            click_header('NAME / PROJECT', 'STATUS')
            expect('Name asc')
            click_header('NAME / PROJECT', 'STATUS')
            expect('Name desc')
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
            expect('Projects: qa-project: /tmp/qa-project')
            os.write(master, b'r')
            expect('Size asc')
            assert 'Containers: qa-api' in screen.text(), screen.text()
            os.write(master, b's' + bytes([27]) + b'[H' + bytes([27]) + b'[B' + bytes([13]))
            expect('Name asc')
            assert 'Containers: qa-api' in screen.text(), screen.text()
            os.write(master, b'r')
            expect('Name desc')
            assert '950 GB' in screen.text(), screen.text()
            assert '800 GB' in screen.text(), screen.text()
            assert 'abcdef0123456789' in screen.text() and 'Unknown' in screen.text(), screen.text()
            click_header('SIZE', 'ACTIVITY')
            expect('Size desc')
            expect('Containers: qa-api')
            click_header('SIZE', 'ACTIVITY')
            expect('Size asc')
            expect('Containers: qa-api')
            click_header('NAME', 'ACTIVITY')
            expect('Name asc')
            click_header('NAME', 'ACTIVITY')
            expect('Name desc')
            os.write(master, b'\x1b[B')
            expect('Containers: None')
            expect('Images: None')
            os.write(master, bytes([27]) + b'[15~')
            expect('Loading')
            expect('Containers: None')
            os.write(master, b'\x1b[A')
            expect('Containers: qa-api')
            expect('Images: demo:latest')
            os.write(master, b'\r')
            expect('Size: 950 GB')
            assert 'Volume details:' in screen.text(), screen.text()
            expect('Project directory: /tmp/qa-project')
            os.write(master, bytes([27]) + b'[6~')  # Scroll to the remaining activity/history
            expect('Last file access: Unknown')
            os.write(master, b'\x1b')
            expect('Volume details:', absent=True)
            expect('Docker Volumes')
            os.write(master, b'c')
            expect('Volume: qa-volume')
            expect('qa-api')
            expect('qa-other', absent=True)
            os.write(master, b'x')
            expect('qa-other')
            os.write(master, b'v')
            expect('Containers: qa-api')
            (fixture / 'deny-container').touch()
            os.write(master, bytes([27]) + b'[21~')  # F10 above the resource list
            expect('Delete Volume')
            os.write(master, bytes([27]) + b'[B' + bytes([13]))  # Keyboard menu selection
            # Attached-container removal will fail
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
            expect('Docker Volumes', predicate=lambda text: 'Docker Volumes' in text and 'qa-volume' in text and 'abcdef0123456789' in text)
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
            os.write(master, bytes([2]))  # Ctrl+B, prune confirmation
            expect('Confirm Prune')
            os.write(master, b'y')
            expect('Pruning build cache')
            os.write(master, b'2')  # Browse ports while cleanup runs
            expect('PORTS VIEW')
            search_via_mouse('qa')
            os.write(master, b'x')
            expect('/ to filter...')
            os.write(master, b's' + bytes([27]) + b'[H' + bytes([27]) + b'[B' + bytes([13]))
            expect('Name asc')
            os.write(master, b'r')
            expect('Name desc')
            click_header('EXT:INT', 'PROTO')
            expect('Port asc')
            click_header('EXT:INT', 'PROTO')
            expect('Port desc')
            expect('Prune failed')
            expect('QA prune permission denied: café')
            os.write(master, bytes([27]))
            expect('Prune failed', absent=True)
            started_native = time.monotonic()
            os.write(master, b'4')
            expect('NODE VIEW')
            search_via_mouse('qa-native')
            expect('native Node row', timeout=1.5, predicate=lambda text: any(row[21:29].strip() == native_pid for row in text.splitlines()))
            assert not (fixture / 'pm2-actions').exists()
            assert time.monotonic() - started_native < 1.5, 'Slow PM2 blocked native Node data'
            click_header('RSS', 'SCRIPT')
            expect('RSS desc')
            # Native tab remains selected even when its filter has no matches.
            os.write(master, b'/definitely-no-native-match' + bytes([13]))
            expect('No matches')
            expect('qa-worker', absent=True)
            os.write(master, b'x')
            os.write(master, bytes([9]))  # Tab to PM2 tab
            expect('qa-worker')
            click_tab('Node.js Processes')
            expect('SCRIPT')
            expect('qa-worker', absent=True)
            os.write(master, bytes([27]) + b'[Z')  # Shift+Tab back to PM2
            expect('qa-worker')
            os.write(master, b's' + bytes([27]) + b'[H' + bytes([27]) + b'[B' + bytes([27]) + b'[B' + bytes([13]))
            expect('RSS desc')
            expect('PM2 memory order', predicate=lambda text: text.index('qa-worker-2') < next(match.start() for match in re.finditer(r'qa-worker\s', text)))
            click_header('RSS', 'STATUS')
            expect('RSS asc')
            click_header('RSS', 'STATUS')
            expect('PM2 memory order after header clicks', predicate=lambda text: text.index('qa-worker-2') < next(match.start() for match in re.finditer(r'qa-worker\s', text)))
            os.write(master, bytes([27]) + b'[21~')
            expect('Restart')
            os.write(master, bytes([27]) + b'[B' + bytes([13]))
            expect('action(s) in progress')
            os.write(master, bytes([18]))  # Ctrl+R duplicate restart
            os.write(master, b'3')
            expect('DOCKER')
            expect('Action failed')
            expect('QA PM2 restart denied')
            assert len((fixture / 'pm2-actions').read_text().splitlines()) == 1
            os.write(master, bytes([27]))
            expect('Action failed', absent=True)
            os.write(master, b'4')
            expect('SCRIPT')
            expect('qa-worker', absent=True)
            click_tab('PM2')
            expect('qa-worker')
            (fixture / 'pm2-offline').touch()
            os.write(master, bytes([27]) + b'[15~')
            expect('QA PM2 list unavailable')
            expect('cached')
            assert 'qa-worker' in screen.text(), screen.text()
            (fixture / 'pm2-offline').unlink()
            os.write(master, bytes([27]) + b'[15~')
            expect('QA PM2 list unavailable', absent=True)
            os.write(master, b'3')
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
            print(f'PASS: isolated terminal launch output for logs/shell with Terminator/GNOME/system fallback, mouse search across all section headers, default native Node.js tab, keyboard/mouse tab switching with empty native results, independent tab sorting/selection, clickable column sorting and keyboard sorting across views/resource lists with preserved action targets, volume project directories/owner navigation, animated logo/pause/help, native Node data during a slow PM2 query, keyboard menus, background prune failure, slow PM2 action failure/duplicate prevention, PM2 stale-data recovery, volume details, long-name menu click, slow deletion, duplicate prevention, container removal failure/success, successful removal, daemon failure/recovery, slow-list cancellation, navigation, clean exit, terminal restoration ({elapsed:.2f}s).')
        finally:
            native_proc.terminate()
            native_proc.wait(timeout=2)
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=2)
            os.close(master)
            os.close(slave)
            os.close(terminal_fd)


if __name__ == '__main__':
    main()
